//! User-initiated, cancellable MobileBackup2 backups confined to a selected directory.

use std::future::Future;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use idevice::mobilebackup2::{BackupDelegate, DirEntryInfo, FsBackupDelegate, MobileBackup2Client};
use idevice::provider::IdeviceProvider;
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use idevice::{IdeviceError, IdeviceService, RsdService};
use serde::Serialize;
use tokio::sync::{mpsc, oneshot, watch};

use crate::protocol::ConnKind;
use crate::supervisor::ServiceReporter;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const STATUS_INTERVAL: Duration = Duration::from_millis(250);
const MAX_PATH_BYTES: usize = 4_096;
const MAX_ERROR_BYTES: usize = 1_024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceBackupState {
    #[default]
    Idle,
    Starting,
    BackingUp,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DeviceBackupStatus {
    pub state: DeviceBackupState,
    pub files_received: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub progress_percent: Option<f64>,
    pub elapsed_ms: u64,
    pub full: bool,
    pub destination_name: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Default)]
pub struct DeviceBackupSlot(Arc<Mutex<DeviceBackupStatus>>);

impl DeviceBackupSlot {
    pub fn set(&self, status: DeviceBackupStatus) {
        *self.0.lock().expect("device backup status lock poisoned") = status;
    }

    pub fn update(&self, update: impl FnOnce(&mut DeviceBackupStatus)) {
        update(&mut self.0.lock().expect("device backup status lock poisoned"));
    }

    pub fn get(&self) -> DeviceBackupStatus {
        self.0
            .lock()
            .expect("device backup status lock poisoned")
            .clone()
    }

    pub fn reset(&self) {
        self.set(DeviceBackupStatus::default());
    }
}

