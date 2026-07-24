//! Bounded file management for the device's public AFC media container.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use idevice::afc::AfcClient;
use idevice::afc::opcode::AfcFopenMode;
use idevice::provider::IdeviceProvider;
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use idevice::{IdeviceService, RsdService};
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader, BufWriter};
use tokio::sync::{mpsc, oneshot, watch};

use crate::protocol::ConnKind;
use crate::supervisor::ServiceReporter;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const METADATA_TIMEOUT: Duration = Duration::from_secs(30);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const MAX_DIRECTORY_ENTRIES: usize = 1_000;
const MAX_PATH_BYTES: usize = 1_024;
const TRANSFER_BUFFER_BYTES: usize = 64 * 1024;
const MAX_TRANSFER_ENTRIES: usize = 100_000;
const MAX_TRANSFER_DEPTH: usize = 64;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
pub const TRANSFER_CANCELLED: &str = "device file transfer cancelled";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceFileKind {
    File,
    Directory,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceFileEntry {
    pub name: String,
    pub path: String,
    pub kind: DeviceFileKind,
    pub size_bytes: u64,
    pub modified: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceFileList {
    pub path: String,
    pub entries: Vec<DeviceFileEntry>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct DeviceFileTransfer {
    pub bytes_transferred: u64,
    pub files_transferred: u64,
    pub directories_transferred: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceFileActivityKind {
    Export,
    Import,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceFileActivityState {
    #[default]
    Idle,
    Running,
    Succeeded,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DeviceFileActivityView {
    pub id: u64,
    pub kind: Option<DeviceFileActivityKind>,
    pub state: DeviceFileActivityState,
    pub path: Option<String>,
    pub bytes_transferred: u64,
    pub bytes_total: Option<u64>,
    pub files_transferred: u64,
    pub directories_transferred: u64,
    pub error: Option<String>,
}

#[derive(Clone, Default)]
pub struct DeviceFileActivitySlot {
    view: Arc<Mutex<DeviceFileActivityView>>,
    active_id: Arc<AtomicU64>,
    cancelled: Arc<AtomicBool>,
}

impl DeviceFileActivitySlot {
    pub(crate) fn start(&self, kind: DeviceFileActivityKind, path: String) -> u64 {
        let mut view = self
            .view
            .lock()
            .expect("device file activity lock poisoned");
        let id = view.id.wrapping_add(1).max(1);
        self.cancelled.store(false, Ordering::Release);
        self.active_id.store(id, Ordering::Release);
        *view = DeviceFileActivityView {
            id,
            kind: Some(kind),
            state: DeviceFileActivityState::Running,
            path: Some(path),
            ..DeviceFileActivityView::default()
        };
        id
    }

    fn update(&self, id: u64, transfer: DeviceFileTransfer) {
        let mut view = self
            .view
            .lock()
            .expect("device file activity lock poisoned");
        if view.id == id && view.state == DeviceFileActivityState::Running {
            view.bytes_transferred = transfer.bytes_transferred;
            view.files_transferred = transfer.files_transferred;
            view.directories_transferred = transfer.directories_transferred;
        }
    }

    fn set_total(&self, id: u64, bytes_total: u64) {
        let mut view = self
            .view
            .lock()
            .expect("device file activity lock poisoned");
        if view.id == id && view.state == DeviceFileActivityState::Running {
            view.bytes_total = Some(bytes_total);
        }
    }

    fn finish(&self, id: u64, result: &Result<(), String>) {
        let mut view = self
            .view
            .lock()
            .expect("device file activity lock poisoned");
        if view.id != id || view.state != DeviceFileActivityState::Running {
            return;
        }
        match result {
            Ok(()) => {
                view.state = DeviceFileActivityState::Succeeded;
                if let Some(total) = view.bytes_total {
                    view.bytes_transferred = total;
                }
            }
            Err(error) if is_transfer_cancelled(error) => {
                view.state = DeviceFileActivityState::Cancelled;
            }
            Err(error) => {
                view.state = DeviceFileActivityState::Failed;
                view.error = Some(error.chars().take(512).collect());
            }
        }
        self.active_id.store(0, Ordering::Release);
    }

    pub fn get(&self) -> DeviceFileActivityView {
        self.view
            .lock()
            .expect("device file activity lock poisoned")
            .clone()
    }

    pub fn cancel(&self) -> bool {
        let view = self
            .view
            .lock()
            .expect("device file activity lock poisoned");
        if view.state != DeviceFileActivityState::Running {
            return false;
        }
        self.cancelled.store(true, Ordering::Release);
        true
    }

    fn is_cancelled(&self, id: u64) -> bool {
        self.active_id.load(Ordering::Acquire) == id && self.cancelled.load(Ordering::Acquire)
    }

    fn reset(&self) {
        let mut view = self
            .view
            .lock()
            .expect("device file activity lock poisoned");
        self.cancelled.store(false, Ordering::Release);
        self.active_id.store(0, Ordering::Release);
        *view = DeviceFileActivityView::default();
    }
}

pub fn is_transfer_cancelled(error: &str) -> bool {
    error.contains(TRANSFER_CANCELLED)
}

struct TransferProgress {
    slot: DeviceFileActivitySlot,
    id: u64,
    transfer: DeviceFileTransfer,
    last_published: Instant,
    buffer: Vec<u8>,
}

impl TransferProgress {
    fn new(slot: DeviceFileActivitySlot, id: u64) -> Self {
        Self {
            slot,
            id,
            transfer: DeviceFileTransfer::default(),
            last_published: Instant::now(),
            buffer: vec![0u8; TRANSFER_BUFFER_BYTES],
        }
    }

    fn set_total(&self, bytes_total: u64) {
        self.slot.set_total(self.id, bytes_total);
    }

    fn check_cancelled(&self) -> Result<(), String> {
        if self.slot.is_cancelled(self.id) {
            Err(TRANSFER_CANCELLED.into())
        } else {
            Ok(())
        }
    }

    fn file(&mut self) {
        self.transfer.files_transferred = self.transfer.files_transferred.saturating_add(1);
        self.publish(true);
    }

    fn directory(&mut self) {
        self.transfer.directories_transferred =
            self.transfer.directories_transferred.saturating_add(1);
        self.publish(true);
    }

    fn publish(&mut self, force: bool) {
        if force || self.last_published.elapsed() >= PROGRESS_INTERVAL {
            self.slot.update(self.id, self.transfer);
            self.last_published = Instant::now();
        }
    }

    async fn copy<R, W>(&mut self, reader: &mut R, writer: &mut W) -> Result<u64, String>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let mut total = 0u64;
        loop {
            self.check_cancelled()?;
            let read = reader
                .read(&mut self.buffer)
                .await
                .map_err(|error| error.to_string())?;
            if read == 0 {
                return Ok(total);
            }
            self.check_cancelled()?;
            writer
                .write_all(&self.buffer[..read])
                .await
                .map_err(|error| error.to_string())?;
            let read = read as u64;
            total = total.saturating_add(read);
            self.transfer.bytes_transferred = self.transfer.bytes_transferred.saturating_add(read);
            self.publish(false);
        }
    }

    fn finish(mut self) -> DeviceFileTransfer {
        self.publish(true);
        self.transfer
    }
}

#[derive(Debug)]
pub enum DeviceFileCommand {
    List {
        path: String,
        reply: oneshot::Sender<Result<DeviceFileList, String>>,
    },
    Export {
        path: String,
        destination: PathBuf,
        reply: oneshot::Sender<Result<DeviceFileTransfer, String>>,
    },
    Import {
        directory: String,
        source: PathBuf,
        reply: oneshot::Sender<Result<DeviceFileEntry, String>>,
    },
    CreateDirectory {
        directory: String,
        name: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Rename {
        path: String,
        name: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Delete {
        path: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
}

pub struct DeviceFileTransport {
    provider: Arc<dyn IdeviceProvider>,
    connection: ConnKind,
    adapter: AdapterHandle,
    handshake: RsdHandshake,
}

impl DeviceFileTransport {
    pub fn new(
        provider: Arc<dyn IdeviceProvider>,
        connection: ConnKind,
        adapter: AdapterHandle,
        handshake: RsdHandshake,
    ) -> Self {
        Self {
            provider,
            connection,
            adapter,
            handshake,
        }
    }

    async fn connect(&mut self) -> Result<AfcClient, String> {
        let mut failures = Vec::new();
        if self.connection == ConnKind::Usb {
            match tokio::time::timeout(CONNECT_TIMEOUT, AfcClient::connect(self.provider.as_ref()))
                .await
            {
                Ok(Ok(client)) => {
                    tracing::debug!(transport = "lockdown-usb", "AFC media service connected");
                    return Ok(client);
                }
                Ok(Err(error)) => failures.push(format!("USB lockdown: {error:?}")),
                Err(_) => failures.push("USB lockdown: connection timed out".into()),
            }
        }
        match tokio::time::timeout(
            CONNECT_TIMEOUT,
            AfcClient::connect_rsd(&mut self.adapter, &mut self.handshake),
        )
        .await
        {
            Ok(Ok(client)) => {
                tracing::debug!(transport = "coredevice-rsd", "AFC media service connected");
                Ok(client)
            }
            Ok(Err(error)) => {
                failures.push(format!("CoreDevice RSD: {error:?}"));
                Err(format!(
                    "AFC media service unavailable: {}",
                    failures.join("; ")
                ))
            }
            Err(_) => {
                failures.push("CoreDevice RSD: connection timed out".into());
                Err(format!(
                    "AFC media service unavailable: {}",
                    failures.join("; ")
                ))
            }
        }
    }
}

pub async fn serve(
    mut transport: DeviceFileTransport,
    mut commands: mpsc::Receiver<DeviceFileCommand>,
    activity: DeviceFileActivitySlot,
    reporter: ServiceReporter,
    mut shutdown: watch::Receiver<bool>,
) {
    activity.reset();
    let mut client = None;
    let mut attempt = 0;
    reporter.stopped(attempt);
    loop {
        let command = tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { break; }
                continue;
            }
            command = commands.recv() => command,
        };
        let Some(command) = command else { break };
        if client.is_none() {
            attempt += 1;
            reporter.connecting(attempt);
            match transport.connect().await {
                Ok(connected) => {
                    client = Some(connected);
                    reporter.ready(attempt);
                }
                Err(error) => {
                    reporter.unavailable(attempt, error.clone());
                    reject(command, error);
                    continue;
                }
            }
        }
        let result = handle(
            client.as_mut().expect("AFC client initialized"),
            command,
            &activity,
        )
        .await;
        if result.is_err() {
            client.take();
            reporter.stopped(attempt);
        }
    }
    activity.reset();
}

async fn handle(
    client: &mut AfcClient,
    command: DeviceFileCommand,
    activity: &DeviceFileActivitySlot,
) -> Result<(), ()> {
    match command {
        DeviceFileCommand::List { path, reply } => {
            let result = tokio::time::timeout(METADATA_TIMEOUT, list_files(client, &path))
                .await
                .unwrap_or_else(|_| Err("device file listing timed out".into()));
            let failed = result.is_err();
            let _ = reply.send(result);
            if failed { Err(()) } else { Ok(()) }
        }
        DeviceFileCommand::Export {
            path,
            destination,
            reply,
        } => {
            let id = activity.start(DeviceFileActivityKind::Export, path.clone());
            let mut progress = TransferProgress::new(activity.clone(), id);
            let result = tokio::time::timeout(
                TRANSFER_TIMEOUT,
                export_file(client, &path, &destination, &mut progress),
            )
            .await
            .unwrap_or_else(|_| Err("device file export timed out".into()));
            let _ = progress.finish();
            let outcome = result.as_ref().map(|_| ()).map_err(Clone::clone);
            activity.finish(id, &outcome);
            let failed = result
                .as_ref()
                .is_err_and(|error| !is_transfer_cancelled(error));
            let _ = reply.send(result);
            if failed { Err(()) } else { Ok(()) }
        }
        DeviceFileCommand::Import {
            directory,
            source,
            reply,
        } => {
            let id = activity.start(DeviceFileActivityKind::Import, directory.clone());
            let mut progress = TransferProgress::new(activity.clone(), id);
            let result = tokio::time::timeout(
                TRANSFER_TIMEOUT,
                import_path(client, &directory, &source, &mut progress),
            )
            .await
            .unwrap_or_else(|_| Err("device file import timed out".into()));
            let _ = progress.finish();
            let outcome = result.as_ref().map(|_| ()).map_err(Clone::clone);
            activity.finish(id, &outcome);
            let failed = result
                .as_ref()
                .is_err_and(|error| !is_transfer_cancelled(error));
            let _ = reply.send(result);
            if failed { Err(()) } else { Ok(()) }
        }
        DeviceFileCommand::CreateDirectory {
            directory,
            name,
            reply,
        } => {
            let result = tokio::time::timeout(
                METADATA_TIMEOUT,
                create_directory(client, &directory, &name),
            )
            .await
            .unwrap_or_else(|_| Err("device directory creation timed out".into()));
            let failed = result.is_err();
            let _ = reply.send(result);
            if failed { Err(()) } else { Ok(()) }
        }
        DeviceFileCommand::Rename { path, name, reply } => {
            let result = tokio::time::timeout(METADATA_TIMEOUT, rename_path(client, &path, &name))
                .await
                .unwrap_or_else(|_| Err("device file rename timed out".into()));
            let failed = result.is_err();
            let _ = reply.send(result);
            if failed { Err(()) } else { Ok(()) }
        }
        DeviceFileCommand::Delete { path, reply } => {
            let result = tokio::time::timeout(METADATA_TIMEOUT, delete_path(client, &path))
                .await
                .unwrap_or_else(|_| Err("device file deletion timed out".into()));
            let failed = result.is_err();
            let _ = reply.send(result);
            if failed { Err(()) } else { Ok(()) }
        }
    }
}

fn reject(command: DeviceFileCommand, error: String) {
    match command {
        DeviceFileCommand::List { reply, .. } => {
            let _ = reply.send(Err(error));
        }
        DeviceFileCommand::Export { reply, .. } => {
            let _ = reply.send(Err(error));
        }
        DeviceFileCommand::Import { reply, .. } => {
            let _ = reply.send(Err(error));
        }
        DeviceFileCommand::CreateDirectory { reply, .. }
        | DeviceFileCommand::Rename { reply, .. }
        | DeviceFileCommand::Delete { reply, .. } => {
            let _ = reply.send(Err(error));
        }
    }
}

async fn list_files(client: &mut AfcClient, path: &str) -> Result<DeviceFileList, String> {
    let path = normalize_path(path, true)?;
    let mut names = client
        .list_dir(path.clone())
        .await
        .map_err(|error| format!("unable to list device files: {error:?}"))?;
    names.retain(|name| name != "." && name != "..");
    names.sort_by_key(|name| name.to_lowercase());
    let truncated = names.len() > MAX_DIRECTORY_ENTRIES;
    names.truncate(MAX_DIRECTORY_ENTRIES);

    let mut entries = Vec::with_capacity(names.len());
    for name in names {
        if validate_name(&name).is_err() {
            tracing::debug!(%path, %name, "ignoring unsafe AFC media entry");
            continue;
        }
        let entry_path = join_path(&path, &name)?;
        let info = match client.get_file_info(entry_path.clone()).await {
            Ok(info) => info,
            Err(error) => {
                tracing::debug!(path = %entry_path, ?error, "AFC media entry disappeared during listing");
                continue;
            }
        };
        let kind = match info.st_ifmt.as_str() {
            "S_IFREG" if info.st_link_target.is_none() => DeviceFileKind::File,
            "S_IFDIR" if info.st_link_target.is_none() => DeviceFileKind::Directory,
            _ => DeviceFileKind::Other,
        };
        entries.push(DeviceFileEntry {
            name,
            path: entry_path,
            kind,
            size_bytes: info.size as u64,
            modified: info.modified.and_utc().to_rfc3339(),
        });
    }
    entries.sort_by(|left, right| {
        kind_order(left.kind)
            .cmp(&kind_order(right.kind))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    Ok(DeviceFileList {
        path,
        entries,
        truncated,
    })
}

async fn export_file(
    client: &mut AfcClient,
    path: &str,
    destination: &Path,
    progress: &mut TransferProgress,
) -> Result<DeviceFileTransfer, String> {
    progress.check_cancelled()?;
    let path = normalize_path(path, false)?;
    let info = client
        .get_file_info(path.clone())
        .await
        .map_err(|error| format!("unable to inspect device file: {error:?}"))?;
    if info.st_link_target.is_some() {
        return Err("symbolic links cannot be exported".into());
    }
    match info.st_ifmt.as_str() {
        "S_IFREG" => {
            progress.set_total(info.size as u64);
            export_regular_file(client, &path, destination, info.size as u64, progress)
                .await
                .map(|bytes_transferred| DeviceFileTransfer {
                    bytes_transferred,
                    files_transferred: 1,
                    directories_transferred: 0,
                })
        }
        "S_IFDIR" => export_directory(client, &path, destination, progress).await,
        _ => Err("only regular device files and directories can be exported".into()),
    }
}

async fn export_regular_file(
    client: &mut AfcClient,
    path: &str,
    destination: &Path,
    expected_size: u64,
    progress: &mut TransferProgress,
) -> Result<u64, String> {
    validate_export_destination(destination).await?;
    progress.check_cancelled()?;
    let temporary = crate::app_documents::temporary_sibling(destination, "device-export")?;
    let result = async {
        let remote = client
            .open(path, AfcFopenMode::RdOnly)
            .await
            .map_err(|error| format!("unable to open device file: {error:?}"))?;
        let mut remote = BufReader::with_capacity(TRANSFER_BUFFER_BYTES, remote);
        let local = tokio::fs::File::create(&temporary)
            .await
            .map_err(|error| format!("unable to create export file: {error}"))?;
        let mut local = BufWriter::with_capacity(TRANSFER_BUFFER_BYTES, local);
        let transfer_result = progress
            .copy(&mut remote, &mut local)
            .await
            .map_err(|error| format!("unable to export device file: {error}"));
        let close_result = remote.into_inner().close().await;
        close_result.map_err(|error| format!("unable to close device file: {error:?}"))?;
        let bytes = transfer_result?;
        if bytes != expected_size {
            return Err("device file changed while it was being exported".into());
        }
        local
            .flush()
            .await
            .map_err(|error| format!("unable to flush export file: {error}"))?;
        progress.check_cancelled()?;
        crate::app_documents::replace_local_file(&temporary, destination).await?;
        progress.file();
        Ok(bytes)
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    result
}

async fn export_directory(
    client: &mut AfcClient,
    path: &str,
    destination: &Path,
    progress: &mut TransferProgress,
) -> Result<DeviceFileTransfer, String> {
    validate_new_directory_destination(destination).await?;
    progress.check_cancelled()?;
    let temporary = crate::app_documents::temporary_sibling(destination, "device-export-dir")?;
    tokio::fs::create_dir(&temporary)
        .await
        .map_err(|error| format!("unable to create temporary export directory: {error}"))?;
    progress.directory();
    let result = async {
        let mut transfer = DeviceFileTransfer {
            directories_transferred: 1,
            ..DeviceFileTransfer::default()
        };
        let mut entries_seen = 0usize;
        let mut pending = vec![(path.to_owned(), temporary.clone(), 0usize)];
        while let Some((remote_directory, local_directory, depth)) = pending.pop() {
            progress.check_cancelled()?;
            if depth >= MAX_TRANSFER_DEPTH {
                return Err("device directory export exceeds the maximum nesting depth".into());
            }
            let names = client
                .list_dir(remote_directory.clone())
                .await
                .map_err(|error| {
                    format!("unable to list device directory during export: {error:?}")
                })?;
            for name in names.into_iter().filter(|name| name != "." && name != "..") {
                progress.check_cancelled()?;
                validate_name(&name)?;
                entries_seen += 1;
                if entries_seen > MAX_TRANSFER_ENTRIES {
                    return Err("device directory export contains too many entries".into());
                }
                let remote_path = join_path(&remote_directory, &name)?;
                let local_path = local_directory.join(&name);
                let info = client
                    .get_file_info(remote_path.clone())
                    .await
                    .map_err(|error| {
                        format!("unable to inspect device entry during export: {error:?}")
                    })?;
                if info.st_link_target.is_some() {
                    return Err(format!("symbolic link cannot be exported: {remote_path}"));
                }
                match info.st_ifmt.as_str() {
                    "S_IFDIR" => {
                        tokio::fs::create_dir(&local_path).await.map_err(|error| {
                            format!("unable to create exported directory: {error}")
                        })?;
                        transfer.directories_transferred += 1;
                        progress.directory();
                        pending.push((remote_path, local_path, depth + 1));
                    }
                    "S_IFREG" => {
                        transfer.bytes_transferred += export_regular_file(
                            client,
                            &remote_path,
                            &local_path,
                            info.size as u64,
                            progress,
                        )
                        .await?;
                        transfer.files_transferred += 1;
                    }
                    _ => {
                        return Err(format!(
                            "unsupported device entry cannot be exported: {remote_path}"
                        ));
                    }
                }
            }
        }
        progress.check_cancelled()?;
        tokio::fs::rename(&temporary, destination)
            .await
            .map_err(|error| format!("unable to finish directory export: {error}"))?;
        Ok(transfer)
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_dir_all(&temporary).await;
    }
    result
}

async fn import_path(
    client: &mut AfcClient,
    directory: &str,
    source: &Path,
    progress: &mut TransferProgress,
) -> Result<DeviceFileEntry, String> {
    progress.check_cancelled()?;
    let directory = normalize_path(directory, true)?;
    let source_metadata = tokio::fs::symlink_metadata(source)
        .await
        .map_err(|error| format!("import source is unavailable: {error}"))?;
    if source_metadata.file_type().is_symlink()
        || (!source_metadata.is_file() && !source_metadata.is_dir())
    {
        return Err("import source must be a regular file or directory".into());
    }
    let source = tokio::fs::canonicalize(source)
        .await
        .map_err(|error| format!("import source is unavailable: {error}"))?;
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "import source has an unsupported file name".to_string())?
        .to_owned();
    validate_name(&name)?;
    ensure_name_available(client, &directory, &name).await?;
    let target = join_path(&directory, &name)?;
    let temporary = join_path(
        &directory,
        &format!(".devicehub-import-{}", uuid::Uuid::new_v4()),
    )?;

    let result = async {
        if source_metadata.is_file() {
            progress.set_total(source_metadata.len());
            upload_regular_file(client, &source, &temporary, progress).await?;
        } else {
            import_directory(client, &source, &temporary, progress).await?;
        }
        progress.check_cancelled()?;
        client
            .rename(temporary.clone(), target.clone())
            .await
            .map_err(|error| format!("unable to finish device file import: {error:?}"))?;
        let info = client
            .get_file_info(target.clone())
            .await
            .map_err(|error| format!("unable to inspect imported device file: {error:?}"))?;
        Ok(entry_from_info(name, target, &info))
    }
    .await;
    if result.is_err() {
        let _ = client.remove_all(temporary).await;
    }
    result
}

async fn upload_regular_file(
    client: &mut AfcClient,
    source: &Path,
    target: &str,
    progress: &mut TransferProgress,
) -> Result<u64, String> {
    progress.check_cancelled()?;
    let metadata = tokio::fs::symlink_metadata(source)
        .await
        .map_err(|error| format!("unable to inspect import source: {error}"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("import source must contain only regular files and directories".into());
    }
    let mut local = tokio::fs::File::open(source)
        .await
        .map_err(|error| format!("unable to open import source: {error}"))?;
    let mut remote = client
        .open(target.to_owned(), AfcFopenMode::WrOnly)
        .await
        .map_err(|error| format!("unable to create device file: {error:?}"))?;
    let transfer_result: Result<u64, String> = async {
        let bytes = progress
            .copy(&mut local, &mut remote)
            .await
            .map_err(|error| format!("unable to import device file: {error}"))?;
        if bytes != metadata.len() {
            return Err("import source changed while it was being transferred".into());
        }
        remote
            .shutdown()
            .await
            .map_err(|error| format!("unable to flush imported device file: {error}"))?;
        progress.check_cancelled()?;
        Ok(bytes)
    }
    .await;
    let close_result = remote.close().await;
    close_result.map_err(|error| format!("unable to close imported device file: {error:?}"))?;
    let bytes = transfer_result?;
    progress.file();
    Ok(bytes)
}

async fn import_directory(
    client: &mut AfcClient,
    source: &Path,
    target: &str,
    progress: &mut TransferProgress,
) -> Result<DeviceFileTransfer, String> {
    progress.check_cancelled()?;
    client
        .mk_dir(target.to_owned())
        .await
        .map_err(|error| format!("unable to create device directory: {error:?}"))?;
    progress.directory();
    let mut transfer = DeviceFileTransfer {
        directories_transferred: 1,
        ..DeviceFileTransfer::default()
    };
    let mut entries_seen = 0usize;
    let mut pending = vec![(source.to_owned(), target.to_owned(), 0usize)];
    while let Some((local_directory, remote_directory, depth)) = pending.pop() {
        progress.check_cancelled()?;
        if depth >= MAX_TRANSFER_DEPTH {
            return Err("import directory exceeds the maximum nesting depth".into());
        }
        let mut entries = tokio::fs::read_dir(&local_directory)
            .await
            .map_err(|error| format!("unable to read import directory: {error}"))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|error| format!("unable to read import directory: {error}"))?
        {
            progress.check_cancelled()?;
            entries_seen += 1;
            if entries_seen > MAX_TRANSFER_ENTRIES {
                return Err("import directory contains too many entries".into());
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "import source has an unsupported file name".to_string())?;
            validate_name(&name)?;
            let remote_path = join_path(&remote_directory, &name)?;
            let metadata = tokio::fs::symlink_metadata(entry.path())
                .await
                .map_err(|error| format!("unable to inspect import entry: {error}"))?;
            if metadata.file_type().is_symlink() {
                return Err("import directories cannot contain symbolic links".into());
            }
            if metadata.is_dir() {
                client
                    .mk_dir(remote_path.clone())
                    .await
                    .map_err(|error| format!("unable to create device directory: {error:?}"))?;
                transfer.directories_transferred += 1;
                progress.directory();
                pending.push((entry.path(), remote_path, depth + 1));
            } else if metadata.is_file() {
                transfer.bytes_transferred +=
                    upload_regular_file(client, &entry.path(), &remote_path, progress).await?;
                transfer.files_transferred += 1;
            } else {
                return Err("import source contains an unsupported entry type".into());
            }
        }
    }
    progress.check_cancelled()?;
    Ok(transfer)
}

async fn create_directory(
    client: &mut AfcClient,
    directory: &str,
    name: &str,
) -> Result<(), String> {
    let directory = normalize_path(directory, true)?;
    validate_name(name)?;
    ensure_name_available(client, &directory, name).await?;
    client
        .mk_dir(join_path(&directory, name)?)
        .await
        .map_err(|error| format!("unable to create device directory: {error:?}"))
}

async fn rename_path(client: &mut AfcClient, path: &str, name: &str) -> Result<(), String> {
    let path = normalize_path(path, false)?;
    validate_name(name)?;
    let parent = parent_path(&path);
    ensure_name_available(client, &parent, name).await?;
    client
        .rename(path, join_path(&parent, name)?)
        .await
        .map_err(|error| format!("unable to rename device file: {error:?}"))
}

async fn delete_path(client: &mut AfcClient, path: &str) -> Result<(), String> {
    let path = normalize_path(path, false)?;
    client
        .remove_all(path)
        .await
        .map_err(|error| format!("unable to delete device file: {error:?}"))
}

async fn ensure_name_available(
    client: &mut AfcClient,
    directory: &str,
    name: &str,
) -> Result<(), String> {
    let entries = client
        .list_dir(directory.to_owned())
        .await
        .map_err(|error| format!("unable to inspect device directory: {error:?}"))?;
    if entries.iter().any(|entry| entry == name) {
        Err("a device file with this name already exists".into())
    } else {
        Ok(())
    }
}

fn entry_from_info(name: String, path: String, info: &idevice::afc::FileInfo) -> DeviceFileEntry {
    let kind = match info.st_ifmt.as_str() {
        "S_IFREG" if info.st_link_target.is_none() => DeviceFileKind::File,
        "S_IFDIR" if info.st_link_target.is_none() => DeviceFileKind::Directory,
        _ => DeviceFileKind::Other,
    };
    DeviceFileEntry {
        name,
        path,
        kind,
        size_bytes: info.size as u64,
        modified: info.modified.and_utc().to_rfc3339(),
    }
}

async fn validate_export_destination(destination: &Path) -> Result<(), String> {
    if !destination.is_absolute() || destination.file_name().is_none() {
        return Err("export destination must be an absolute file path".into());
    }
    let parent = destination
        .parent()
        .ok_or_else(|| "export destination has no parent directory".to_string())?;
    let metadata = tokio::fs::metadata(parent)
        .await
        .map_err(|error| format!("export destination is unavailable: {error}"))?;
    if !metadata.is_dir() {
        return Err("export destination parent is not a directory".into());
    }
    Ok(())
}

async fn validate_new_directory_destination(destination: &Path) -> Result<(), String> {
    validate_export_destination(destination).await?;
    match tokio::fs::symlink_metadata(destination).await {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err("directory export destination already exists".into()),
        Err(error) => Err(format!(
            "unable to inspect directory export destination: {error}"
        )),
    }
}

fn normalize_path(path: &str, allow_root: bool) -> Result<String, String> {
    if path.len() > MAX_PATH_BYTES || path.contains(['\0', '\\']) {
        return Err("invalid device file path".into());
    }
    let components = path
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .map(validate_name)
        .collect::<Result<Vec<_>, _>>()?;
    if components.is_empty() {
        return if allow_root {
            Ok("/".into())
        } else {
            Err("the AFC root cannot be exported".into())
        };
    }
    Ok(format!("/{}", components.join("/")))
}

fn validate_name(name: &str) -> Result<&str, String> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.len() > 255
        || name.contains(['/', '\\', '\0'])
    {
        Err("invalid device file name".into())
    } else {
        Ok(name)
    }
}

fn join_path(directory: &str, name: &str) -> Result<String, String> {
    validate_name(name)?;
    normalize_path(
        &if directory == "/" {
            format!("/{name}")
        } else {
            format!("{directory}/{name}")
        },
        false,
    )
}

fn parent_path(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
        .unwrap_or("/")
        .to_owned()
}

fn kind_order(kind: DeviceFileKind) -> u8 {
    match kind {
        DeviceFileKind::Directory => 0,
        DeviceFileKind::File => 1,
        DeviceFileKind::Other => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_afc_paths_are_bounded_and_cannot_traverse() {
        assert_eq!(normalize_path("/", true).unwrap(), "/");
        assert_eq!(
            normalize_path("/DCIM/100APPLE", false).unwrap(),
            "/DCIM/100APPLE"
        );
        assert_eq!(
            join_path("/DCIM", "IMG_0001.HEIC").unwrap(),
            "/DCIM/IMG_0001.HEIC"
        );
        for path in [
            "..",
            "/DCIM/../escape",
            r"/DCIM\escape",
            "/DCIM/./file",
            "/a\0b",
        ] {
            assert!(normalize_path(path, true).is_err(), "accepted {path:?}");
        }
        assert!(normalize_path("/", false).is_err());
    }

    #[test]
    fn public_afc_names_reject_path_components() {
        for name in ["", ".", "..", "a/b", r"a\b", "a\0b"] {
            assert!(validate_name(name).is_err(), "accepted {name:?}");
        }
    }

    #[test]
    fn transfer_activity_tracks_progress_and_completion() {
        let slot = DeviceFileActivitySlot::default();
        let id = slot.start(DeviceFileActivityKind::Export, "/DCIM/photo.heic".into());
        slot.set_total(id, 100);
        slot.update(
            id,
            DeviceFileTransfer {
                bytes_transferred: 42,
                files_transferred: 0,
                directories_transferred: 0,
            },
        );
        let running = slot.get();
        assert_eq!(running.state, DeviceFileActivityState::Running);
        assert_eq!(running.bytes_transferred, 42);
        assert_eq!(running.bytes_total, Some(100));

        slot.finish(id, &Ok(()));
        let completed = slot.get();
        assert_eq!(completed.state, DeviceFileActivityState::Succeeded);
        assert_eq!(completed.bytes_transferred, 100);
    }

    #[test]
    fn stale_transfer_activity_updates_are_ignored() {
        let slot = DeviceFileActivitySlot::default();
        let stale = slot.start(DeviceFileActivityKind::Export, "/old".into());
        let current = slot.start(DeviceFileActivityKind::Import, "/new".into());
        slot.update(
            stale,
            DeviceFileTransfer {
                bytes_transferred: 99,
                ..DeviceFileTransfer::default()
            },
        );
        slot.finish(stale, &Err("stale failure".into()));

        let view = slot.get();
        assert_eq!(view.id, current);
        assert_eq!(view.kind, Some(DeviceFileActivityKind::Import));
        assert_eq!(view.state, DeviceFileActivityState::Running);
        assert_eq!(view.bytes_transferred, 0);
        assert_eq!(view.error, None);
    }

    #[test]
    fn transfer_cancellation_is_scoped_to_the_running_activity() {
        let slot = DeviceFileActivitySlot::default();
        assert!(!slot.cancel());

        let cancelled = slot.start(DeviceFileActivityKind::Export, "/old".into());
        assert!(slot.cancel());
        assert!(slot.is_cancelled(cancelled));
        slot.finish(cancelled, &Err(TRANSFER_CANCELLED.into()));
        assert_eq!(slot.get().state, DeviceFileActivityState::Cancelled);
        assert!(!slot.cancel());

        let current = slot.start(DeviceFileActivityKind::Import, "/new".into());
        assert!(!slot.is_cancelled(cancelled));
        assert!(!slot.is_cancelled(current));
    }

    #[tokio::test]
    async fn transfer_copy_stops_when_cancelled() {
        let slot = DeviceFileActivitySlot::default();
        let id = slot.start(DeviceFileActivityKind::Export, "/DCIM/photo.heic".into());
        let mut progress = TransferProgress::new(slot.clone(), id);
        assert!(slot.cancel());

        let error = progress
            .copy(&mut tokio::io::empty(), &mut tokio::io::sink())
            .await
            .unwrap_err();
        assert_eq!(error, TRANSFER_CANCELLED);
    }

    #[tokio::test]
    async fn export_destination_must_be_an_absolute_file_path() {
        assert!(
            validate_export_destination(Path::new("photo.heic"))
                .await
                .is_err()
        );
        assert!(validate_export_destination(Path::new("/")).await.is_err());
        let destination = std::env::temp_dir().join("devicehub-mask-afc-export.heic");
        validate_export_destination(&destination).await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires a connected physical device"]
    async fn lists_public_afc_root_from_hardware() {
        use idevice::usbmuxd::{UsbmuxdAddr, UsbmuxdConnection};

        let mut usbmuxd = UsbmuxdConnection::default().await.unwrap();
        let device = usbmuxd
            .get_devices()
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("no connected device");
        let provider = device.to_provider(UsbmuxdAddr::default(), "devicehub-mask-afc-test");
        let mut client = AfcClient::connect(&provider).await.unwrap();
        let listing = list_files(&mut client, "/").await.unwrap();
        println!("listed {} public AFC root entries", listing.entries.len());
    }
}
