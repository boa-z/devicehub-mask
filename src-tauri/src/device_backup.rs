//! Desktop filesystem adapter for runtime-owned MobileBackup2 orchestration.

use std::future::Future;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Instant;

use idevice::IdeviceError;
use idevice::mobilebackup2::{BackupDelegate, DirEntryInfo, FsBackupDelegate, MobileBackup2Client};

#[cfg(test)]
pub(crate) use devicehub_runtime::DeviceBackupState;
pub(crate) use devicehub_runtime::{DeviceBackupSlot, DeviceBackupStatus, DeviceBackupTransport};
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
pub(crate) struct TokioDeviceBackupExecutor;

impl devicehub_runtime::DeviceBackupExecutor for TokioDeviceBackupExecutor {
    type Destination = PathBuf;
    type Prepared = PathBuf;

    fn destination_name(&self, destination: &PathBuf) -> Option<String> {
        destination
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
    }

    fn prepare<'a>(
        &'a self,
        destination: PathBuf,
        source_identifier: &'a str,
    ) -> devicehub_runtime::DeviceBackupPrepareFuture<'a, PathBuf> {
        Box::pin(async move {
            let destination = prepare_destination(&destination).await?;
            reject_symlink(&destination.join(source_identifier)).await?;
            Ok(destination)
        })
    }

    fn execute<'a>(
        &'a self,
        mut client: MobileBackup2Client,
        destination: PathBuf,
        source_identifier: String,
        full: bool,
        status: DeviceBackupSlot,
        started: Instant,
    ) -> devicehub_runtime::DeviceBackupFuture<'a> {
        Box::pin(async move {
            let delegate = ConfinedBackupDelegate::new(destination.clone(), status, started);
            let mut options = plist::Dictionary::new();
            if full {
                options.insert("ForceFullBackup".into(), plist::Value::Boolean(true));
            }
            let options = (!options.is_empty()).then_some(options);
            let result = client
                .backup_from_path(&destination, Some(&source_identifier), options, &delegate)
                .await;
            if result.is_ok() {
                let _ = client.disconnect().await;
            }
            result
        })
    }
}

pub(crate) async fn serve(
    transport: DeviceBackupTransport,
    commands: tokio::sync::mpsc::Receiver<DeviceBackupCommand>,
    status: DeviceBackupSlot,
    reporter: crate::supervisor::ServiceReporter,
    shutdown: tokio::sync::watch::Receiver<bool>,
) {
    devicehub_runtime::serve_device_backup(
        transport,
        commands,
        status,
        TokioDeviceBackupExecutor,
        reporter,
        shutdown,
    )
    .await;
}

async fn reject_symlink(path: &Path) -> Result<(), String> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err("the device backup directory cannot be a symbolic link".into())
        }
        Ok(metadata) if !metadata.is_dir() => {
            Err("the existing device backup path is not a directory".into())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "unable to inspect the device backup directory: {error}"
        )),
    }
}

#[derive(Default)]
struct DelegateProgress {
    completed_batches: u64,
    last_batch_count: u32,
}

struct ConfinedBackupDelegate {
    fs: FsBackupDelegate,
    root: PathBuf,
    status: DeviceBackupSlot,
    started: Instant,
    progress: Mutex<DelegateProgress>,
}

impl ConfinedBackupDelegate {
    fn new(root: PathBuf, status: DeviceBackupSlot, started: Instant) -> Self {
        Self {
            fs: FsBackupDelegate,
            root,
            status,
            started,
            progress: Mutex::new(DelegateProgress::default()),
        }
    }

    async fn validate_path(&self, path: &Path) -> Result<(), IdeviceError> {
        let relative = path.strip_prefix(&self.root).map_err(|_| {
            IdeviceError::InternalError("backup path escaped the selected directory".into())
        })?;
        let mut current = self.root.clone();
        for component in relative.components() {
            let Component::Normal(component) = component else {
                return Err(IdeviceError::InternalError(
                    "backup path contains an unsafe component".into(),
                ));
            };
            current.push(component);
            match tokio::fs::symlink_metadata(&current).await {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(IdeviceError::InternalError(
                        "backup path traverses a symbolic link".into(),
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) => return Err(IdeviceError::InternalError(error.to_string())),
            }
        }
        Ok(())
    }
}

impl BackupDelegate for ConfinedBackupDelegate {
    fn get_free_disk_space(&self, _path: &Path) -> u64 {
        self.fs.get_free_disk_space(&self.root)
    }

