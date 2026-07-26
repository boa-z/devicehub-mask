//! Desktop filesystem adapter for runtime-owned MobileBackup2 orchestration.

use std::path::{Path, PathBuf};

#[cfg(test)]
pub(crate) use devicehub_core::DeviceBackupState;
pub(crate) use devicehub_core::{DeviceBackupSlot, DeviceBackupStatus};
pub(crate) type DeviceBackupCommand = devicehub_runtime::DeviceBackupCommand<PathBuf>;

const MAX_PATH_BYTES: usize = 4_096;

pub(crate) async fn prepare_destination(destination: &Path) -> Result<PathBuf, String> {
    if !destination.is_absolute() {
        return Err("backup destination must be an absolute directory".into());
    }
    if destination.to_string_lossy().len() > MAX_PATH_BYTES {
        return Err("backup destination path is too long".into());
    }
    let canonical = tokio::fs::canonicalize(destination)
        .await
        .map_err(|error| format!("backup destination is unavailable: {error}"))?;
    let metadata = tokio::fs::metadata(&canonical)
        .await
        .map_err(|error| format!("backup destination is unavailable: {error}"))?;
    if !metadata.is_dir() {
        return Err("backup destination must be an existing directory".into());
    }
    if canonical.parent().is_none() {
        return Err("the filesystem root cannot be used as a backup destination".into());
    }
    Ok(canonical)
}

#[derive(Clone, Copy, Default)]
pub(crate) struct TokioDeviceBackupDestination;

impl devicehub_runtime::DeviceBackupDestination for TokioDeviceBackupDestination {
    type Destination = PathBuf;

    fn destination_name(&self, destination: &PathBuf) -> Option<String> {
        destination
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
    }

    fn prepare<'a>(
        &'a self,
        destination: PathBuf,
    ) -> devicehub_runtime::DeviceBackupPrepareFuture<'a> {
        Box::pin(async move { prepare_destination(&destination).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn backup_destination_must_be_an_existing_non_root_directory() {
        assert!(prepare_destination(Path::new("relative")).await.is_err());
        let missing = std::env::temp_dir().join(format!(
            "devicehub-mask-missing-backup-{}",
            uuid::Uuid::new_v4()
        ));
        assert!(prepare_destination(&missing).await.is_err());
        assert!(prepare_destination(&std::env::temp_dir()).await.is_ok());
    }
}
