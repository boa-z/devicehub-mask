//! Network throughput sampling and privacy-bounded interface catalog policy.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use devicehub_core::{DeviceNetworkInterface, DeviceNetworkInterfaceKind};
use idevice::dvt::device_info::DeviceInfoClient;
use idevice::dvt::network_monitor::{NetworkEvent, NetworkMonitorClient};
use idevice::dvt::remote_server::RemoteServerClient;
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use idevice::{ReadWrite, RsdService};
use tokio::sync::watch;

use super::source::{SETUP_TIMEOUT, connect_remote, wait_until_enabled};
use super::{PerformanceSlot, update_network_sample};
use crate::supervisor::{ServiceReporter, reconnect_backoff, wait_for_retry};

const CATALOG_TIMEOUT: Duration = Duration::from_secs(3);
const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const CONNECTION_TTL: Duration = Duration::from_secs(60);
const MAX_CONNECTIONS: usize = 16_384;
const MAX_RAW_INTERFACES: usize = 256;
const MAX_INTERFACES: usize = 64;
const MAX_INTERFACE_NAME_BYTES: usize = 64;
const MAX_INTERFACE_DESCRIPTION_CHARS: usize = 96;
#[cfg(test)]
pub(super) const TEST_CONNECTION_TTL: Duration = CONNECTION_TTL;
#[cfg(test)]
pub(super) const TEST_MAX_INTERFACES: usize = MAX_INTERFACES;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct NetworkRateSample {
    pub(super) rx_bytes_per_second: f64,
    pub(super) tx_bytes_per_second: f64,
    pub(super) recent_connections: u32,
}

#[derive(Debug, Clone, Copy)]
struct NetworkConnectionCounters {
    rx_bytes: u64,
    tx_bytes: u64,
    last_seen: Instant,
    initialized: bool,
}

pub(super) struct NetworkAccumulator {
    connections: HashMap<u64, NetworkConnectionCounters>,
    window_rx_bytes: u64,
    window_tx_bytes: u64,
    window_started: Instant,
}

impl NetworkAccumulator {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            connections: HashMap::new(),
            window_rx_bytes: 0,
            window_tx_bytes: 0,
            window_started: now,
        }
    }

    pub(super) fn observe(&mut self, event: NetworkEvent, now: Instant) {
        match event {
            NetworkEvent::ConnectionDetection(event) => {
                if self.connections.len() < MAX_CONNECTIONS {
                    self.connections.entry(event.serial_number).or_insert(
                        NetworkConnectionCounters {
                            rx_bytes: 0,
                            tx_bytes: 0,
                            last_seen: now,
                            initialized: false,
                        },
                    );
                }
            }
            NetworkEvent::ConnectionUpdate(event) => {
                if let Some(previous) = self.connections.get_mut(&event.connection_serial) {
                    if previous.initialized {
                        self.window_rx_bytes = self
                            .window_rx_bytes
                            .saturating_add(event.rx_bytes.saturating_sub(previous.rx_bytes));
                        self.window_tx_bytes = self
                            .window_tx_bytes
                            .saturating_add(event.tx_bytes.saturating_sub(previous.tx_bytes));
                    }
                    previous.rx_bytes = event.rx_bytes;
                    previous.tx_bytes = event.tx_bytes;
                    previous.last_seen = now;
                    previous.initialized = true;
                } else if self.connections.len() < MAX_CONNECTIONS {
                    self.connections.insert(
                        event.connection_serial,
                        NetworkConnectionCounters {
                            rx_bytes: event.rx_bytes,
                            tx_bytes: event.tx_bytes,
                            last_seen: now,
                            initialized: true,
                        },
                    );
                }
            }
            NetworkEvent::InterfaceDetection(_) | NetworkEvent::Unknown(_) => {}
        }
    }

    pub(super) fn sample(&mut self, now: Instant) -> NetworkRateSample {
        self.connections.retain(|_, counters| {
            now.saturating_duration_since(counters.last_seen) <= CONNECTION_TTL
        });
        let elapsed = now
            .saturating_duration_since(self.window_started)
            .as_secs_f64()
            .max(f64::EPSILON);
        let sample = NetworkRateSample {
            rx_bytes_per_second: self.window_rx_bytes as f64 / elapsed,
            tx_bytes_per_second: self.window_tx_bytes as f64 / elapsed,
            recent_connections: self.connections.len().min(u32::MAX as usize) as u32,
        };
        self.window_rx_bytes = 0;
        self.window_tx_bytes = 0;
        self.window_started = now;
        sample
    }
}

