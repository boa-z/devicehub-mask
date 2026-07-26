//! Paired iOS device discovery over Bonjour.
//!
//! Apple usbmuxd does not consistently publish iOS 26.4+ network devices. We
//! cache the pairing record while USB is available, authenticate Bonjour TXT
//! records with its HostID, then connect to lockdownd services directly.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::time::{Duration, Instant};

use devicehub_core::device_id_fingerprint;
use idevice::pairing_file::PairingFile;
use mdns_sd::{Receiver, ResolvedService, ScopedIp, ServiceDaemon, ServiceEvent};

use super::WifiEndpoint;

const SERVICE_TYPE: &str = "_apple-mobdev2._tcp.local.";
const REMOTE_PAIRING_SERVICE_TYPE: &str = "_remotepairing._tcp.local.";
const SERVICE_REMOVAL_GRACE: Duration = Duration::from_secs(10);

/// Serialized pairing record loaded from host-confined storage.
pub struct StoredWifiPairingRecord {
    pub udid: String,
    pub bytes: Vec<u8>,
}

/// Host persistence for pairing records used to authenticate Bonjour devices.
pub trait WifiPairingStore: Clone + Send + Sync + 'static {
    fn load(&self) -> Result<Vec<StoredWifiPairingRecord>, String>;
    fn save(&self, udid: &str, bytes: &[u8]) -> Result<(), String>;
    fn remove(&self, udid: &str) -> Result<(), String>;
}

pub(crate) struct WifiDiscovery<Store> {
    store: Store,
    pairing_files: HashMap<String, PairingFile>,
    refreshed_pairings: HashSet<String>,
    services: HashMap<String, ResolvedService>,
    remote_pairing_services: HashMap<String, ResolvedService>,
    pending_service_removals: HashMap<String, Instant>,
    pending_remote_pairing_removals: HashMap<String, Instant>,
    announced: HashSet<String>,
    receiver: Receiver<ServiceEvent>,
    remote_pairing_receiver: Receiver<ServiceEvent>,
    _daemon: ServiceDaemon,
}

