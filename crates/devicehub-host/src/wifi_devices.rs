//! Host filesystem adapter for runtime-owned device pairing credentials.

use std::fs::OpenOptions;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use devicehub_runtime::{PairingStore, StoredLockdownPairingRecord};

const MAX_PAIRING_RECORD_BYTES: u64 = 1024 * 1024;

#[derive(Clone)]
pub struct HostPairingStore {
    directories: PairingDirectories,
}

#[derive(Clone)]
struct PairingDirectories {
    lockdown: PathBuf,
    remote: PathBuf,
}

impl HostPairingStore {
    pub fn new(directory: PathBuf) -> Result<Self, String> {
        secure_directory(&directory)?;
        let remote = directory
            .parent()
            .unwrap_or(&directory)
            .join("remote-pairings");
        secure_directory(&remote)?;
        Ok(Self {
            directories: PairingDirectories {
                lockdown: directory,
                remote,
            },
        })
    }
}

impl PairingStore for HostPairingStore {
    fn load_lockdown_pairings(&self) -> Result<Vec<StoredLockdownPairingRecord>, String> {
        let directory = &self.directories.lockdown;
        let entries = std::fs::read_dir(directory)
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
                Ok(bytes) => records.push(StoredLockdownPairingRecord {
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

    fn save_lockdown_pairing(&self, udid: &str, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() as u64 > MAX_PAIRING_RECORD_BYTES {
            return Err("pairing record exceeds the storage limit".into());
        }
        write_private_file(&pairing_path(&self.directories.lockdown, udid)?, bytes)
    }

    fn remove_lockdown_pairing(&self, udid: &str) -> Result<(), String> {
        let path = pairing_path(&self.directories.lockdown, udid)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("cannot remove cached pairing record: {error}")),
        }
    }
    fn load_remote_pairing(&self, udid: &str) -> Result<Option<Vec<u8>>, String> {
        let path = pairing_path(&self.directories.remote, udid)?;
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(format!(
                    "cannot inspect remote pairing credentials: {error}"
                ));
            }
        };
        if !metadata.is_file() || metadata.len() > MAX_PAIRING_RECORD_BYTES {
            return Err("remote pairing credential file is invalid or too large".into());
        }
        std::fs::read(path)
            .map(Some)
            .map_err(|error| format!("cannot read remote pairing credentials: {error}"))
    }

    fn save_remote_pairing(&self, udid: &str, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() as u64 > MAX_PAIRING_RECORD_BYTES {
            return Err("remote pairing credentials exceed the storage limit".into());
        }
        let directory = &self.directories.remote;
        secure_directory(directory)?;
        write_private_file(&pairing_path(directory, udid)?, bytes)
    }

    fn remove_remote_pairing(&self, udid: &str) -> Result<(), String> {
        let path = pairing_path(&self.directories.remote, udid)?;
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("cannot remove remote pairing credentials: {error}")),
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
        let store = HostPairingStore::new(directory.clone()).unwrap();
        let udid = "00008110-0011223344556677";
        store.save_lockdown_pairing(udid, b"pairing").unwrap();

        store.remove_lockdown_pairing(udid).unwrap();
        assert!(store.load_lockdown_pairings().unwrap().is_empty());
        store.remove_lockdown_pairing(udid).unwrap();
        assert!(store.remove_lockdown_pairing("../outside").is_err());

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn remote_pairing_storage_is_private_idempotent_and_confined() {
        let base = std::env::temp_dir().join(format!(
            "devicehub-mask-remote-pairing-{}",
            uuid::Uuid::new_v4()
        ));
        let store = HostPairingStore::new(base.join("pairings")).unwrap();
        let udid = "00008110-0011223344556677";

        assert_eq!(store.load_remote_pairing(udid).unwrap(), None);
        store.save_remote_pairing(udid, b"remote-pairing").unwrap();
        assert_eq!(
            store.load_remote_pairing(udid).unwrap(),
            Some(b"remote-pairing".to_vec())
        );
        store.remove_remote_pairing(udid).unwrap();
        store.remove_remote_pairing(udid).unwrap();
        assert!(store.remove_remote_pairing("../outside").is_err());

        std::fs::remove_dir_all(base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn startup_secures_an_existing_remote_pairing_directory() {
        let base = std::env::temp_dir().join(format!(
            "devicehub-mask-remote-permissions-{}",
            uuid::Uuid::new_v4()
        ));
        let remote = base.join("remote-pairings");
        std::fs::create_dir_all(&remote).unwrap();
        std::fs::set_permissions(&remote, std::fs::Permissions::from_mode(0o755)).unwrap();

        HostPairingStore::new(base.join("pairings")).unwrap();

        let mode = std::fs::metadata(&remote).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        std::fs::remove_dir_all(base).unwrap();
    }
}
