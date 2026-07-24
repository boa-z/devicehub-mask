//! Supervised DVT performance sampling over the active CoreDevice tunnel.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use idevice::dvt::device_info::DeviceInfoClient;
use idevice::dvt::energy_monitor::{EnergyMonitorClient, EnergySample};
use idevice::dvt::graphics::GraphicsClient;
use idevice::dvt::network_monitor::{NetworkEvent, NetworkMonitorClient};
use idevice::dvt::notifications::{NotificationInfo, NotificationsClient};
use idevice::dvt::remote_server::RemoteServerClient;
use idevice::dvt::sysmontap::{SysmontapClient, SysmontapConfig, SysmontapSample};
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use idevice::{ReadWrite, RsdService};
use plist::Value;
use serde::Serialize;
use tokio::sync::watch;

use crate::supervisor::{ServiceReporter, reconnect_backoff, wait_for_retry};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
const SETUP_TIMEOUT: Duration = Duration::from_secs(8);
const NETWORK_CATALOG_TIMEOUT: Duration = Duration::from_secs(3);
const SAMPLE_INTERVAL_MS: u32 = 1_000;
const TOP_PROCESSES_PER_METRIC: usize = 10;
const MAX_ENERGY_PROCESSES: usize = 16;
const ENERGY_SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const ENERGY_OPERATION_TIMEOUT: Duration = Duration::from_secs(4);
const NETWORK_SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const NETWORK_CONNECTION_TTL: Duration = Duration::from_secs(60);
const MAX_NETWORK_CONNECTIONS: usize = 16_384;
const MAX_ACTIVITY_EVENTS: usize = 100;
const MAX_ACTIVITY_TYPE_CHARS: usize = 96;
const MAX_ACTIVITY_NAME_CHARS: usize = 128;
const MAX_ACTIVITY_STATE_CHARS: usize = 160;
const MAX_RAW_NETWORK_INTERFACES: usize = 256;
const MAX_NETWORK_INTERFACES: usize = 64;
const MAX_NETWORK_INTERFACE_NAME_BYTES: usize = 64;
const MAX_NETWORK_INTERFACE_DESCRIPTION_CHARS: usize = 96;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProcessPerformance {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: Option<f64>,
    pub memory_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProcessEnergy {
    pub pid: u32,
    pub name: String,
    pub total_score: f64,
    pub cpu_score: f64,
    pub gpu_score: f64,
    pub networking_score: f64,
    pub display_score: f64,
    pub location_score: f64,
    pub app_state_score: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceNetworkInterfaceKind {
    Wifi,
    Cellular,
    Ethernet,
    Loopback,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceNetworkInterface {
    pub name: String,
    pub kind: DeviceNetworkInterfaceKind,
    pub description: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PerformanceSnapshot {
    pub captured_at_ms: u64,
    pub system_cpu_percent: Option<f64>,
    pub process_count: Option<u32>,
    pub logical_cpu_count: Option<u32>,
    pub physical_cpu_count: Option<u32>,
    pub physical_memory_bytes: Option<u64>,
    pub top_processes: Vec<ProcessPerformance>,
    pub energy_processes: Vec<ProcessEnergy>,
    pub graphics_fps: Option<f64>,
    pub gpu_allocated_bytes: Option<u64>,
    pub gpu_in_use_bytes: Option<u64>,
    pub gpu_driver_bytes: Option<u64>,
    pub gpu_recovery_count: Option<u64>,
    pub network_rx_bytes_per_second: Option<f64>,
    pub network_tx_bytes_per_second: Option<f64>,
    pub network_recent_connections: Option<u32>,
    pub network_interfaces: Vec<DeviceNetworkInterface>,
    pub network_interfaces_available: bool,
    pub network_interfaces_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppActivityEvent {
    pub sequence: u64,
    pub received_at_ms: u64,
    pub notification_type: String,
    pub app_name: Option<String>,
    pub exec_name: Option<String>,
    pub pid: Option<u32>,
    pub state_description: Option<String>,
}

struct PerformanceSlotInner {
    sample: Mutex<PerformanceSnapshot>,
    activity_events: Mutex<VecDeque<AppActivityEvent>>,
    activity_sequence: AtomicU64,
}

#[derive(Clone)]
pub struct PerformanceSlot(Arc<PerformanceSlotInner>);

impl Default for PerformanceSlot {
    fn default() -> Self {
        Self(Arc::new(PerformanceSlotInner {
            sample: Mutex::new(PerformanceSnapshot::default()),
            activity_events: Mutex::new(VecDeque::with_capacity(MAX_ACTIVITY_EVENTS)),
            activity_sequence: AtomicU64::new(0),
        }))
    }
}

impl PerformanceSlot {
    pub fn get(&self) -> PerformanceSnapshot {
        self.0.sample.lock().unwrap().clone()
    }

    pub fn app_activity(&self) -> Vec<AppActivityEvent> {
        self.0
            .activity_events
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .collect()
    }

    pub fn reset(&self) {
        *self.0.sample.lock().unwrap() = PerformanceSnapshot::default();
        self.0.activity_events.lock().unwrap().clear();
        self.0.activity_sequence.store(0, Ordering::Relaxed);
    }

    fn update_system(
        &self,
        sample: &SysmontapSample,
        cpu_count: u32,
        process_schema: &ProcessSchema,
    ) {
        let mut snapshot = self.0.sample.lock().unwrap();
        snapshot.captured_at_ms = unix_millis();
        snapshot.logical_cpu_count = Some(cpu_count);
        if let Some(processes) = sample.processes.as_ref() {
            snapshot.process_count = Some(processes.len() as u32);
            snapshot.top_processes = top_processes(processes, process_schema, cpu_count);
        }
        let raw_cpu_total_load = sample
            .system_cpu_usage
            .as_ref()
            .and_then(|cpu| cpu.get("CPU_TotalLoad"))
            .and_then(numeric_value);
        let normalized_cpu_load =
            raw_cpu_total_load.and_then(|value| normalize_aggregate_cpu_percent(value, cpu_count));
        if let Some(cpu) = sample.system_cpu_usage.as_ref() {
            tracing::debug!(
                raw_cpu_total_load,
                cpu_count,
                normalized_cpu_load,
                fields = ?cpu.keys().collect::<Vec<_>>(),
                "received DVT system CPU sample"
            );
            snapshot.system_cpu_percent = normalized_cpu_load;
        }
    }

    fn update_hardware(&self, hardware: &plist::Dictionary) {
        let mut snapshot = self.0.sample.lock().unwrap();
        snapshot.logical_cpu_count = cpu_count(hardware);
        snapshot.physical_cpu_count = physical_cpu_count(hardware);
        snapshot.physical_memory_bytes = physical_memory_bytes(hardware);
    }

    fn update_network_interfaces(&self, network: &plist::Dictionary) {
        let (interfaces, truncated) = normalize_network_interfaces(network);
        let mut snapshot = self.0.sample.lock().unwrap();
        snapshot.network_interfaces = interfaces;
        snapshot.network_interfaces_available = true;
        snapshot.network_interfaces_truncated = truncated;
    }

    fn update_graphics(&self, sample: &idevice::dvt::graphics::GraphicsSample) {
        let mut snapshot = self.0.sample.lock().unwrap();
        snapshot.captured_at_ms = unix_millis();
        snapshot.graphics_fps = sample.fps.is_finite().then_some(sample.fps.max(0.0));
        snapshot.gpu_allocated_bytes = Some(sample.alloc_system_memory);
        snapshot.gpu_in_use_bytes = Some(sample.in_use_system_memory);
        snapshot.gpu_driver_bytes = Some(sample.in_use_system_memory_driver);
        snapshot.gpu_recovery_count = Some(sample.recovery_count);
    }

    fn update_network(&self, sample: NetworkRateSample) {
        let mut snapshot = self.0.sample.lock().unwrap();
        snapshot.captured_at_ms = unix_millis();
        snapshot.network_rx_bytes_per_second = Some(sample.rx_bytes_per_second);
        snapshot.network_tx_bytes_per_second = Some(sample.tx_bytes_per_second);
        snapshot.network_recent_connections = Some(sample.recent_connections);
    }

    fn energy_targets(&self) -> Vec<u32> {
        let snapshot = self.0.sample.lock().unwrap();
        let mut seen = HashSet::with_capacity(MAX_ENERGY_PROCESSES);
        let mut pids = snapshot
            .top_processes
            .iter()
            .map(|process| process.pid)
            .filter(|pid| *pid > 0 && seen.insert(*pid))
            .take(MAX_ENERGY_PROCESSES)
            .collect::<Vec<_>>();
        pids.sort_unstable();
        pids
    }

    fn update_energy(&self, samples: Vec<EnergySample>) {
        let mut snapshot = self.0.sample.lock().unwrap();
        let names = snapshot
            .top_processes
            .iter()
            .map(|process| (process.pid, process.name.clone()))
            .collect::<HashMap<_, _>>();
        let mut processes = samples
            .into_iter()
            .filter(|sample| sample.pid > 0 && names.contains_key(&sample.pid))
            .map(|sample| ProcessEnergy {
                pid: sample.pid,
                name: names
                    .get(&sample.pid)
                    .cloned()
                    .unwrap_or_else(|| format!("pid {}", sample.pid)),
                total_score: energy_score(sample.total_energy),
                cpu_score: energy_score(sample.cpu_energy),
                gpu_score: energy_score(sample.gpu_energy),
                networking_score: energy_score(sample.networking_energy),
                display_score: energy_score(sample.display_energy),
                location_score: energy_score(sample.location_energy),
                app_state_score: energy_score(sample.appstate_energy),
            })
            .collect::<Vec<_>>();
        processes.sort_by(|left, right| {
            right
                .total_score
                .total_cmp(&left.total_score)
                .then_with(|| left.pid.cmp(&right.pid))
        });
        processes.truncate(MAX_ENERGY_PROCESSES);
        snapshot.captured_at_ms = unix_millis();
        snapshot.energy_processes = processes;
    }

    fn publish_app_activity(&self, notification: NotificationInfo) {
        let sequence = self
            .0
            .activity_sequence
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
            .max(1);
        let event = AppActivityEvent {
            sequence,
            received_at_ms: unix_millis(),
            notification_type: bounded_activity_text(
                &notification.notification_type,
                MAX_ACTIVITY_TYPE_CHARS,
            )
            .unwrap_or_else(|| "unknown".into()),
            app_name: bounded_activity_text(&notification.app_name, MAX_ACTIVITY_NAME_CHARS),
            exec_name: bounded_activity_text(&notification.exec_name, MAX_ACTIVITY_NAME_CHARS),
            pid: (notification.pid > 0).then_some(notification.pid),
            state_description: bounded_activity_text(
                &notification.state_description,
                MAX_ACTIVITY_STATE_CHARS,
            ),
        };
        let mut events = self.0.activity_events.lock().unwrap();
        if events.len() == MAX_ACTIVITY_EVENTS {
            events.pop_front();
        }
        events.push_back(event);
    }
}

fn bounded_activity_text(value: &str, max_chars: usize) -> Option<String> {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        None
    } else {
        Some(normalized.chars().take(max_chars).collect())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct NetworkRateSample {
    rx_bytes_per_second: f64,
    tx_bytes_per_second: f64,
    recent_connections: u32,
}

#[derive(Debug, Clone, Copy)]
struct NetworkConnectionCounters {
    rx_bytes: u64,
    tx_bytes: u64,
    last_seen: Instant,
    initialized: bool,
}

struct NetworkAccumulator {
    connections: HashMap<u64, NetworkConnectionCounters>,
    window_rx_bytes: u64,
    window_tx_bytes: u64,
    window_started: Instant,
}

impl NetworkAccumulator {
    fn new(now: Instant) -> Self {
        Self {
            connections: HashMap::new(),
            window_rx_bytes: 0,
            window_tx_bytes: 0,
            window_started: now,
        }
    }

    fn observe(&mut self, event: NetworkEvent, now: Instant) {
        match event {
            NetworkEvent::ConnectionDetection(event) => {
                if self.connections.len() < MAX_NETWORK_CONNECTIONS {
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
                } else if self.connections.len() < MAX_NETWORK_CONNECTIONS {
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

    fn sample(&mut self, now: Instant) -> NetworkRateSample {
        self.connections.retain(|_, counters| {
            now.saturating_duration_since(counters.last_seen) <= NETWORK_CONNECTION_TTL
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

#[derive(Clone, Default)]
pub struct PerformanceDemand(crate::demand::Demand);

impl PerformanceDemand {
    pub fn set(&self, enabled: bool) {
        self.0.set(enabled);
    }

    pub fn enabled(&self) -> bool {
        self.0.enabled()
    }

    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.0.subscribe()
    }

    pub(crate) fn acquire(&self) -> crate::demand::DemandLease {
        self.0.acquire()
    }
}

pub async fn supervise_system(
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
        let result = run_system_once(
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
        let Some(error) = result.err() else { continue };
        reporter.retrying(attempt, error);
        if !wait_for_retry(&mut shutdown, reconnect_backoff(attempt - 1)).await {
            break;
        }
    }
    reporter.stopped(attempt);
}

pub async fn supervise_graphics(
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
        let result = run_graphics_once(
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
        let Some(error) = result.err() else { continue };
        reporter.retrying(attempt, error);
        if !wait_for_retry(&mut shutdown, reconnect_backoff(attempt - 1)).await {
            break;
        }
    }
    reporter.stopped(attempt);
}

pub async fn supervise_network(
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
        let result = run_network_once(
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
        let Some(error) = result.err() else { continue };
        reporter.retrying(attempt, error);
        if !wait_for_retry(&mut shutdown, reconnect_backoff(attempt - 1)).await {
            break;
        }
    }
    reporter.stopped(attempt);
}

pub async fn supervise_energy(
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
        let result = run_energy_once(
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
        let Some(error) = result.err() else { continue };
        reporter.retrying(attempt, error);
        if !wait_for_retry(&mut shutdown, reconnect_backoff(attempt - 1)).await {
            break;
        }
    }
    reporter.stopped(attempt);
}

pub async fn supervise_app_activity(
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
        let result = run_app_activity_once(
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

async fn run_system_once(
    adapter: AdapterHandle,
    handshake: RsdHandshake,
    slot: PerformanceSlot,
    shutdown: &mut watch::Receiver<bool>,
    enabled: &mut watch::Receiver<bool>,
    reporter: &ServiceReporter,
    attempt: u32,
) -> Result<(), String> {
    let network_catalog = load_network_interface_catalog(adapter.clone(), handshake.clone());
    tokio::pin!(network_catalog);
    let mut network_catalog_pending = true;
    let mut remote = connect_remote(adapter, handshake).await?;
    let (process_attributes, system_attributes, hardware) =
        tokio::time::timeout(SETUP_TIMEOUT, async {
            let mut device_info = DeviceInfoClient::new(&mut remote).await?;
            let process = device_info.sysmon_process_attributes().await?;
            let system = device_info.sysmon_system_attributes().await?;
            let hardware = device_info.hardware_information().await?;
            Ok::<_, idevice::IdeviceError>((process, system, hardware))
        })
        .await
        .map_err(|_| "DVT sysmontap attribute query timed out".to_string())?
        .map_err(|error| format!("DVT sysmontap attribute query failed: {error:?}"))?;
    let cpu_count = cpu_count(&hardware).ok_or_else(|| {
        "DVT hardware information did not report a valid logical CPU count".to_string()
    })?;
    slot.update_hardware(&hardware);
    let process_schema = ProcessSchema::new(&process_attributes);
    let mut client = SysmontapClient::new(&mut remote)
        .await
        .map_err(|error| format!("DVT sysmontap channel failed: {error:?}"))?;
    let config = SysmontapConfig {
        interval_ms: SAMPLE_INTERVAL_MS,
        process_attributes,
        system_attributes,
    };
    tokio::time::timeout(SETUP_TIMEOUT, async {
        client.set_config(&config).await?;
        client.start().await
    })
    .await
    .map_err(|_| "DVT sysmontap setup timed out".to_string())?
    .map_err(|error| format!("DVT sysmontap setup failed: {error:?}"))?;
    reporter.ready(attempt);
    loop {
        tokio::select! {
            result = &mut network_catalog, if network_catalog_pending => {
                network_catalog_pending = false;
                match result {
                    Ok(network) => {
                        slot.update_network_interfaces(&network);
                        tracing::debug!(
                            count = slot.get().network_interfaces.len(),
                            "DVT network interface catalog updated"
                        );
                    }
                    Err(error) => tracing::debug!(%error, "DVT network interface catalog unavailable"),
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    let _ = client.stop().await;
                    return Ok(());
                }
            }
            changed = enabled.changed() => {
                if changed.is_err() || !*enabled.borrow() {
                    let _ = client.stop().await;
                    return Ok(());
                }
            }
            sample = client.next_sample() => match sample {
                Ok(sample) => slot.update_system(&sample, cpu_count, &process_schema),
                Err(error) => return Err(format!("DVT sysmontap stream failed: {error:?}")),
            }
        }
    }
}

async fn load_network_interface_catalog(
    mut adapter: AdapterHandle,
    mut handshake: RsdHandshake,
) -> Result<plist::Dictionary, String> {
    tokio::time::timeout(NETWORK_CATALOG_TIMEOUT, async {
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

async fn run_graphics_once(
    adapter: AdapterHandle,
    handshake: RsdHandshake,
    slot: PerformanceSlot,
    shutdown: &mut watch::Receiver<bool>,
    enabled: &mut watch::Receiver<bool>,
    reporter: &ServiceReporter,
    attempt: u32,
) -> Result<(), String> {
    let mut remote = connect_remote(adapter, handshake).await?;
    let mut client = GraphicsClient::new(&mut remote)
        .await
        .map_err(|error| format!("DVT graphics channel failed: {error:?}"))?;
    tokio::time::timeout(SETUP_TIMEOUT, client.start_sampling(1.0))
        .await
        .map_err(|_| "DVT graphics setup timed out".to_string())?
        .map_err(|error| format!("DVT graphics setup failed: {error:?}"))?;
    reporter.ready(attempt);
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    let _ = client.stop_sampling().await;
                    return Ok(());
                }
            }
            changed = enabled.changed() => {
                if changed.is_err() || !*enabled.borrow() {
                    let _ = client.stop_sampling().await;
                    return Ok(());
                }
            }
            sample = client.sample() => match sample {
                Ok(sample) => slot.update_graphics(&sample),
                Err(error) => return Err(format!("DVT graphics stream failed: {error:?}")),
            }
        }
    }
}

async fn run_network_once(
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
    let mut tick = tokio::time::interval(NETWORK_SAMPLE_INTERVAL);
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
            _ = tick.tick() => slot.update_network(accumulator.sample(Instant::now())),
        }
    }
}

async fn run_energy_once(
    adapter: AdapterHandle,
    handshake: RsdHandshake,
    slot: PerformanceSlot,
    shutdown: &mut watch::Receiver<bool>,
    enabled: &mut watch::Receiver<bool>,
    reporter: &ServiceReporter,
    attempt: u32,
) -> Result<(), String> {
    let mut remote = connect_remote(adapter, handshake).await?;
    let mut client = EnergyMonitorClient::new(&mut remote)
        .await
        .map_err(|error| format!("DVT energy monitor channel failed: {error:?}"))?;
    reporter.ready(attempt);
    let mut sampled_pids = Vec::new();
    let mut tick = tokio::time::interval(ENERGY_SAMPLE_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    tick.tick().await;
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    stop_energy_sampling(&mut client, &sampled_pids).await;
                    return Ok(());
                }
            }
            changed = enabled.changed() => {
                if changed.is_err() || !*enabled.borrow() {
                    stop_energy_sampling(&mut client, &sampled_pids).await;
                    slot.update_energy(Vec::new());
                    return Ok(());
                }
            }
            _ = tick.tick() => {
                let targets = slot.energy_targets();
                if targets != sampled_pids {
                    let removed = sampled_pids
                        .iter()
                        .copied()
                        .filter(|pid| targets.binary_search(pid).is_err())
                        .collect::<Vec<_>>();
                    let added = targets
                        .iter()
                        .copied()
                        .filter(|pid| sampled_pids.binary_search(pid).is_err())
                        .collect::<Vec<_>>();
                    stop_energy_sampling(&mut client, &removed).await;
                    if !added.is_empty() {
                        // Clear device-side state left by an interrupted prior session.
                        stop_energy_sampling(&mut client, &added).await;
                        tokio::time::timeout(
                            ENERGY_OPERATION_TIMEOUT,
                            client.start_sampling(&added),
                        )
                        .await
                        .map_err(|_| "DVT energy sampling setup timed out".to_string())?
                        .map_err(|error| format!("DVT energy sampling setup failed: {error:?}"))?;
                    }
                    sampled_pids = targets;
                    if sampled_pids.is_empty() {
                        slot.update_energy(Vec::new());
                    }
                }
                if !sampled_pids.is_empty() {
                    let bytes = tokio::time::timeout(
                        ENERGY_OPERATION_TIMEOUT,
                        client.sample_attributes(&sampled_pids),
                    )
                    .await
                    .map_err(|_| "DVT energy sample timed out".to_string())?
                    .map_err(|error| format!("DVT energy sample failed: {error:?}"))?;
                    let samples = EnergySample::from_bytes(&bytes)
                        .map_err(|error| format!("DVT energy sample decode failed: {error:?}"))?;
                    slot.update_energy(samples);
                }
            }
        }
    }
}

async fn run_app_activity_once(
    adapter: AdapterHandle,
    handshake: RsdHandshake,
    slot: PerformanceSlot,
    shutdown: &mut watch::Receiver<bool>,
    enabled: &mut watch::Receiver<bool>,
    reporter: &ServiceReporter,
    attempt: u32,
) -> Result<(), String> {
    let mut remote = connect_remote(adapter, handshake).await?;
    let mut client = NotificationsClient::new(&mut remote)
        .await
        .map_err(|error| format!("DVT app activity channel failed: {error:?}"))?;
    tokio::time::timeout(SETUP_TIMEOUT, client.start_notifications())
        .await
        .map_err(|_| "DVT app activity setup timed out".to_string())?
        .map_err(|error| format!("DVT app activity setup failed: {error:?}"))?;
    reporter.ready(attempt);
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    stop_app_activity(&mut client).await;
                    return Ok(());
                }
            }
            changed = enabled.changed() => {
                if changed.is_err() || !*enabled.borrow() {
                    stop_app_activity(&mut client).await;
                    return Ok(());
                }
            }
            notification = client.get_notification() => match notification {
                Ok(notification) => slot.publish_app_activity(notification),
                Err(error) => return Err(format!("DVT app activity stream failed: {error:?}")),
            }
        }
    }
}

async fn stop_app_activity<R: ReadWrite>(client: &mut NotificationsClient<'_, R>) {
    let _ = tokio::time::timeout(SETUP_TIMEOUT, client.stop_notifications()).await;
}

async fn stop_energy_sampling<R: ReadWrite>(client: &mut EnergyMonitorClient<'_, R>, pids: &[u32]) {
    if !pids.is_empty() {
        let _ = tokio::time::timeout(ENERGY_OPERATION_TIMEOUT, client.stop_sampling(pids)).await;
    }
}

async fn connect_remote(
    mut adapter: AdapterHandle,
    mut handshake: RsdHandshake,
) -> Result<RemoteServerClient<Box<dyn ReadWrite + 'static>>, String> {
    tokio::time::timeout(
        CONNECT_TIMEOUT,
        RemoteServerClient::connect_rsd(&mut adapter, &mut handshake),
    )
    .await
    .map_err(|_| "DVT performance connection timed out".to_string())?
    .map_err(|error| format!("DVT performance connection failed: {error:?}"))
}

fn numeric_value(value: &Value) -> Option<f64> {
    match value {
        Value::Real(value) => Some(*value),
        Value::Integer(value) => value
            .as_signed()
            .map(|value| value as f64)
            .or_else(|| value.as_unsigned().map(|value| value as f64)),
        _ => None,
    }
}

fn energy_score(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

fn cpu_count(hardware: &plist::Dictionary) -> Option<u32> {
    ["numberOfCpus", "numberOfPhysicalCpus"]
        .into_iter()
        .filter_map(|key| hardware.get(key))
        .filter_map(numeric_u32)
        .find(|count| (1..=256).contains(count))
}

fn physical_cpu_count(hardware: &plist::Dictionary) -> Option<u32> {
    hardware
        .get("numberOfPhysicalCpus")
        .and_then(numeric_u32)
        .filter(|count| (1..=256).contains(count))
}

fn physical_memory_bytes(hardware: &plist::Dictionary) -> Option<u64> {
    hardware
        .get("physicalMemory")
        .and_then(numeric_u64)
        .filter(|bytes| (16 * 1024 * 1024..=1024 * 1024 * 1024 * 1024).contains(bytes))
}

fn numeric_u32(value: &Value) -> Option<u32> {
    match value {
        Value::Integer(value) => value
            .as_unsigned()
            .and_then(|value| u32::try_from(value).ok())
            .or_else(|| {
                value
                    .as_signed()
                    .and_then(|value| u32::try_from(value).ok())
            }),
        _ => None,
    }
}

fn numeric_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Integer(value) => value.as_unsigned().or_else(|| {
            value
                .as_signed()
                .and_then(|value| u64::try_from(value).ok())
        }),
        _ => None,
    }
}

#[derive(Debug, Default)]
struct ProcessSchema {
    name: Option<usize>,
    pid: Option<usize>,
    cpu_usage: Option<usize>,
    physical_footprint: Option<usize>,
}

impl ProcessSchema {
    fn new(attributes: &[String]) -> Self {
        let index = |name: &str| attributes.iter().position(|attribute| attribute == name);
        Self {
            name: index("name"),
            pid: index("pid"),
            cpu_usage: index("cpuUsage"),
            physical_footprint: index("physFootprint"),
        }
    }
}

fn top_processes(
    processes: &plist::Dictionary,
    schema: &ProcessSchema,
    cpu_count: u32,
) -> Vec<ProcessPerformance> {
    let mut normalized = processes
        .iter()
        .filter_map(|(key, value)| normalize_process(key, value, schema, cpu_count))
        .collect::<Vec<_>>();
    let mut by_cpu = normalized.clone();
    by_cpu.sort_by(compare_process_cpu);
    normalized.sort_by(compare_process_memory);

    let mut selected = Vec::with_capacity(TOP_PROCESSES_PER_METRIC * 2);
    let mut selected_pids = HashSet::with_capacity(TOP_PROCESSES_PER_METRIC * 2);
    for process in by_cpu
        .into_iter()
        .take(TOP_PROCESSES_PER_METRIC)
        .chain(normalized.into_iter().take(TOP_PROCESSES_PER_METRIC))
    {
        if selected_pids.insert(process.pid) {
            selected.push(process);
        }
    }
    selected.sort_by(compare_process_cpu);
    selected
}

fn normalize_process(
    key: &str,
    value: &Value,
    schema: &ProcessSchema,
    cpu_count: u32,
) -> Option<ProcessPerformance> {
    let row = value.as_array()?;
    let pid = schema
        .pid
        .and_then(|index| row.get(index))
        .and_then(numeric_u32)
        .or_else(|| key.parse().ok())?;
    let name = schema
        .name
        .and_then(|index| row.get(index))
        .and_then(Value::as_string)
        .map(sanitize_process_name)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("pid {pid}"));
    let cpu_percent = schema
        .cpu_usage
        .and_then(|index| row.get(index))
        .and_then(numeric_value)
        .and_then(|value| normalize_aggregate_cpu_percent(value, cpu_count));
    let memory_bytes = schema
        .physical_footprint
        .and_then(|index| row.get(index))
        .and_then(numeric_u64);
    Some(ProcessPerformance {
        pid,
        name,
        cpu_percent,
        memory_bytes,
    })
}

fn sanitize_process_name(name: &str) -> String {
    name.chars()
        .filter(|character| !character.is_control())
        .take(256)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn compare_process_cpu(
    left: &ProcessPerformance,
    right: &ProcessPerformance,
) -> std::cmp::Ordering {
    right
        .cpu_percent
        .unwrap_or(-1.0)
        .total_cmp(&left.cpu_percent.unwrap_or(-1.0))
        .then_with(|| compare_process_memory(left, right))
        .then_with(|| left.pid.cmp(&right.pid))
}

fn compare_process_memory(
    left: &ProcessPerformance,
    right: &ProcessPerformance,
) -> std::cmp::Ordering {
    right
        .memory_bytes
        .unwrap_or(0)
        .cmp(&left.memory_bytes.unwrap_or(0))
        .then_with(|| left.pid.cmp(&right.pid))
}

fn normalize_aggregate_cpu_percent(value: f64, cpu_count: u32) -> Option<f64> {
    let normalized = value / f64::from(cpu_count);
    (value.is_finite() && cpu_count > 0 && (0.0..=100.0).contains(&normalized))
        .then_some(normalized)
}

async fn wait_until_enabled(
    enabled: &mut watch::Receiver<bool>,
    shutdown: &mut watch::Receiver<bool>,
    reporter: &ServiceReporter,
    attempt: u32,
) -> bool {
    while !*enabled.borrow() {
        reporter.stopped(attempt);
        tokio::select! {
            changed = enabled.changed() => {
                if changed.is_err() {
                    return false;
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return false;
                }
            }
        }
    }
    true
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn normalize_network_interfaces(
    network: &plist::Dictionary,
) -> (Vec<DeviceNetworkInterface>, bool) {
    let mut interfaces = network
        .iter()
        .take(MAX_RAW_NETWORK_INTERFACES)
        .filter_map(|(name, value)| {
            let name = normalize_network_interface_name(name)?;
            let description = value
                .as_string()
                .and_then(normalize_network_interface_description)?;
            Some(DeviceNetworkInterface {
                kind: classify_network_interface(&name, &description),
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
    let truncated =
        network.len() > MAX_RAW_NETWORK_INTERFACES || interfaces.len() > MAX_NETWORK_INTERFACES;
    interfaces.truncate(MAX_NETWORK_INTERFACES);
    (interfaces, truncated)
}

fn normalize_network_interface_name(value: &str) -> Option<String> {
    (!value.is_empty()
        && value.len() <= MAX_NETWORK_INTERFACE_NAME_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')))
    .then(|| value.to_string())
}

fn normalize_network_interface_description(value: &str) -> Option<String> {
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
            .take(MAX_NETWORK_INTERFACE_DESCRIPTION_CHARS)
            .collect()
    })
}

fn classify_network_interface(name: &str, description: &str) -> DeviceNetworkInterfaceKind {
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

#[cfg(test)]
mod tests {
    use super::*;
    use idevice::IdeviceService;
    use idevice::core_device_proxy::CoreDeviceProxy;
    use idevice::usbmuxd::{UsbmuxdAddr, UsbmuxdConnection};

    #[test]
    fn aggregate_cpu_load_is_normalized_by_device_cpu_count() {
        assert_eq!(normalize_aggregate_cpu_percent(240.0, 6), Some(40.0));
        assert_eq!(normalize_aggregate_cpu_percent(600.0, 6), Some(100.0));
        assert_eq!(normalize_aggregate_cpu_percent(601.0, 6), None);
        assert_eq!(normalize_aggregate_cpu_percent(42.0, 0), None);
        assert_eq!(normalize_aggregate_cpu_percent(f64::NAN, 6), None);
    }

    #[test]
    fn hardware_metrics_are_bounded_and_logical_count_falls_back() {
        let mut hardware = plist::Dictionary::new();
        hardware.insert("numberOfPhysicalCpus".into(), Value::Integer(6.into()));
        hardware.insert(
            "physicalMemory".into(),
            Value::Integer(6_442_450_944_u64.into()),
        );
        assert_eq!(cpu_count(&hardware), Some(6));
        assert_eq!(physical_cpu_count(&hardware), Some(6));
        assert_eq!(physical_memory_bytes(&hardware), Some(6_442_450_944));

        hardware.insert("numberOfCpus".into(), Value::Integer(8.into()));
        assert_eq!(cpu_count(&hardware), Some(8));

        hardware.insert("numberOfCpus".into(), Value::Integer(0.into()));
        assert_eq!(cpu_count(&hardware), Some(6));

        hardware.insert("numberOfPhysicalCpus".into(), Value::Integer(257.into()));
        hardware.insert("physicalMemory".into(), Value::Integer(u64::MAX.into()));
        assert_eq!(cpu_count(&hardware), None);
        assert_eq!(physical_cpu_count(&hardware), None);
        assert_eq!(physical_memory_bytes(&hardware), None);
    }

    #[test]
    fn hardware_metrics_are_retained_across_partial_samples() {
        let slot = PerformanceSlot::default();
        let hardware = plist::Dictionary::from_iter([
            (String::from("numberOfCpus"), Value::Integer(8.into())),
            (
                String::from("numberOfPhysicalCpus"),
                Value::Integer(6.into()),
            ),
            (
                String::from("physicalMemory"),
                Value::Integer(6_442_450_944_u64.into()),
            ),
        ]);
        slot.update_hardware(&hardware);
        slot.update_graphics(&idevice::dvt::graphics::GraphicsSample {
            timestamp: 1,
            fps: 60.0,
            alloc_system_memory: 10,
            in_use_system_memory: 8,
            in_use_system_memory_driver: 3,
            gpu_bundle_name: "Built-In".into(),
            recovery_count: 0,
        });
        let snapshot = slot.get();
        assert_eq!(snapshot.logical_cpu_count, Some(8));
        assert_eq!(snapshot.physical_cpu_count, Some(6));
        assert_eq!(snapshot.physical_memory_bytes, Some(6_442_450_944));
        let serialized = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(serialized["physical_cpu_count"], 6);
        assert_eq!(serialized["physical_memory_bytes"], 6_442_450_944_u64);
    }

    #[test]
    fn network_interface_catalog_is_classified_sanitized_and_bounded() {
        let mut network = plist::Dictionary::from_iter([
            (String::from("en0"), Value::String("  Wi-Fi  ".into())),
            (
                String::from("pdp_ip0"),
                Value::String("Cellular (pdp_ip0)".into()),
            ),
            (
                String::from("en2"),
                Value::String("Ethernet\nAdaptor (en2)".into()),
            ),
            (String::from("lo0"), Value::String("Loopback".into())),
            (String::from("utun0"), Value::String("Tunnel".into())),
            (String::from("bad/name"), Value::String("Private".into())),
            (String::from("empty"), Value::String("   ".into())),
            (String::from("control"), Value::String("bad\0value".into())),
            (String::from("numeric"), Value::Integer(1.into())),
        ]);
        let (interfaces, truncated) = normalize_network_interfaces(&network);
        assert!(!truncated);
        assert_eq!(interfaces.len(), 5);
        assert_eq!(interfaces[0].name, "en0");
        assert_eq!(interfaces[0].kind, DeviceNetworkInterfaceKind::Wifi);
        assert_eq!(interfaces[1].kind, DeviceNetworkInterfaceKind::Cellular);
        assert_eq!(interfaces[2].description, "Ethernet Adaptor (en2)");
        assert_eq!(interfaces[3].kind, DeviceNetworkInterfaceKind::Loopback);
        assert_eq!(interfaces[4].kind, DeviceNetworkInterfaceKind::Other);

        network.clear();
        for index in 0..=MAX_NETWORK_INTERFACES {
            network.insert(format!("utun{index}"), Value::String("Tunnel".into()));
        }
        let (interfaces, truncated) = normalize_network_interfaces(&network);
        assert_eq!(interfaces.len(), MAX_NETWORK_INTERFACES);
        assert!(truncated);
    }

    #[test]
    fn network_interface_catalog_is_retained_and_serialized_without_addresses() {
        let slot = PerformanceSlot::default();
        slot.update_network_interfaces(&plist::Dictionary::from_iter([(
            String::from("en0"),
            Value::String("Wi-Fi".into()),
        )]));
        slot.update_graphics(&idevice::dvt::graphics::GraphicsSample {
            timestamp: 1,
            fps: 60.0,
            alloc_system_memory: 10,
            in_use_system_memory: 8,
            in_use_system_memory_driver: 3,
            gpu_bundle_name: "Built-In".into(),
            recovery_count: 0,
        });
        let serialized = serde_json::to_value(slot.get()).unwrap();
        assert_eq!(serialized["network_interfaces"][0]["name"], "en0");
        assert_eq!(serialized["network_interfaces"][0]["kind"], "wifi");
        assert_eq!(serialized["network_interfaces"][0]["description"], "Wi-Fi");
        assert_eq!(serialized["network_interfaces_available"], true);
        assert_eq!(serialized["network_interfaces_truncated"], false);
        assert!(serialized.get("address").is_none());
    }

    #[test]
    fn partial_system_samples_preserve_the_latest_metrics() {
        let slot = PerformanceSlot::default();
        let mut cpu = plist::Dictionary::new();
        cpu.insert("CPU_TotalLoad".into(), Value::Real(240.0));
        let mut processes = plist::Dictionary::new();
        processes.insert("1".into(), Value::Array(Vec::new()));
        slot.update_system(
            &SysmontapSample {
                processes: Some(processes),
                system: None,
                system_cpu_usage: Some(cpu),
            },
            6,
            &ProcessSchema::default(),
        );
        slot.update_system(
            &SysmontapSample {
                processes: None,
                system: None,
                system_cpu_usage: None,
            },
            6,
            &ProcessSchema::default(),
        );

        let snapshot = slot.get();
        assert_eq!(snapshot.system_cpu_percent, Some(40.0));
        assert_eq!(snapshot.process_count, Some(1));
        assert_eq!(snapshot.logical_cpu_count, Some(6));
        assert_eq!(snapshot.top_processes.len(), 1);
        assert_eq!(snapshot.top_processes[0].pid, 1);
        assert_eq!(snapshot.top_processes[0].name, "pid 1");
    }

    #[test]
    fn process_metrics_follow_the_negotiated_attribute_order() {
        let attributes = vec![
            "physFootprint".into(),
            "name".into(),
            "cpuUsage".into(),
            "pid".into(),
        ];
        let schema = ProcessSchema::new(&attributes);
        let row = Value::Array(vec![
            Value::Integer(25_000_000.into()),
            Value::String("Example\nGame".into()),
            Value::Real(120.0),
            Value::Integer(42.into()),
        ]);
        let process = normalize_process("ignored", &row, &schema, 6).unwrap();
        assert_eq!(process.pid, 42);
        assert_eq!(process.name, "ExampleGame");
        assert_eq!(process.cpu_percent, Some(20.0));
        assert_eq!(process.memory_bytes, Some(25_000_000));
    }

    #[test]
    fn top_processes_include_cpu_and_memory_leaders() {
        let attributes = vec![
            "pid".into(),
            "name".into(),
            "cpuUsage".into(),
            "physFootprint".into(),
        ];
        let schema = ProcessSchema::new(&attributes);
        let mut processes = plist::Dictionary::new();
        for pid in 1..=12_u32 {
            processes.insert(
                pid.to_string(),
                Value::Array(vec![
                    Value::Integer(pid.into()),
                    Value::String(format!("cpu-{pid}")),
                    Value::Real(f64::from(100 - pid)),
                    Value::Integer(u64::from(pid * 1_000).into()),
                ]),
            );
        }
        processes.insert(
            "99".into(),
            Value::Array(vec![
                Value::Integer(99.into()),
                Value::String("memory-leader".into()),
                Value::Real(0.0),
                Value::Integer(9_000_000_000_u64.into()),
            ]),
        );

        let top = top_processes(&processes, &schema, 6);
        assert!(top.len() <= TOP_PROCESSES_PER_METRIC * 2);
        assert!(top.iter().any(|process| process.pid == 1));
        assert!(top.iter().any(|process| process.pid == 99));
        assert_eq!(top[0].pid, 1);
    }

    #[test]
    fn performance_slot_merges_independent_sources() {
        let slot = PerformanceSlot::default();
        slot.update_graphics(&idevice::dvt::graphics::GraphicsSample {
            timestamp: 1,
            fps: 59.5,
            alloc_system_memory: 10,
            in_use_system_memory: 8,
            in_use_system_memory_driver: 3,
            gpu_bundle_name: "Built-In".into(),
            recovery_count: 0,
        });
        let snapshot = slot.get();
        assert_eq!(snapshot.graphics_fps, Some(59.5));
        assert_eq!(snapshot.gpu_in_use_bytes, Some(8));
        assert!(snapshot.captured_at_ms > 0);
    }

    #[test]
    fn app_activity_is_sanitized_bounded_and_reset_with_the_session() {
        let slot = PerformanceSlot::default();
        for index in 0..=MAX_ACTIVITY_EVENTS {
            slot.publish_app_activity(NotificationInfo {
                notification_type: " application\nstate ".into(),
                mach_absolute_time: index as i64,
                exec_name: " Example\tGame ".into(),
                app_name: " Example  Game ".into(),
                pid: (index + 1) as u32,
                state_description: " foreground\nactive ".into(),
            });
        }

        let events = slot.app_activity();
        assert_eq!(events.len(), MAX_ACTIVITY_EVENTS);
        assert_eq!(events.first().unwrap().sequence, 2);
        assert_eq!(events.last().unwrap().sequence, 101);
        assert_eq!(
            events.last().unwrap().notification_type,
            "application state"
        );
        assert_eq!(
            events.last().unwrap().exec_name.as_deref(),
            Some("Example Game")
        );
        assert_eq!(
            events.last().unwrap().app_name.as_deref(),
            Some("Example Game")
        );
        assert_eq!(
            events.last().unwrap().state_description.as_deref(),
            Some("foreground active")
        );
        assert_eq!(events.last().unwrap().pid, Some(101));

        slot.reset();
        assert!(slot.app_activity().is_empty());
    }

    #[test]
    fn energy_sampling_tracks_ranked_processes_and_sanitizes_scores() {
        let slot = PerformanceSlot::default();
        {
            let mut snapshot = slot.0.sample.lock().unwrap();
            snapshot.top_processes = (0..20)
                .map(|index| ProcessPerformance {
                    pid: 100 - index,
                    name: format!("rank-{index}"),
                    cpu_percent: Some(f64::from(20 - index)),
                    memory_bytes: None,
                })
                .collect();
        }
        let targets = slot.energy_targets();
        assert_eq!(targets.len(), MAX_ENERGY_PROCESSES);
        assert!(targets.contains(&100));
        assert!(targets.contains(&85));
        assert!(!targets.contains(&84));

        slot.update_energy(vec![
            EnergySample {
                pid: 100,
                timestamp: 1,
                total_energy: 5.0,
                cpu_energy: f64::NAN,
                gpu_energy: -2.0,
                networking_energy: 1.5,
                display_energy: 0.5,
                location_energy: 0.0,
                appstate_energy: f64::INFINITY,
            },
            EnergySample {
                pid: 99,
                timestamp: 1,
                total_energy: 8.0,
                cpu_energy: 3.0,
                gpu_energy: 2.0,
                networking_energy: 1.0,
                display_energy: 1.0,
                location_energy: 0.5,
                appstate_energy: 0.5,
            },
            EnergySample {
                pid: 777,
                timestamp: 1,
                total_energy: 99.0,
                cpu_energy: 99.0,
                gpu_energy: 0.0,
                networking_energy: 0.0,
                display_energy: 0.0,
                location_energy: 0.0,
                appstate_energy: 0.0,
            },
        ]);
        let snapshot = slot.get();
        assert_eq!(snapshot.energy_processes.len(), 2);
        assert_eq!(snapshot.energy_processes[0].pid, 99);
        assert_eq!(snapshot.energy_processes[0].name, "rank-1");
        assert_eq!(snapshot.energy_processes[1].cpu_score, 0.0);
        assert_eq!(snapshot.energy_processes[1].gpu_score, 0.0);
        assert_eq!(snapshot.energy_processes[1].app_state_score, 0.0);
    }

    #[test]
    fn network_rates_use_connection_deltas_and_expire_stale_entries() {
        use idevice::dvt::network_monitor::{ConnectionDetectionEvent, ConnectionUpdateEvent};

        let started = Instant::now();
        let mut accumulator = NetworkAccumulator::new(started);
        accumulator.observe(
            NetworkEvent::ConnectionDetection(ConnectionDetectionEvent {
                local_address: None,
                remote_address: None,
                interface_index: 1,
                pid: 42,
                recv_buffer_size: 0,
                recv_buffer_used: 0,
                serial_number: 7,
                kind: 0,
            }),
            started,
        );
        accumulator.observe(
            NetworkEvent::ConnectionUpdate(ConnectionUpdateEvent {
                rx_packets: 1,
                rx_bytes: 1_000,
                tx_packets: 1,
                tx_bytes: 200,
                rx_dups: 0,
                rx_ooo: 0,
                tx_retx: 0,
                min_rtt: 0,
                avg_rtt: 0,
                connection_serial: 7,
                time: 0,
            }),
            started + Duration::from_millis(500),
        );
        let first = accumulator.sample(started + Duration::from_secs(1));
        assert_eq!(first.rx_bytes_per_second, 0.0);
        assert_eq!(first.tx_bytes_per_second, 0.0);
        assert_eq!(first.recent_connections, 1);

        accumulator.observe(
            NetworkEvent::ConnectionUpdate(ConnectionUpdateEvent {
                rx_packets: 2,
                rx_bytes: 1_500,
                tx_packets: 2,
                tx_bytes: 500,
                rx_dups: 0,
                rx_ooo: 0,
                tx_retx: 0,
                min_rtt: 0,
                avg_rtt: 0,
                connection_serial: 7,
                time: 1,
            }),
            started + Duration::from_millis(1_500),
        );
        let second = accumulator.sample(started + Duration::from_secs(2));
        assert_eq!(second.rx_bytes_per_second, 500.0);
        assert_eq!(second.tx_bytes_per_second, 300.0);
        assert_eq!(
            accumulator
                .sample(started + NETWORK_CONNECTION_TTL + Duration::from_secs(2))
                .recent_connections,
            0
        );
    }

    #[tokio::test]
    #[ignore = "requires a connected physical device"]
    async fn inspects_sysmontap_process_schema_from_hardware() {
        let mut usbmuxd = UsbmuxdConnection::default().await.unwrap();
        let device = usbmuxd
            .get_devices()
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("no connected device");
        let provider =
            device.to_provider(UsbmuxdAddr::default(), "devicehub-mask-performance-test");
        let proxy = CoreDeviceProxy::connect(&provider).await.unwrap();
        let rsd_port = proxy.tunnel_info().server_rsd_port;
        let adapter = proxy.create_software_tunnel().unwrap();
        let mut adapter = adapter.to_async_handle();
        let stream = adapter.connect(rsd_port).await.unwrap();
        let mut handshake = RsdHandshake::new(stream).await.unwrap();
        let mut remote = RemoteServerClient::connect_rsd(&mut adapter, &mut handshake)
            .await
            .unwrap();
        let (process_attributes, system_attributes, logical_cpu_count) = {
            let mut info = DeviceInfoClient::new(&mut remote).await.unwrap();
            (
                info.sysmon_process_attributes().await.unwrap(),
                info.sysmon_system_attributes().await.unwrap(),
                cpu_count(&info.hardware_information().await.unwrap()).unwrap(),
            )
        };
        let process_schema = ProcessSchema::new(&process_attributes);
        assert!(process_schema.name.is_some());
        assert!(process_schema.pid.is_some());
        assert!(process_schema.cpu_usage.is_some());
        assert!(process_schema.physical_footprint.is_some());
        let mut client = SysmontapClient::new(&mut remote).await.unwrap();
        client
            .set_config(&SysmontapConfig {
                interval_ms: SAMPLE_INTERVAL_MS,
                process_attributes: process_attributes.clone(),
                system_attributes,
            })
            .await
            .unwrap();
        client.start().await.unwrap();
        let processes = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Some(processes) = client.next_sample().await.unwrap().processes {
                    break processes;
                }
            }
        })
        .await
        .expect("timed out waiting for process sample");
        let top = top_processes(&processes, &process_schema, logical_cpu_count);
        assert!(!top.is_empty());
        assert!(top.iter().all(|process| !process.name.is_empty()));
        assert!(
            top.iter()
                .filter_map(|process| process.cpu_percent)
                .all(|cpu| (0.0..=100.0).contains(&cpu))
        );
        assert!(top.iter().any(|process| process.memory_bytes.is_some()));
        println!("normalized top processes: {:#?}", &top[..top.len().min(5)]);
        client.stop().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires a connected physical device"]
    async fn receives_network_monitor_event_from_hardware() {
        let mut usbmuxd = UsbmuxdConnection::default().await.unwrap();
        let device = usbmuxd
            .get_devices()
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("no connected device");
        let provider = device.to_provider(
            UsbmuxdAddr::default(),
            "devicehub-mask-network-monitor-test",
        );
        let proxy = CoreDeviceProxy::connect(&provider).await.unwrap();
        let rsd_port = proxy.tunnel_info().server_rsd_port;
        let adapter = proxy.create_software_tunnel().unwrap();
        let mut adapter = adapter.to_async_handle();
        let stream = adapter.connect(rsd_port).await.unwrap();
        let mut handshake = RsdHandshake::new(stream).await.unwrap();
        let mut remote = RemoteServerClient::connect_rsd(&mut adapter, &mut handshake)
            .await
            .unwrap();
        let mut client = NetworkMonitorClient::new(&mut remote).await.unwrap();
        client.start_monitoring().await.unwrap();
        let (serial, rx_delta, tx_delta, detections, updates) =
            tokio::time::timeout(Duration::from_secs(20), async {
                let mut detections = 0_u32;
                let mut updates = 0_u32;
                let mut baselines = HashMap::<u64, (u64, u64)>::new();
                loop {
                    let event = client.next_event().await.unwrap();
                    match event {
                        NetworkEvent::ConnectionDetection(_) => detections += 1,
                        NetworkEvent::ConnectionUpdate(update) => {
                            updates += 1;
                            if let Some((previous_rx, previous_tx)) = baselines.insert(
                                update.connection_serial,
                                (update.rx_bytes, update.tx_bytes),
                            ) {
                                let rx_delta = update.rx_bytes.saturating_sub(previous_rx);
                                let tx_delta = update.tx_bytes.saturating_sub(previous_tx);
                                if rx_delta > 0 || tx_delta > 0 {
                                    break (
                                        update.connection_serial,
                                        rx_delta,
                                        tx_delta,
                                        detections,
                                        updates,
                                    );
                                }
                            }
                        }
                        NetworkEvent::InterfaceDetection(_) | NetworkEvent::Unknown(_) => {}
                    }
                }
            })
            .await
            .expect("timed out waiting for a positive network counter delta");
        println!(
            "received network delta for serial {serial} after {detections} detections and {updates} updates: rx={rx_delta} tx={tx_delta}"
        );
        client.stop_monitoring().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires a connected physical device"]
    async fn receives_energy_sample_from_hardware() {
        let mut usbmuxd = UsbmuxdConnection::default().await.unwrap();
        let device = usbmuxd
            .get_devices()
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("no connected device");
        let provider =
            device.to_provider(UsbmuxdAddr::default(), "devicehub-mask-energy-monitor-test");
        let proxy = CoreDeviceProxy::connect(&provider).await.unwrap();
        let rsd_port = proxy.tunnel_info().server_rsd_port;
        let adapter = proxy.create_software_tunnel().unwrap();
        let mut adapter = adapter.to_async_handle();
        let stream = adapter.connect(rsd_port).await.unwrap();
        let mut handshake = RsdHandshake::new(stream).await.unwrap();
        let mut energy_adapter = adapter.clone();
        let mut energy_handshake = handshake.clone();
        let mut remote = RemoteServerClient::connect_rsd(&mut adapter, &mut handshake)
            .await
            .unwrap();
        let process = {
            let mut info = DeviceInfoClient::new(&mut remote).await.unwrap();
            let processes = info.running_processes().await.unwrap();
            processes
                .iter()
                .find(|process| process.is_application && process.pid > 0)
                .or_else(|| processes.iter().find(|process| process.pid > 1))
                .cloned()
                .expect("no running process found")
        };
        drop(remote);

        let mut energy_remote =
            RemoteServerClient::connect_rsd(&mut energy_adapter, &mut energy_handshake)
                .await
                .unwrap();
        let mut client = EnergyMonitorClient::new(&mut energy_remote).await.unwrap();
        client.start_sampling(&[process.pid]).await.unwrap();
        let observations = tokio::time::timeout(Duration::from_secs(10), async {
            let mut observations = Vec::new();
            while observations.len() < 3 {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let bytes = client.sample_attributes(&[process.pid]).await.unwrap();
                let samples = EnergySample::from_bytes(&bytes).unwrap();
                if let Some(sample) = samples.into_iter().find(|sample| {
                    sample.pid == process.pid && (sample.timestamp > 0 || sample.total_energy > 0.0)
                }) && observations
                    .last()
                    .is_none_or(|previous: &EnergySample| sample.timestamp > previous.timestamp)
                {
                    observations.push(sample);
                }
            }
            observations
        })
        .await
        .expect("energy sample timestamp did not advance");
        assert!(observations.iter().all(|sample| sample.pid == process.pid));
        assert!(
            observations
                .iter()
                .all(|sample| sample.total_energy.is_finite())
        );
        assert!(
            observations
                .windows(2)
                .all(|samples| samples[1].timestamp > samples[0].timestamp)
        );
        println!("received energy samples for {process:?}: {observations:#?}");
        client.stop_sampling(&[process.pid]).await.unwrap();
    }
}