impl<Store> WifiDiscovery<Store>
where
    Store: WifiPairingStore,
{
    pub(crate) fn start(store: Store) -> Result<Self, String> {
        let pairing_files = load_pairing_files(&store);
        let daemon = ServiceDaemon::new()
            .map_err(|error| format!("cannot initialize Bonjour discovery: {error}"))?;
        let receiver = daemon
            .browse(SERVICE_TYPE)
            .map_err(|error| format!("cannot browse for iOS devices: {error}"))?;
        let remote_pairing_receiver = daemon
            .browse(REMOTE_PAIRING_SERVICE_TYPE)
            .map_err(|error| format!("cannot browse for iOS remote pairing: {error}"))?;
        tracing::info!(
            cached_pairings = pairing_files.len(),
            service_type = SERVICE_TYPE,
            "Wi-Fi device discovery started"
        );
        Ok(Self {
            store,
            pairing_files,
            refreshed_pairings: HashSet::new(),
            services: HashMap::new(),
            remote_pairing_services: HashMap::new(),
            pending_service_removals: HashMap::new(),
            pending_remote_pairing_removals: HashMap::new(),
            announced: HashSet::new(),
            receiver,
            remote_pairing_receiver,
            _daemon: daemon,
        })
    }

    pub(crate) fn cache_pairing(
        &mut self,
        udid: &str,
        pairing_file: PairingFile,
    ) -> Result<(), String> {
        let bytes = pairing_file
            .clone()
            .serialize()
            .map_err(|error| format!("cannot serialize pairing record: {error:?}"))?;
        self.store.save(udid, &bytes)?;
        let inserted = self
            .pairing_files
            .insert(udid.to_owned(), pairing_file)
            .is_none();
        if inserted {
            tracing::info!(
                device_id = %device_id_fingerprint(udid),
                "cached pairing record for authenticated Wi-Fi discovery"
            );
        }
        Ok(())
    }

    pub(crate) fn pairing_needs_refresh(&self, udid: &str) -> bool {
        !self.refreshed_pairings.contains(udid)
    }

    pub(crate) fn mark_pairing_refreshed(&mut self, udid: &str) {
        self.refreshed_pairings.insert(udid.to_owned());
    }

    pub(crate) fn remove_pairing(&mut self, udid: &str) -> Result<(), String> {
        self.pairing_files.remove(udid);
        self.refreshed_pairings.remove(udid);
        self.store.remove(udid)?;
        tracing::info!(
            device_id = %device_id_fingerprint(udid),
            "removed cached pairing record for Wi-Fi discovery"
        );
        Ok(())
    }

    pub(crate) fn refresh(&mut self) -> Vec<WifiEndpoint> {
        self.expire_removed_services();
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                ServiceEvent::ServiceResolved(service) => {
                    self.pending_service_removals.remove(&service.fullname);
                    self.services.insert(service.fullname.clone(), *service);
                }
                ServiceEvent::ServiceRemoved(_, fullname) => {
                    self.pending_service_removals
                        .insert(fullname, Instant::now() + SERVICE_REMOVAL_GRACE);
                }
                _ => {}
            }
        }
        while let Ok(event) = self.remote_pairing_receiver.try_recv() {
            match event {
                ServiceEvent::ServiceResolved(service) => {
                    self.pending_remote_pairing_removals
                        .remove(&service.fullname);
                    self.remote_pairing_services
                        .insert(service.fullname.clone(), *service);
                }
                ServiceEvent::ServiceRemoved(_, fullname) => {
                    self.pending_remote_pairing_removals
                        .insert(fullname, Instant::now() + SERVICE_REMOVAL_GRACE);
                }
                _ => {}
            }
        }

        let mut by_udid = HashMap::<String, (WifiEndpoint, bool)>::new();
        for (fullname, service) in &self.services {
            let Some((endpoint, ipv4)) =
                resolve_service(service, &self.remote_pairing_services, &self.pairing_files)
            else {
                continue;
            };
            if self.announced.insert(fullname.clone()) {
                tracing::info!(
                    device_id = %device_id_fingerprint(&endpoint.udid),
                    address_family = if ipv4 { "ipv4" } else { "ipv6" },
                    "authenticated Wi-Fi device discovered"
                );
            }
            match by_udid.entry(endpoint.udid.clone()) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert((endpoint, ipv4));
                }
                std::collections::hash_map::Entry::Occupied(mut entry)
                    if ipv4 && !entry.get().1 =>
                {
                    entry.insert((endpoint, ipv4));
                }
                _ => {}
            }
        }
        by_udid
            .into_values()
            .map(|(endpoint, _)| endpoint)
            .collect()
    }

    fn expire_removed_services(&mut self) {
        let now = Instant::now();
        let expired = self
            .pending_service_removals
            .iter()
            .filter_map(|(fullname, deadline)| (*deadline <= now).then_some(fullname.clone()))
            .collect::<Vec<_>>();
        for fullname in expired {
            self.pending_service_removals.remove(&fullname);
            self.services.remove(&fullname);
            self.announced.remove(&fullname);
        }

        let expired = self
            .pending_remote_pairing_removals
            .iter()
            .filter_map(|(fullname, deadline)| (*deadline <= now).then_some(fullname.clone()))
            .collect::<Vec<_>>();
        for fullname in expired {
            self.pending_remote_pairing_removals.remove(&fullname);
            self.remote_pairing_services.remove(&fullname);
        }
    }

    pub(crate) fn requires_pairing(&self) -> bool {
        self.services
            .values()
            .any(|service| !service_matches_pairing(service, &self.pairing_files))
    }
}

fn service_matches_pairing(
    service: &ResolvedService,
    pairing_files: &HashMap<String, PairingFile>,
) -> bool {
    let Some(identifier) = service
        .get_property_val("identifier")
        .and_then(|value| value)
    else {
        return false;
    };
    let auth_tags = service
        .get_properties()
        .iter()
        .filter(|property| property.key() == "authTag" || property.key().starts_with("authTag#"))
        .filter_map(|property| property.val())
        .collect::<Vec<_>>();
    pairing_files.values().any(|pairing_file| {
        idevice::mdns::txt_record_matches(pairing_file.host_id.as_bytes(), identifier, &auth_tags)
    })
}

