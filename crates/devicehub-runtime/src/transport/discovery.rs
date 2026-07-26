//! Stateful USB and Wi-Fi device discovery for the runtime session manager.
//!
//! A single owner keeps device names, pairing refresh state and netmuxd fallback
//! coherent across scans. The session manager consumes snapshots and endpoints;
//! it does not decide which mux daemon to probe or mutate pairing caches.

use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::time::Duration;

use idevice::{
    IdeviceService,
    lockdown::LockdownClient,
    pairing_file::PairingFile,
    provider::IdeviceProvider,
    usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdDevice},
};

use devicehub_core::{
    ConnKind, DeviceInfo, DevicePairingState, device_id_fingerprint, device_selector,
};

use super::{
    CoreTunnelConfig, PairingStore, SessionEndpoint, UsbmuxdEndpoint, WifiDiscovery,
    connection_kind, connection_kind_priority, connection_priority, uses_usbmuxd_core_proxy,
    wifi_provider,
};
use crate::session::PairingCredentialStore;

const NAME_TIMEOUT: Duration = Duration::from_secs(2);

pub type MuxSidecarFuture<'a> = Pin<Box<dyn Future<Output = Option<SocketAddr>> + Send + 'a>>;

/// Optional host process that exposes an additional usbmuxd-compatible endpoint.
pub trait MuxSidecar: Send + 'static {
    fn is_forced(&self) -> bool;
    fn ensure_ready(&mut self) -> MuxSidecarFuture<'_>;
}

pub(crate) struct DeviceDiscovery<Sidecar, Store> {
    /// Names are stable enough to cache for the manager lifetime. Explicit
    /// refresh and trust changes invalidate them through [`Self::invalidate`].
    names: HashMap<String, String>,
    sidecar: Sidecar,
    wifi_store: Option<Store>,
    wifi: Option<WifiDiscovery<Store>>,
    prefer_netmuxd: bool,
    tunnel: CoreTunnelConfig,
}