    fn open_file_read<'a>(
        &'a self,
        path: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<Box<dyn Read + Send>, IdeviceError>> + Send + 'a>> {
        Box::pin(async move {
            self.validate_path(path).await?;
            self.fs.open_file_read(path).await
        })
    }

    fn create_file_write<'a>(
        &'a self,
        path: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<Box<dyn Write + Send>, IdeviceError>> + Send + 'a>>
    {
        Box::pin(async move {
            self.validate_path(path).await?;
            self.fs.create_file_write(path).await
        })
    }

    fn create_dir_all<'a>(
        &'a self,
        path: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<(), IdeviceError>> + Send + 'a>> {
        Box::pin(async move {
            self.validate_path(path).await?;
            self.fs.create_dir_all(path).await
        })
    }

    fn remove<'a>(
        &'a self,
        path: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<(), IdeviceError>> + Send + 'a>> {
        Box::pin(async move {
            self.validate_path(path).await?;
            self.fs.remove(path).await
        })
    }

    fn rename<'a>(
        &'a self,
        from: &'a Path,
        to: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<(), IdeviceError>> + Send + 'a>> {
        Box::pin(async move {
            self.validate_path(from).await?;
            self.validate_path(to).await?;
            self.fs.rename(from, to).await
        })
    }

    fn copy<'a>(
        &'a self,
        source: &'a Path,
        destination: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<(), IdeviceError>> + Send + 'a>> {
        Box::pin(async move {
            self.validate_path(source).await?;
            self.validate_path(destination).await?;
            self.fs.copy(source, destination).await
        })
    }

    fn exists<'a>(&'a self, path: &'a Path) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(
            async move { self.validate_path(path).await.is_ok() && self.fs.exists(path).await },
        )
    }

    fn is_dir<'a>(&'a self, path: &'a Path) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(
            async move { self.validate_path(path).await.is_ok() && self.fs.is_dir(path).await },
        )
    }

    fn list_dir<'a>(
        &'a self,
        path: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<DirEntryInfo>, IdeviceError>> + Send + 'a>> {
        Box::pin(async move {
            self.validate_path(path).await?;
            self.fs.list_dir(path).await
        })
    }

    fn on_file_received(&self, _path: &str, file_count: u32) {
        let total = {
            let mut progress = self
                .progress
                .lock()
                .expect("device backup progress lock poisoned");
            if file_count <= progress.last_batch_count && progress.last_batch_count > 0 {
                progress.completed_batches = progress
                    .completed_batches
                    .saturating_add(progress.last_batch_count as u64);
            }
            progress.last_batch_count = file_count;
            progress.completed_batches.saturating_add(file_count as u64)
        };
        self.status.update(|current| current.files_received = total);
    }

    fn on_progress(&self, bytes_done: u64, bytes_total: u64, overall_progress: f64) {
        self.status.update(|current| {
            current.bytes_done = bytes_done;
            current.bytes_total = bytes_total;
            current.progress_percent = if overall_progress.is_finite() && overall_progress >= 0.0 {
                Some(overall_progress.clamp(0.0, 100.0))
            } else if bytes_total > 0 {
                Some((bytes_done as f64 * 100.0 / bytes_total as f64).clamp(0.0, 100.0))
            } else {
                None
            };
            current.elapsed_ms = elapsed_ms(self.started);
        });
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u64::MAX as u128) as u64
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

    #[cfg(unix)]
    #[tokio::test]
    async fn confined_delegate_rejects_symbolic_link_ancestors() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "devicehub-mask-backup-root-{}",
            uuid::Uuid::new_v4()
        ));
        let outside = std::env::temp_dir().join(format!(
            "devicehub-mask-backup-outside-{}",
            uuid::Uuid::new_v4()
        ));
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::create_dir_all(&outside).await.unwrap();
        symlink(&outside, root.join("device")).unwrap();
        let delegate = ConfinedBackupDelegate::new(
            tokio::fs::canonicalize(&root).await.unwrap(),
            DeviceBackupSlot::default(),
            Instant::now(),
        );
        assert!(
            delegate
                .create_file_write(&root.join("device/Manifest.db"))
                .await
                .is_err()
        );
        tokio::fs::remove_dir_all(&root).await.unwrap();
        tokio::fs::remove_dir_all(&outside).await.unwrap();
    }
}