fn resolve_service(
    service: &ResolvedService,
    remote_pairing_services: &HashMap<String, ResolvedService>,
    pairing_files: &HashMap<String, PairingFile>,
) -> Option<(WifiEndpoint, bool)> {
    let identifier = service
        .get_property_val("identifier")
        .and_then(|value| value)?;
    let auth_tags = service
        .get_properties()
        .iter()
        .filter(|property| property.key() == "authTag" || property.key().starts_with("authTag#"))
        .filter_map(|property| property.val())
        .collect::<Vec<_>>();
    if auth_tags.is_empty() {
        return None;
    }
    let (udid, pairing_file) = pairing_files.iter().find(|(_, pairing_file)| {
        idevice::mdns::txt_record_matches(pairing_file.host_id.as_bytes(), identifier, &auth_tags)
    })?;
    let (address, scope_id, ipv4) = preferred_address(service)?;
    let remote_pairing = remote_pairing_services
        .values()
        .find(|candidate| service_addresses_overlap(service, candidate))?;
    let (remote_pairing_address, remote_pairing_scope_id, _) = preferred_address(remote_pairing)?;
    Some((
        WifiEndpoint {
            udid: udid.clone(),
            address,
            scope_id,
            remote_pairing_address,
            remote_pairing_scope_id,
            remote_pairing_port: remote_pairing.port,
            pairing_file: pairing_file.clone(),
        },
        ipv4,
    ))
}

fn service_addresses_overlap(left: &ResolvedService, right: &ResolvedService) -> bool {
    left.addresses.iter().any(|left_address| {
        right
            .addresses
            .iter()
            .any(|right_address| left_address.to_ip_addr() == right_address.to_ip_addr())
    })
}

fn preferred_address(service: &ResolvedService) -> Option<(IpAddr, Option<u32>, bool)> {
    // A service may be advertised over both Wi-Fi and the USB network interface.
    // Connecting to its 169.254/16 USB address can complete Lockdown but then lose
    // the CoreDevice proxy. Prefer a routable LAN address deterministically.
    service
        .addresses
        .iter()
        .find_map(|address| match address {
            ScopedIp::V4(address)
                if !address.addr().is_link_local()
                    && !address.addr().is_loopback()
                    && !address.addr().is_unspecified() =>
            {
                Some((IpAddr::V4(*address.addr()), None, true))
            }
            _ => None,
        })
        .or_else(|| {
            service.addresses.iter().find_map(|address| match address {
                ScopedIp::V6(address) => Some((
                    IpAddr::V6(*address.addr()),
                    Some(address.scope_id().index),
                    false,
                )),
                _ => None,
            })
        })
        .or_else(|| {
            service.addresses.iter().find_map(|address| match address {
                ScopedIp::V4(address) => Some((IpAddr::V4(*address.addr()), None, true)),
                _ => None,
            })
        })
}

fn load_pairing_files<Store>(store: &Store) -> HashMap<String, PairingFile>
where
    Store: WifiPairingStore,
{
    let mut files = HashMap::new();
    let records = match store.load() {
        Ok(records) => records,
        Err(error) => {
            tracing::warn!(%error, "unable to load cached Wi-Fi pairing records");
            return files;
        }
    };
    for record in records {
        match PairingFile::from_bytes(&record.bytes) {
            Ok(pairing_file) => {
                files.insert(record.udid, pairing_file);
            }
            Err(error) => tracing::warn!(
                device_id = %device_id_fingerprint(&record.udid),
                ?error,
                "ignored invalid cached pairing record"
            ),
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone, Default)]
    struct MemoryStore(Arc<Mutex<HashMap<String, Vec<u8>>>>);

    impl WifiPairingStore for MemoryStore {
        fn load(&self) -> Result<Vec<StoredWifiPairingRecord>, String> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .iter()
                .map(|(udid, bytes)| StoredWifiPairingRecord {
                    udid: udid.clone(),
                    bytes: bytes.clone(),
                })
                .collect())
        }

        fn save(&self, udid: &str, bytes: &[u8]) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .insert(udid.to_owned(), bytes.to_vec());
            Ok(())
        }

        fn remove(&self, udid: &str) -> Result<(), String> {
            self.0.lock().unwrap().remove(udid);
            Ok(())
        }
    }

    #[test]
    fn invalid_host_records_are_ignored_without_exposing_storage() {
        let store = MemoryStore::default();
        store.save("device", b"not a plist").unwrap();
        assert!(load_pairing_files(&store).is_empty());
        store.remove("device").unwrap();
        assert!(store.load().unwrap().is_empty());
    }
}