impl<Sidecar, Store> DeviceDiscovery<Sidecar, Store>
where
    Sidecar: MuxSidecar,
    Store: PairingStore,
{
    pub(crate) fn new(
        sidecar: Sidecar,
        wifi_store: Option<Store>,
        tunnel: CoreTunnelConfig,
    ) -> Self {
        let prefer_netmuxd = sidecar.is_forced();
        let wifi = wifi_store.clone().and_then(start_wifi_discovery);
        Self {
            names: HashMap::new(),
            sidecar,
            wifi_store,
            wifi,
            prefer_netmuxd,
            tunnel,
        }
    }

    pub(crate) fn invalidate(&mut self) {
        self.names.clear();
    }

    pub(crate) fn requires_pairing(&self) -> bool {
        self.wifi
            .as_ref()
            .is_some_and(WifiDiscovery::requires_pairing)
    }

    /// Remove credentials from the active discovery backend when available, or
    /// from the same confined on-disk cache when Wi-Fi discovery is unavailable.
    fn remove_credentials(&mut self, udid: &str) -> Result<(), String> {
        let discovery_result = match self.wifi.as_mut() {
            Some(discovery) => discovery.remove_pairing(udid),
            None => match self.wifi_store.as_ref() {
                Some(store) => store.remove_lockdown_pairing(udid),
                None => Ok(()),
            },
        };
        let remote_result = self.tunnel.remove_remote_pairing_credentials(udid);
        match (discovery_result, remote_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(discovery), Ok(())) => Err(discovery),
            (Ok(()), Err(remote)) => Err(remote),
            (Err(discovery), Err(remote)) => Err(format!("{discovery}; {remote}")),
        }
    }

    /// Produce one consistent picker snapshot and its exact connection targets.
    /// Failures are best-effort and yield fewer devices rather than terminating
    /// the manager, allowing later idle scans to recover automatically.
    pub(crate) async fn refresh(&mut self) -> (Vec<DeviceInfo>, HashMap<String, SessionEndpoint>) {
        let netmuxd_addr = if self.prefer_netmuxd || self.wifi.is_none() {
            self.sidecar.ensure_ready().await
        } else {
            None
        };
        if self.wifi.is_none() {
            self.wifi = self.wifi_store.clone().and_then(start_wifi_discovery);
        }
        let system_addr = self.tunnel.system_usbmuxd_address().map_err(|error| {
            tracing::warn!(%error, "invalid usbmuxd address; USB discovery disabled");
        });
        let mut candidates = Vec::new();
        if let Some(address) = netmuxd_addr {
            candidates.push((UsbmuxdAddr::TcpSocket(address), true));
        }
        if let Ok(address) = system_addr {
            candidates.push((address, false));
        }

        // The configured preference controls probe order, but a failed daemon
        // never hides devices available from the fallback mux implementation.
        let mut selected_mux = None;
        for (address, is_netmuxd) in candidates {
            match address.connect(0).await {
                Ok(mut connection) => match connection.get_devices().await {
                    Ok(devices) => {
                        selected_mux = Some((address, connection, devices, is_netmuxd));
                        break;
                    }
                    Err(error) => tracing::warn!(
                        ?error,
                        is_netmuxd,
                        "unable to list usbmuxd devices; trying transport fallback"
                    ),
                },
                Err(error) => tracing::warn!(
                    ?error,
                    is_netmuxd,
                    "unable to connect to usbmuxd; trying transport fallback"
                ),
            }
        }
        let (address, mut usbmuxd, devices, using_netmuxd) = match selected_mux {
            Some(selected) => (Some(selected.0), Some(selected.1), selected.2, selected.3),
            None => (None, None, Vec::new(), false),
        };

        let mut pairing_states = HashMap::new();
        if let Some(usbmuxd) = usbmuxd.as_mut() {
            for device in devices
                .iter()
                .filter(|device| matches!(device.connection_type, Connection::Usb))
            {
                match usbmuxd.get_pair_record(&device.udid).await {
                    Ok(pairing_file) => {
                        pairing_states.insert(device.udid.clone(), DevicePairingState::Paired);
                        self.refresh_wifi_pairing(device, pairing_file);
                    }
                    Err(error) => {
                        pairing_states.insert(device.udid.clone(), DevicePairingState::Unpaired);
                        tracing::debug!(
                            device_id = %device_id_fingerprint(&device.udid),
                            ?error,
                            "USB pairing record unavailable"
                        );
                    }
                }
            }
        }

        // Network entries exposed by usbmuxd/netmuxd are Lockdown transports,
        // not USB CoreDevice proxies. Wi-Fi control must use RemotePairing below.
        let mut selected = Vec::with_capacity(devices.len());
        for device in devices {
            if uses_usbmuxd_core_proxy(&device.connection_type) {
                selected.push(device);
            } else if let Connection::Unknown(connection_type) = &device.connection_type {
                tracing::warn!(
                    device_id = %device_id_fingerprint(&device.udid),
                    %connection_type,
                    "ignoring usbmuxd device with an ambiguous transport"
                );
            }
        }
        selected.sort_by(|left, right| {
            left.udid.cmp(&right.udid).then_with(|| {
                connection_priority(&left.connection_type)
                    .cmp(&connection_priority(&right.connection_type))
            })
        });

        let mut output = Vec::with_capacity(selected.len());
        let mut endpoints = HashMap::new();
        for device in selected {
            let connection = connection_kind(&device.connection_type);
            let id = device_selector(&device.udid, connection);
            let name = match self.names.get(&device.udid) {
                Some(name) => name.clone(),
                None => {
                    let name = match &address {
                        Some(address) => fetch_device_name(&device, address).await,
                        None => None,
                    }
                    .unwrap_or_else(|| device.udid.clone());
                    self.names.insert(device.udid.clone(), name.clone());
                    name
                }
            };
            output.push(DeviceInfo {
                id: id.clone(),
                udid: device.udid.clone(),
                name,
                connection,
                pairing: pairing_states
                    .get(&device.udid)
                    .copied()
                    .unwrap_or_default(),
            });
            if let Some(address) = address.clone() {
                endpoints
                    .entry(id)
                    .or_insert(SessionEndpoint::Usbmuxd(Box::new(UsbmuxdEndpoint {
                        device,
                        address,
                    })));
            }
        }

        if let Some(discovery) = self.wifi.as_mut() {
            for endpoint in discovery.refresh() {
                let id = device_selector(&endpoint.udid, ConnKind::Network);
                if endpoints.contains_key(&id) {
                    continue;
                }
                let provider = wifi_provider(&endpoint);
                let name = match self.names.get(&endpoint.udid) {
                    Some(name) => name.clone(),
                    None => {
                        let name = fetch_device_name_from_provider(&provider)
                            .await
                            .unwrap_or_else(|| endpoint.udid.clone());
                        self.names.insert(endpoint.udid.clone(), name.clone());
                        name
                    }
                };
                output.push(DeviceInfo {
                    id: id.clone(),
                    udid: endpoint.udid.clone(),
                    name,
                    connection: ConnKind::Network,
                    pairing: DevicePairingState::NotApplicable,
                });
                endpoints.insert(id, SessionEndpoint::Wifi(Box::new(endpoint)));
            }
        }

        let usb_count = output
            .iter()
            .filter(|device| device.connection == ConnKind::Usb)
            .count();
        let wifi_count = output
            .iter()
            .filter(|device| device.connection == ConnKind::Network)
            .count();
        tracing::debug!(
            provider = if using_netmuxd {
                "netmuxd"
            } else {
                "system-usbmuxd"
            },
            usb_count,
            wifi_count,
            "device discovery refresh completed"
        );
        output.sort_by(|left, right| {
            left.udid.cmp(&right.udid).then_with(|| {
                connection_kind_priority(left.connection)
                    .cmp(&connection_kind_priority(right.connection))
            })
        });
        (output, endpoints)
    }

    fn refresh_wifi_pairing(&mut self, device: &UsbmuxdDevice, pairing_file: PairingFile) {
        let Some(discovery) = self.wifi.as_mut() else {
            return;
        };
        if !discovery.pairing_needs_refresh(&device.udid) {
            return;
        }
        if let Err(error) = discovery.cache_pairing(&device.udid, pairing_file) {
            tracing::warn!(
                device_id = %device_id_fingerprint(&device.udid),
                %error,
                "unable to cache pairing record for Wi-Fi discovery"
            );
        } else {
            discovery.mark_pairing_refreshed(&device.udid);
        }
    }
}