#[derive(Debug)]
pub enum DeviceBackupCommand {
    Start {
        destination: PathBuf,
        full: bool,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Stop {
        reply: oneshot::Sender<Result<(), String>>,
    },
}

pub struct DeviceBackupTransport {
    provider: Arc<dyn IdeviceProvider>,
    connection: ConnKind,
    adapter: AdapterHandle,
    handshake: RsdHandshake,
    source_identifier: String,
}

impl DeviceBackupTransport {
    pub fn new(
        provider: Arc<dyn IdeviceProvider>,
        connection: ConnKind,
        adapter: AdapterHandle,
        handshake: RsdHandshake,
        source_identifier: String,
    ) -> Self {
        Self {
            provider,
            connection,
            adapter,
            handshake,
            source_identifier,
        }
    }
}

pub async fn prepare_destination(destination: &Path) -> Result<PathBuf, String> {
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

pub async fn serve(
    mut transport: DeviceBackupTransport,
    mut commands: mpsc::Receiver<DeviceBackupCommand>,
    status: DeviceBackupSlot,
    reporter: ServiceReporter,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut attempt = 0;
    status.reset();
    reporter.stopped(attempt);
    loop {
        let command = tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
                continue;
            }
            command = commands.recv() => command,
        };
        let Some(command) = command else { return };
        match command {
            DeviceBackupCommand::Stop { reply } => {
                let _ = reply.send(Err("no device backup is running".into()));
            }
            DeviceBackupCommand::Start {
                destination,
                full,
                reply,
            } => {
                attempt += 1;
                let destination_name = destination
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .filter(|name| !name.is_empty());
                status.set(DeviceBackupStatus {
                    state: DeviceBackupState::Starting,
                    full,
                    destination_name,
                    ..DeviceBackupStatus::default()
                });
                reporter.connecting(attempt);
                let result = run_backup(
                    &mut transport,
                    destination,
                    full,
                    &mut commands,
                    &status,
                    &reporter,
                    attempt,
                    &mut shutdown,
                    reply,
                )
                .await;
                if result == BackupRunResult::SessionEnded {
                    return;
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackupRunResult {
    Continue,
    SessionEnded,
}

#[allow(clippy::too_many_arguments)]
async fn run_backup(
    transport: &mut DeviceBackupTransport,
    destination: PathBuf,
    full: bool,
    commands: &mut mpsc::Receiver<DeviceBackupCommand>,
    status: &DeviceBackupSlot,
    reporter: &ServiceReporter,
    attempt: u32,
    shutdown: &mut watch::Receiver<bool>,
    reply: oneshot::Sender<Result<(), String>>,
) -> BackupRunResult {
    let destination = match prepare_destination(&destination).await {
        Ok(destination) => destination,
        Err(error) => {
            fail_start(status, reporter, attempt, error, reply);
            return BackupRunResult::Continue;
        }
    };
    if let Err(error) = validate_source_identifier(&transport.source_identifier) {
        fail_start(status, reporter, attempt, error, reply);
        return BackupRunResult::Continue;
    }
    let device_dir = destination.join(&transport.source_identifier);
    if let Err(error) = reject_symlink(&device_dir).await {
        fail_start(status, reporter, attempt, error, reply);
        return BackupRunResult::Continue;
    }

    let client = match connect_client(transport).await {
        Ok(client) => client,
        Err(error) => {
            fail_start(status, reporter, attempt, error, reply);
            return BackupRunResult::Continue;
        }
    };
    let started = Instant::now();
    let delegate = ConfinedBackupDelegate::new(destination.clone(), status.clone(), started);
    let source_identifier = transport.source_identifier.clone();
    let mut options = plist::Dictionary::new();
    if full {
        options.insert("ForceFullBackup".into(), plist::Value::Boolean(true));
    }
    let options = (!options.is_empty()).then_some(options);
    let backup = async move {
        let mut client = client;
        let result = client
            .backup_from_path(&destination, Some(&source_identifier), options, &delegate)
            .await;
        if result.is_ok() {
            let _ = client.disconnect().await;
        }
        result
    };
    tokio::pin!(backup);

    status.update(|current| {
        current.state = DeviceBackupState::BackingUp;
        current.elapsed_ms = 0;
    });
    reporter.ready(attempt);
    tracing::info!(full, "MobileBackup2 backup started");
    let _ = reply.send(Ok(()));
    let mut ticker = tokio::time::interval(STATUS_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            result = &mut backup => {
                match result.and_then(validate_final_response) {
                    Ok(()) => {
                        status.update(|current| {
                            current.state = DeviceBackupState::Completed;
                            current.progress_percent = Some(100.0);
                            current.elapsed_ms = elapsed_ms(started);
                            current.error = None;
                        });
                        reporter.stopped(attempt);
                        tracing::info!(elapsed_ms = elapsed_ms(started), "MobileBackup2 backup completed");
                    }
                    Err(error) => {
                        let error = describe_error(&error);
                        status.update(|current| {
                            current.state = DeviceBackupState::Failed;
                            current.elapsed_ms = elapsed_ms(started);
                            current.error = Some(error.clone());
                        });
                        reporter.unavailable(attempt, error.clone());
                        tracing::warn!(elapsed_ms = elapsed_ms(started), error, "MobileBackup2 backup failed");
                    }
                }
                return BackupRunResult::Continue;
            }
            _ = ticker.tick() => {
                status.update(|current| current.elapsed_ms = elapsed_ms(started));
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    cancel_status(status, started, "device session ended");
                    reporter.stopped(attempt);
                    return BackupRunResult::SessionEnded;
                }
            }
            command = commands.recv() => match command {
                Some(DeviceBackupCommand::Stop { reply }) => {
                    cancel_status(status, started, "cancelled by user");
                    reporter.stopped(attempt);
                    let _ = reply.send(Ok(()));
                    tracing::info!(elapsed_ms = elapsed_ms(started), "MobileBackup2 backup cancelled");
                    return BackupRunResult::Continue;
                }
                Some(DeviceBackupCommand::Start { reply, .. }) => {
                    let _ = reply.send(Err("a device backup is already running".into()));
                }
                None => {
                    cancel_status(status, started, "device session ended");
                    reporter.stopped(attempt);
                    return BackupRunResult::SessionEnded;
                }
            }
        }
    }
}

fn fail_start(
    status: &DeviceBackupSlot,
    reporter: &ServiceReporter,
    attempt: u32,
    error: String,
    reply: oneshot::Sender<Result<(), String>>,
) {
    status.update(|current| {
        current.state = DeviceBackupState::Failed;
        current.error = Some(error.clone());
    });
    reporter.unavailable(attempt, error.clone());
    let _ = reply.send(Err(error));
}

fn cancel_status(status: &DeviceBackupSlot, started: Instant, reason: &str) {
    status.update(|current| {
        current.state = DeviceBackupState::Cancelled;
        current.elapsed_ms = elapsed_ms(started);
        current.error = Some(reason.into());
    });
}

async fn connect_client(
    transport: &mut DeviceBackupTransport,
) -> Result<MobileBackup2Client, String> {
    let mut failures = Vec::new();
    if transport.connection == ConnKind::Usb {
        match tokio::time::timeout(
            CONNECT_TIMEOUT,
            MobileBackup2Client::connect(transport.provider.as_ref()),
        )
        .await
        {
            Ok(Ok(client)) => {
                tracing::info!(
                    transport = "lockdown-usb",
                    "MobileBackup2 service connected"
                );
                return Ok(client);
            }
            Ok(Err(error)) => failures.push(format!("USB lockdown: {}", describe_error(&error))),
            Err(_) => failures.push("USB lockdown: connection timed out".into()),
        }
    }

    match tokio::time::timeout(
        CONNECT_TIMEOUT,
        MobileBackup2Client::connect_rsd(&mut transport.adapter, &mut transport.handshake),
    )
    .await
    {
        Ok(Ok(client)) => {
            tracing::info!(
                transport = "coredevice-rsd",
                "MobileBackup2 service connected"
            );
            Ok(client)
        }
        Ok(Err(error)) => {
            failures.push(format!("CoreDevice RSD: {}", describe_error(&error)));
            Err(format!(
                "MobileBackup2 service unavailable: {}",
                failures.join("; ")
            ))
        }
        Err(_) => {
            failures.push("CoreDevice RSD: connection timed out".into());
            Err(format!(
                "MobileBackup2 service unavailable: {}",
                failures.join("; ")
            ))
        }
    }
}

fn validate_final_response(response: Option<plist::Dictionary>) -> Result<(), IdeviceError> {
    let Some(response) = response else {
        return Ok(());
    };
    let code = response
        .get("ErrorCode")
        .and_then(|value| {
            value
                .as_signed_integer()
                .or_else(|| value.as_unsigned_integer().map(|value| value as i64))
        })
        .unwrap_or(0);
    if code == 0 {
        return Ok(());
    }
    let description = response
        .get("ErrorDescription")
        .and_then(plist::Value::as_string)
        .unwrap_or("the device reported an unknown backup error");
    Err(IdeviceError::InternalError(format!(
        "device backup error {code}: {description}"
    )))
}

fn validate_source_identifier(source: &str) -> Result<(), String> {
    if source.is_empty()
        || source.len() > 128
        || !source
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("device identifier cannot be used as a safe backup directory name".into());
    }
    Ok(())
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

fn describe_error(error: &IdeviceError) -> String {
    let message = match error {
        IdeviceError::UnknownErrorType(message)
            if message.eq_ignore_ascii_case("ServiceProhibited") =>
        {
            "the device prohibited the MobileBackup2 service".into()
        }
        _ => format!("{error:?}"),
    };
    sanitize_message(&message)
}

fn sanitize_message(message: &str) -> String {
    let message = message.replace(['\r', '\n'], " ");
    if message.len() <= MAX_ERROR_BYTES {
        return message;
    }
    let mut end = MAX_ERROR_BYTES;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &message[..end])
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u64::MAX as u128) as u64
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
        let existing = std::env::temp_dir();
        assert!(prepare_destination(&existing).await.is_ok());
    }

    #[test]
    fn source_identifiers_cannot_introduce_paths() {
        assert!(validate_source_identifier("00008110-001234567890001E").is_ok());
        assert!(validate_source_identifier("../outside").is_err());
        assert!(validate_source_identifier("").is_err());
    }

    #[test]
    fn final_device_errors_are_not_treated_as_success() {
        let mut response = plist::Dictionary::new();
        response.insert("ErrorCode".into(), plist::Value::Integer(42.into()));
        response.insert(
            "ErrorDescription".into(),
            plist::Value::String("device locked".into()),
        );
        assert!(validate_final_response(Some(response)).is_err());
        assert!(validate_final_response(None).is_ok());
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
