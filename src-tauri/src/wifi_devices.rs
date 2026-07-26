//! Host filesystem adapter for runtime-owned Bonjour device discovery.

use std::fs::OpenOptions;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use devicehub_runtime::{StoredWifiPairingRecord, WifiPairingStore};

const MAX_PAIRING_RECORD_BYTES: u64 = 1024 * 1024;

#[derive(Clone)]
pub(crate) struct HostWifiPairingStore {
    directory: PathBuf,
}

impl HostWifiPairingStore {
    pub(crate) fn new(directory: PathBuf) -> Result<Self, String> {
        secure_directory(&directory)?;
        Ok(Self { directory })
    }
}

impl WifiPairingStore for HostWifiPairingStore {
    fn load(&self) -> Result<Vec<StoredWifiPairingRecord>, String> {
        let entries = std::fs::read_dir(&self.directory)
            .map_err(|error| format!("cannot read pairing directory: {error}"))?;
        let mut records = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("plist") {
                continue;
            }
            let Some(udid) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            if validate_udid(udid).is_err() {
                continue;
            }
            match entry.metadata() {
                Ok(metadata)
                    if metadata.is_file() && metadata.len() <= MAX_PAIRING_RECORD_BYTES => {}
                Ok(_) => continue,
                Err(error) => {
                    tracing::warn!(path = %path.display(), %error, "ignored unreadable pairing record");
                    continue;
                }
            }
            match std::fs::read(&path) {
                Ok(bytes) => records.push(StoredWifiPairingRecord {
                    udid: udid.to_owned(),
                    bytes,
                }),
                Err(error) => tracing::warn!(
                    path = %path.display(),
                    %error,
                    "ignored unreadable pairing record"
                ),
            }
        }
        Ok(records)
    }

    fn save(&self, udid: &str, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() as u64 > MAX_PAIRING_RECORD_BYTES {
            return Err("pairing record exceeds the storage limit".into());
        }
        write_private_file(&pairing_path(&self.directory, udid)?, bytes)
    }

    fn remove(&self, udid: &str) -> Result<(), String> {
        let path = pairing_path(&self.directory, udid)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("cannot remove cached pairing record: {error}")),
        }
    }
}

fn validate_udid(udid: &str) -> Result<(), String> {
    if udid.is_empty()
        || !udid
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("device UDID contains unsupported characters".into());
    }
    Ok(())
}

fn pairing_path(directory: &Path, udid: &str) -> Result<PathBuf, String> {
    validate_udid(udid)?;
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

    #[test]
    fn cached_pairing_removal_is_idempotent_and_confined() {
        let directory = std::env::temp_dir().join(format!(
            "devicehub-mask-pairing-removal-{}",
            uuid::Uuid::new_v4()
        ));
        let store = HostWifiPairingStore::new(directory.clone()).unwrap();
        let udid = "00008110-0011223344556677";
        store.save(udid, b"pairing").unwrap();

        store.remove(udid).unwrap();
        assert!(store.load().unwrap().is_empty());
        store.remove(udid).unwrap();
        assert!(store.remove("../outside").is_err());

        std::fs::remove_dir_all(directory).unwrap();
    }
}