impl<Sidecar, Store> PairingCredentialStore for DeviceDiscovery<Sidecar, Store>
where
    Sidecar: MuxSidecar,
    Store: PairingStore,
{
    fn remove_cached_pairing(&mut self, udid: &str) -> Result<(), String> {
        self.remove_credentials(udid)
    }
}

fn start_wifi_discovery<Store>(store: Store) -> Option<WifiDiscovery<Store>>
where
    Store: PairingStore,
{
    match WifiDiscovery::start(store) {
        Ok(discovery) => Some(discovery),
        Err(error) => {
            tracing::warn!(%error, "Wi-Fi discovery unavailable; continuing with usbmuxd");
            None
        }
    }
}

async fn fetch_device_name(device: &UsbmuxdDevice, address: &UsbmuxdAddr) -> Option<String> {
    let provider = device.to_provider(address.clone(), "devicehub_rs");
    fetch_device_name_from_provider(&provider).await
}

async fn fetch_device_name_from_provider(provider: &dyn IdeviceProvider) -> Option<String> {
    let lookup = async {
        let mut lockdown = LockdownClient::connect(provider).await.ok()?;
        let value = lockdown.get_value(Some("DeviceName"), None).await.ok()?;
        value.as_string().map(str::to_owned)
    };
    tokio::time::timeout(NAME_TIMEOUT, lookup)
        .await
        .ok()
        .flatten()
}