pub(crate) async fn supervise_performance_network(
    adapter: AdapterHandle,
    handshake: RsdHandshake,
    slot: PerformanceSlot,
    reporter: ServiceReporter,
    mut enabled: watch::Receiver<bool>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut attempt = 0;
    loop {
        if *shutdown.borrow() {
            break;
        }
        if !wait_until_enabled(&mut enabled, &mut shutdown, &reporter, attempt).await {
            break;
        }
        attempt += 1;
        reporter.connecting(attempt);
        let result = run_once(
            adapter.clone(),
            handshake.clone(),
            slot.clone(),
            &mut shutdown,
            &mut enabled,
            &reporter,
            attempt,
        )
        .await;
        if *shutdown.borrow() {
            break;
        }
        let Some(error) = result.err() else {
            continue;
        };
        reporter.retrying(attempt, error);
        if !wait_for_retry(&mut shutdown, reconnect_backoff(attempt - 1)).await {
            break;
        }
    }
    reporter.stopped(attempt);
}

async fn run_once(
    adapter: AdapterHandle,
    handshake: RsdHandshake,
    slot: PerformanceSlot,
    shutdown: &mut watch::Receiver<bool>,
    enabled: &mut watch::Receiver<bool>,
    reporter: &ServiceReporter,
    attempt: u32,
) -> Result<(), String> {
    let mut remote = connect_remote(adapter, handshake).await?;
    let mut client = NetworkMonitorClient::new(&mut remote)
        .await
        .map_err(|error| format!("DVT network monitor channel failed: {error:?}"))?;
    tokio::time::timeout(SETUP_TIMEOUT, client.start_monitoring())
        .await
        .map_err(|_| "DVT network monitor setup timed out".to_string())?
        .map_err(|error| format!("DVT network monitor setup failed: {error:?}"))?;
    reporter.ready(attempt);
    let mut accumulator = NetworkAccumulator::new(Instant::now());
    let mut tick = tokio::time::interval(SAMPLE_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    tick.tick().await;
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    let _ = client.stop_monitoring().await;
                    return Ok(());
                }
            }
            changed = enabled.changed() => {
                if changed.is_err() || !*enabled.borrow() {
                    let _ = client.stop_monitoring().await;
                    return Ok(());
                }
            }
            event = client.next_event() => match event {
                Ok(event) => accumulator.observe(event, Instant::now()),
                Err(error) => return Err(format!("DVT network monitor stream failed: {error:?}")),
            },
            _ = tick.tick() => update_network_sample(&slot, accumulator.sample(Instant::now())),
        }
    }
}

pub(super) async fn load_interface_catalog(
    mut adapter: AdapterHandle,
    mut handshake: RsdHandshake,
) -> Result<plist::Dictionary, String> {
    tokio::time::timeout(CATALOG_TIMEOUT, async {
        let mut remote =
            RemoteServerClient::<Box<dyn ReadWrite>>::connect_rsd(&mut adapter, &mut handshake)
                .await?;
        let mut device_info = DeviceInfoClient::new(&mut remote).await?;
        device_info.network_information().await
    })
    .await
    .map_err(|_| "DVT network interface catalog request timed out".to_string())?
    .map_err(|error| format!("DVT network interface catalog request failed: {error:?}"))
}

pub(super) fn normalize_interfaces(
    network: &plist::Dictionary,
) -> (Vec<DeviceNetworkInterface>, bool) {
    let mut interfaces = network
        .iter()
        .take(MAX_RAW_INTERFACES)
        .filter_map(|(name, value)| {
            let name = normalize_interface_name(name)?;
            let description = value
                .as_string()
                .and_then(normalize_interface_description)?;
            Some(DeviceNetworkInterface {
                kind: classify_interface(&name, &description),
                name,
                description,
            })
        })
        .collect::<Vec<_>>();
    interfaces.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.name.cmp(&right.name))
    });
    let truncated = network.len() > MAX_RAW_INTERFACES || interfaces.len() > MAX_INTERFACES;
    interfaces.truncate(MAX_INTERFACES);
    (interfaces, truncated)
}

fn normalize_interface_name(value: &str) -> Option<String> {
    (!value.is_empty()
        && value.len() <= MAX_INTERFACE_NAME_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')))
    .then(|| value.to_string())
}

fn normalize_interface_description(value: &str) -> Option<String> {
    if value
        .chars()
        .any(|character| character.is_control() && !character.is_whitespace())
    {
        return None;
    }
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    (!normalized.is_empty()).then(|| {
        normalized
            .chars()
            .take(MAX_INTERFACE_DESCRIPTION_CHARS)
            .collect()
    })
}

fn classify_interface(name: &str, description: &str) -> DeviceNetworkInterfaceKind {
    let description = description.to_ascii_lowercase();
    if name == "lo0" || description.contains("loopback") {
        DeviceNetworkInterfaceKind::Loopback
    } else if name.starts_with("pdp_ip") || description.contains("cellular") {
        DeviceNetworkInterfaceKind::Cellular
    } else if description.contains("wi-fi") || description.contains("wifi") {
        DeviceNetworkInterfaceKind::Wifi
    } else if description.contains("ethernet") {
        DeviceNetworkInterfaceKind::Ethernet
    } else {
        DeviceNetworkInterfaceKind::Other
    }
}
