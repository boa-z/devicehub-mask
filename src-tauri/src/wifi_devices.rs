//! Paired iOS device discovery over Bonjour.
//!
//! Apple usbmuxd does not consistently publish iOS 26.4+ network devices. We
//! cache the pairing record while USB is available, authenticate Bonjour TXT
//! records with its HostID, then connect to lockdownd services directly.

use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::net::IpAddr;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use idevice::pairing_file::PairingFile;
use mdns_sd::{Receiver, ResolvedService, ScopedIp, ServiceDaemon, ServiceEvent};

const SERVICE_TYPE: &str = "_apple-mobdev2._tcp.local.";

#[derive(Clone, Debug)]
pub struct WifiEndpoint {
    pub udid: String,
    pub address: IpAddr,
    pub scope_id: Option<u32>,
    pub pairing_file: PairingFile,
}

pub struct WifiDiscovery {
    pairing_dir: PathBuf,
    pairing_files: HashMap<String, PairingFile>,
    refreshed_pairings: HashSet<String>,
    services: HashMap<String, ResolvedService>,
    announced: HashSet<String>,
    receiver: Receiver<ServiceEvent>,
    _daemon: ServiceDaemon,
}

impl WifiDiscovery {
    pub fn start(pairing_dir: PathBuf) -> Result<Self, String> {
        secure_directory(&pairing_dir)?;
        let pairing_files = load_pairing_files(&pairing_dir);
        let daemon = ServiceDaemon::new()
            .map_err(|error| format!("cannot initialize Bonjour discovery: {error}"))?;
        let receiver = daemon
            .browse(SERVICE_TYPE)
            .map_err(|error| format!("cannot browse for iOS devices: {error}"))?;
        tracing::info!(
            cached_pairings = pairing_files.len(),
            service_type = SERVICE_TYPE,
            "Wi-Fi device discovery started"
        );
        Ok(Self {
            pairing_dir,
            pairing_files,
            refreshed_pairings: HashSet::new(),
            services: HashMap::new(),
            announced: HashSet::new(),
            receiver,
            _daemon: daemon,
        })
    }

    pub fn cache_pairing(&mut self, udid: &str, pairing_file: PairingFile) -> Result<(), String> {
        let path = pairing_path(&self.pairing_dir, udid)?;
        let bytes = pairing_file
            .clone()
            .serialize()
            .map_err(|error| format!("cannot serialize pairing record: {error:?}"))?;
        write_private_file(&path, &bytes)?;
        let inserted = self
            .pairing_files
            .insert(udid.to_owned(), pairing_file)
            .is_none();
        if inserted {
            tracing::info!(
                device_id = %crate::diagnostics::device_id_fingerprint(udid),
                "cached pairing record for authenticated Wi-Fi discovery"
            );
        }
        Ok(())
    }

    pub fn pairing_needs_refresh(&self, udid: &str) -> bool {
        !self.refreshed_pairings.contains(udid)
    }

    pub fn mark_pairing_refreshed(&mut self, udid: &str) {
        self.refreshed_pairings.insert(udid.to_owned());
    }

    pub fn refresh(&mut self) -> Vec<WifiEndpoint> {
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                ServiceEvent::ServiceResolved(service) => {
                    self.services.insert(service.fullname.clone(), *service);
                }
                ServiceEvent::ServiceRemoved(_, fullname) => {
                    self.services.remove(&fullname);
                    self.announced.remove(&fullname);
                }
                _ => {}
            }
        }

        let mut by_udid = HashMap::<String, (WifiEndpoint, bool)>::new();
        for (fullname, service) in &self.services {
            let Some((endpoint, ipv4)) = resolve_service(service, &self.pairing_files) else {
                continue;
            };
            if self.announced.insert(fullname.clone()) {
                tracing::info!(
                    device_id = %crate::diagnostics::device_id_fingerprint(&endpoint.udid),
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

    pub fn requires_pairing(&self) -> bool {
        self.services
            .values()
            .any(|service| resolve_service(service, &self.pairing_files).is_none())
    }
}

fn resolve_service(
    service: &ResolvedService,
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
    Some((
        WifiEndpoint {
            udid: udid.clone(),
            address,
            scope_id,
            pairing_file: pairing_file.clone(),
        },
        ipv4,
    ))
}

fn preferred_address(service: &ResolvedService) -> Option<(IpAddr, Option<u32>, bool)> {
    service
        .addresses
        .iter()
        .find_map(|address| match address {
            ScopedIp::V4(address) => Some((IpAddr::V4(*address.addr()), None, true)),
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
}

fn load_pairing_files(directory: &Path) -> HashMap<String, PairingFile> {
    let mut files = HashMap::new();
    let Ok(entries) = std::fs::read_dir(directory) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("plist") {
            continue;
        }
        let Some(udid) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        match PairingFile::read_from_file(&path) {
            Ok(pairing_file) => {
                files.insert(udid.to_owned(), pairing_file);
            }
            Err(error) => tracing::warn!(
                path = %path.display(),
                ?error,
                "ignored invalid cached pairing record"
            ),
        }
    }
    files
}

fn pairing_path(directory: &Path, udid: &str) -> Result<PathBuf, String> {
    if udid.is_empty()
        || !udid
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("device UDID contains unsupported characters".into());
    }
    Ok(directory.join(format!("{udid}.plist")))
}

fn secure_directory(directory: &Path) -> Result<(), String> {
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    #[cfg(unix)]
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("cannot secure {}: {error}", directory.display()))?;
    Ok(())
}

fn write_private_file(path: &Path, contents: &[u8]) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    file.write_all(contents)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    #[cfg(unix)]
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("cannot secure {}: {error}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_path_rejects_path_traversal() {
        let directory = Path::new("pairings");
        assert_eq!(
            pairing_path(directory, "00008110-0011223344556677").unwrap(),
            directory.join("00008110-0011223344556677.plist")
        );
        assert!(pairing_path(directory, "../device").is_err());
        assert!(pairing_path(directory, "device/name").is_err());
    }
}
