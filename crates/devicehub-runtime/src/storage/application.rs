//! Sandboxed application storage access through House Arrest and AFC.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use idevice::afc::AfcClient;
use idevice::afc::opcode::AfcFopenMode;
use idevice::house_arrest::HouseArrestClient;
use idevice::provider::IdeviceProvider;
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use idevice::{IdeviceService, RsdService};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, watch};

use devicehub_core::ConnKind;

use super::{HostFileIo, HostFileKind};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const METADATA_TIMEOUT: Duration = Duration::from_secs(15);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const MAX_DIRECTORY_ENTRIES: usize = 500;
const MAX_PATH_BYTES: usize = 1_024;
const TRANSFER_BUFFER_BYTES: usize = 64 * 1024;
const MAX_TRANSFER_ENTRIES: usize = 100_000;
const MAX_TRANSFER_DEPTH: usize = 64;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
pub const TRANSFER_CANCELLED: &str = "application storage transfer cancelled";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppStorageScope {
    #[default]
    Documents,
    Container,
}

#[derive(Debug, Clone, Serialize)]
pub struct AppDocumentEntry {
    pub name: String,
    pub path: String,
    pub kind: AppDocumentKind,
    pub size_bytes: u64,
    pub modified: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppDocumentKind {
    File,
    Directory,
    Other,
}

#[derive(Debug, Clone, Serialize)]
pub struct AppDocumentList {
    pub path: String,
    pub entries: Vec<AppDocumentEntry>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct AppDocumentTransfer {
    pub bytes_transferred: u64,
    pub files_transferred: u64,
    pub directories_transferred: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppDocumentActivityKind {
    Export,
    Import,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppDocumentActivityState {
    #[default]
    Idle,
    Running,
    Succeeded,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AppDocumentActivityView {
    pub id: u64,
    pub bundle_id: Option<String>,
    pub scope: Option<AppStorageScope>,
    pub kind: Option<AppDocumentActivityKind>,
    pub state: AppDocumentActivityState,
    pub path: Option<String>,
    pub bytes_transferred: u64,
    pub bytes_total: Option<u64>,
    pub files_transferred: u64,
    pub directories_transferred: u64,
    pub error: Option<String>,
}

#[derive(Clone, Default)]
pub struct AppDocumentActivitySlot {
    view: Arc<Mutex<AppDocumentActivityView>>,
    active_id: Arc<AtomicU64>,
    cancelled: Arc<AtomicBool>,
}

impl AppDocumentActivitySlot {
    pub(crate) fn start(
        &self,
        bundle_id: &str,
        scope: AppStorageScope,
        kind: AppDocumentActivityKind,
        path: String,
        bytes_total: Option<u64>,
    ) -> u64 {
        let mut view = self
            .view
            .lock()
            .expect("app document activity lock poisoned");
        let id = view.id.wrapping_add(1).max(1);
        self.cancelled.store(false, Ordering::Release);
        self.active_id.store(id, Ordering::Release);
        *view = AppDocumentActivityView {
            id,
            bundle_id: Some(bundle_id.to_owned()),
            scope: Some(scope),
            kind: Some(kind),
            state: AppDocumentActivityState::Running,
            path: Some(path),
            bytes_total,
            ..AppDocumentActivityView::default()
        };
        id
    }

    fn update(&self, id: u64, transfer: AppDocumentTransfer) {
        let mut view = self
            .view
            .lock()
            .expect("app document activity lock poisoned");
        if view.id == id && view.state == AppDocumentActivityState::Running {
            view.bytes_transferred = transfer.bytes_transferred;
            view.files_transferred = transfer.files_transferred;
            view.directories_transferred = transfer.directories_transferred;
        }
    }

    fn set_total(&self, id: u64, bytes_total: u64) {
        let mut view = self
            .view
            .lock()
            .expect("app document activity lock poisoned");
        if view.id == id && view.state == AppDocumentActivityState::Running {
            view.bytes_total = Some(bytes_total);
        }
    }

    fn finish(&self, id: u64, result: &Result<(), String>) {
        let mut view = self
            .view
            .lock()
            .expect("app document activity lock poisoned");
        if view.id != id || view.state != AppDocumentActivityState::Running {
            return;
        }
        match result {
            Ok(()) => {
                view.state = AppDocumentActivityState::Succeeded;
                if let Some(total) = view.bytes_total {
                    view.bytes_transferred = total;
                }
            }
            Err(error) if is_transfer_cancelled(error) => {
                view.state = AppDocumentActivityState::Cancelled;
            }
            Err(error) => {
                view.state = AppDocumentActivityState::Failed;
                view.error = Some(error.chars().take(512).collect());
            }
        }
        self.active_id.store(0, Ordering::Release);
    }

    pub fn get(&self, bundle_id: &str) -> AppDocumentActivityView {
        let view = self
            .view
            .lock()
            .expect("app document activity lock poisoned");
        if view.bundle_id.as_deref() == Some(bundle_id) {
            view.clone()
        } else {
            AppDocumentActivityView::default()
        }
    }

    pub fn cancel(&self, bundle_id: &str) -> bool {
        let view = self
            .view
            .lock()
            .expect("app document activity lock poisoned");
        if view.state != AppDocumentActivityState::Running
            || view.bundle_id.as_deref() != Some(bundle_id)
        {
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
            .expect("app document activity lock poisoned");
        self.cancelled.store(false, Ordering::Release);
        self.active_id.store(0, Ordering::Release);
        *view = AppDocumentActivityView::default();
    }
}

pub fn is_transfer_cancelled(error: &str) -> bool {
    error.contains(TRANSFER_CANCELLED)
}

struct TransferProgress {
    slot: AppDocumentActivitySlot,
    id: u64,
    transfer: AppDocumentTransfer,
    last_published: Instant,
    buffer: Vec<u8>,
}

impl TransferProgress {
    fn new(slot: AppDocumentActivitySlot, id: u64) -> Self {
        Self {
            slot,
            id,
            transfer: AppDocumentTransfer::default(),
            last_published: Instant::now(),
            buffer: vec![0u8; TRANSFER_BUFFER_BYTES],
        }
    }

    fn bytes(&mut self, bytes: u64) {
        self.transfer.bytes_transferred = self.transfer.bytes_transferred.saturating_add(bytes);
        self.publish(false);
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
            self.bytes(read);
        }
    }

    fn finish(mut self) -> AppDocumentTransfer {
        self.publish(true);
        self.transfer
    }
}

#[derive(Debug)]
pub enum AppDocumentCommand<Path> {
    List {
        bundle_id: String,
        scope: AppStorageScope,
        path: String,
        reply: oneshot::Sender<Result<AppDocumentList, String>>,
    },
    Export {
        bundle_id: String,
        scope: AppStorageScope,
        path: String,
        destination: Path,
        reply: oneshot::Sender<Result<AppDocumentTransfer, String>>,
    },
    Import {
        bundle_id: String,
        scope: AppStorageScope,
        directory: String,
        source: Path,
        reply: oneshot::Sender<Result<AppDocumentEntry, String>>,
    },
    CreateDirectory {
        bundle_id: String,
        scope: AppStorageScope,
        directory: String,
        name: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Rename {
        bundle_id: String,
        scope: AppStorageScope,
        path: String,
        name: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Delete {
        bundle_id: String,
        scope: AppStorageScope,
        path: String,
        recursive: bool,
        reply: oneshot::Sender<Result<(), String>>,
    },
}

pub(crate) struct AppStorageTransport {
    provider: Arc<dyn IdeviceProvider>,
    connection: ConnKind,
    adapter: AdapterHandle,
    handshake: RsdHandshake,
}

impl AppStorageTransport {
    pub(crate) fn new(
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

    async fn connect(
        &mut self,
        bundle_id: &str,
        scope: AppStorageScope,
    ) -> Result<AfcClient, String> {
        validate_bundle_id(bundle_id)?;
        let mut failures = Vec::new();
        if self.connection == ConnKind::Usb {
            let direct = tokio::time::timeout(CONNECT_TIMEOUT, async {
                let client = HouseArrestClient::connect(self.provider.as_ref()).await?;
                vend_storage(client, bundle_id, scope).await
            })
            .await;
            match direct {
                Ok(Ok(client)) => {
                    tracing::debug!(
                        ?scope,
                        transport = "lockdown-usb",
                        "House Arrest storage connected"
                    );
                    return Ok(client);
                }
                Ok(Err(error)) => {
                    tracing::debug!(?scope, ?error, "USB House Arrest failed; trying RSD");
                    failures.push(format!("USB lockdown: {error:?}"));
                }
                Err(_) => {
                    tracing::debug!(?scope, "USB House Arrest timed out; trying RSD");
                    failures.push("USB lockdown: connection timed out".into());
                }
            }
        }

        let remote = tokio::time::timeout(CONNECT_TIMEOUT, async {
            let client =
                HouseArrestClient::connect_rsd(&mut self.adapter, &mut self.handshake).await?;
            vend_storage(client, bundle_id, scope).await
        })
        .await;
        match remote {
            Ok(Ok(client)) => {
                tracing::debug!(
                    ?scope,
                    transport = "coredevice-rsd",
                    "House Arrest storage connected"
                );
                Ok(client)
            }
            Ok(Err(error)) => {
                failures.push(format!("CoreDevice RSD: {error:?}"));
                Err(storage_unavailable(scope, &failures))
            }
            Err(_) => {
                failures.push("CoreDevice RSD: connection timed out".into());
                Err(storage_unavailable(scope, &failures))
            }
        }
    }
}

async fn vend_storage(
    client: HouseArrestClient,
    bundle_id: &str,
    scope: AppStorageScope,
) -> Result<AfcClient, idevice::IdeviceError> {
    match scope {
        AppStorageScope::Documents => client.vend_documents(bundle_id.to_owned()).await,
        AppStorageScope::Container => client.vend_container(bundle_id.to_owned()).await,
    }
}

fn storage_unavailable(scope: AppStorageScope, failures: &[String]) -> String {
    if scope == AppStorageScope::Container
        && failures
            .iter()
            .any(|failure| failure.contains("InstallationLookupFailed"))
    {
        return "application container is unavailable; verify the app is installed and developer-signed"
            .into();
    }
    let name = match scope {
        AppStorageScope::Documents => "application Documents",
        AppStorageScope::Container => "application container",
    };
    format!("{name} is unavailable: {}", failures.join("; "))
}

pub(crate) async fn serve<FileIo>(
    mut transport: AppStorageTransport,
    mut commands: mpsc::Receiver<AppDocumentCommand<FileIo::Path>>,
    activity: AppDocumentActivitySlot,
    file_io: FileIo,
    mut shutdown: watch::Receiver<bool>,
) where
    FileIo: HostFileIo,
{
    activity.reset();
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            command = commands.recv() => {
                let Some(command) = command else { break };
                handle(command, &mut transport, &activity, &file_io).await;
            }
        }
    }
    activity.reset();
}

async fn handle<FileIo>(
    command: AppDocumentCommand<FileIo::Path>,
    transport: &mut AppStorageTransport,
    activity: &AppDocumentActivitySlot,
    file_io: &FileIo,
) where
    FileIo: HostFileIo,
{
    match command {
        AppDocumentCommand::List {
            bundle_id,
            scope,
            path,
            reply,
        } => {
            let result = tokio::time::timeout(
                METADATA_TIMEOUT,
                list_documents(transport, &bundle_id, scope, &path),
            )
            .await
            .unwrap_or_else(|_| Err("application document listing timed out".into()));
            let _ = reply.send(result);
        }
        AppDocumentCommand::Export {
            bundle_id,
            scope,
            path,
            destination,
            reply,
        } => {
            let id = activity.start(
                &bundle_id,
                scope,
                AppDocumentActivityKind::Export,
                path.clone(),
                None,
            );
            let mut progress = TransferProgress::new(activity.clone(), id);
            let result = tokio::time::timeout(
                TRANSFER_TIMEOUT,
                export_path(
                    transport,
                    &bundle_id,
                    scope,
                    &path,
                    &destination,
                    &mut progress,
                    file_io,
                ),
            )
            .await
            .unwrap_or_else(|_| Err("application document export timed out".into()));
            let _ = progress.finish();
            let outcome = result.as_ref().map(|_| ()).map_err(Clone::clone);
            activity.finish(id, &outcome);
            let _ = reply.send(result);
        }
        AppDocumentCommand::Import {
            bundle_id,
            scope,
            directory,
            source,
            reply,
        } => {
            let id = activity.start(
                &bundle_id,
                scope,
                AppDocumentActivityKind::Import,
                directory.clone(),
                None,
            );
            let mut progress = TransferProgress::new(activity.clone(), id);
            let result = tokio::time::timeout(
                TRANSFER_TIMEOUT,
                import_path(
                    transport,
                    &bundle_id,
                    scope,
                    &directory,
                    &source,
                    &mut progress,
                    file_io,
                ),
            )
            .await
            .unwrap_or_else(|_| Err("application document upload timed out".into()));
            let _ = progress.finish();
            let outcome = result.as_ref().map(|_| ()).map_err(Clone::clone);
            activity.finish(id, &outcome);
            let _ = reply.send(result);
        }
        AppDocumentCommand::CreateDirectory {
            bundle_id,
            scope,
            directory,
            name,
            reply,
        } => {
            let result = tokio::time::timeout(
                METADATA_TIMEOUT,
                create_directory(transport, &bundle_id, scope, &directory, &name),
            )
            .await
            .unwrap_or_else(|_| Err("application directory creation timed out".into()));
            let _ = reply.send(result);
        }
        AppDocumentCommand::Rename {
            bundle_id,
            scope,
            path,
            name,
            reply,
        } => {
            let result = tokio::time::timeout(
                METADATA_TIMEOUT,
                rename_document(transport, &bundle_id, scope, &path, &name),
            )
            .await
            .unwrap_or_else(|_| Err("application document rename timed out".into()));
            let _ = reply.send(result);
        }
        AppDocumentCommand::Delete {
            bundle_id,
            scope,
            path,
            recursive,
            reply,
        } => {
            let result = tokio::time::timeout(
                TRANSFER_TIMEOUT,
                delete_document(transport, &bundle_id, scope, &path, recursive),
            )
            .await
            .unwrap_or_else(|_| Err("application document deletion timed out".into()));
            let _ = reply.send(result);
        }
    }
}

async fn list_documents(
    transport: &mut AppStorageTransport,
    bundle_id: &str,
    scope: AppStorageScope,
    path: &str,
) -> Result<AppDocumentList, String> {
    let path = normalize_path(path, true)?;
    let mut client = transport.connect(bundle_id, scope).await?;
    ensure_no_symlink_components(&mut client, scope, &path).await?;
    let device_path = afc_path(scope, &path);
    let mut names = client
        .list_dir(device_path)
        .await
        .map_err(|error| format!("unable to list application storage: {error:?}"))?;
    names.retain(|name| name != "." && name != "..");
    names.sort_by_key(|name| name.to_lowercase());
    let truncated = names.len() > MAX_DIRECTORY_ENTRIES;
    names.truncate(MAX_DIRECTORY_ENTRIES);

    let mut entries = Vec::with_capacity(names.len());
    for name in names {
        if validate_name(&name).is_err() {
            tracing::debug!(%bundle_id, %path, %name, "ignoring unsafe AFC directory entry");
            continue;
        }
        let entry_path = join_path(&path, &name)?;
        let info = match client.get_file_info(afc_path(scope, &entry_path)).await {
            Ok(info) => info,
            Err(error) => {
                tracing::debug!(%bundle_id, path = %entry_path, ?error, "AFC entry disappeared during listing");
                continue;
            }
        };
        entries.push(entry_from_info(name, entry_path, &info));
    }
    entries.sort_by(|left, right| {
        document_kind_order(left.kind)
            .cmp(&document_kind_order(right.kind))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    Ok(AppDocumentList {
        path,
        entries,
        truncated,
    })
}

fn entry_from_info(name: String, path: String, info: &idevice::afc::FileInfo) -> AppDocumentEntry {
    let kind = match info.st_ifmt.as_str() {
        "S_IFREG" if info.st_link_target.is_none() => AppDocumentKind::File,
        "S_IFDIR" if info.st_link_target.is_none() => AppDocumentKind::Directory,
        _ => AppDocumentKind::Other,
    };
    AppDocumentEntry {
        name,
        path,
        kind,
        size_bytes: info.size as u64,
        modified: info.modified.and_utc().to_rfc3339(),
    }
}

fn document_kind_order(kind: AppDocumentKind) -> u8 {
    match kind {
        AppDocumentKind::Directory => 0,
        AppDocumentKind::File => 1,
        AppDocumentKind::Other => 2,
    }
}

async fn export_path<FileIo>(
    transport: &mut AppStorageTransport,
    bundle_id: &str,
    scope: AppStorageScope,
    path: &str,
    destination: &FileIo::Path,
    progress: &mut TransferProgress,
    file_io: &FileIo,
) -> Result<AppDocumentTransfer, String>
where
    FileIo: HostFileIo,
{
    progress.check_cancelled()?;
    let path = normalize_path(path, false)?;
    let mut client = transport.connect(bundle_id, scope).await?;
    ensure_no_symlink_components(&mut client, scope, &path).await?;
    let info = client
        .get_file_info(afc_path(scope, &path))
        .await
        .map_err(|error| format!("unable to inspect application storage item: {error:?}"))?;
    if info.st_link_target.is_some() {
        return Err("symbolic links cannot be exported".into());
    }
    match info.st_ifmt.as_str() {
        "S_IFREG" => {
            progress.set_total(info.size as u64);
            export_regular_file(
                &mut client,
                scope,
                &path,
                destination,
                info.size as u64,
                progress,
                file_io,
            )
            .await
            .map(|bytes_transferred| AppDocumentTransfer {
                bytes_transferred,
                files_transferred: 1,
                directories_transferred: 0,
            })
        }
        "S_IFDIR" => {
            export_directory(&mut client, scope, &path, destination, progress, file_io).await
        }
        _ => Err("only regular application files and directories can be exported".into()),
    }
}

async fn export_regular_file<FileIo>(
    client: &mut AfcClient,
    scope: AppStorageScope,
    path: &str,
    destination: &FileIo::Path,
    expected_size: u64,
    progress: &mut TransferProgress,
    file_io: &FileIo,
) -> Result<u64, String>
where
    FileIo: HostFileIo,
{
    file_io.validate_export_file(destination).await?;
    progress.check_cancelled()?;
    let temporary = file_io.temporary_sibling(destination, "app-export")?;
    let result = async {
        let remote = client
            .open(afc_path(scope, path), AfcFopenMode::RdOnly)
            .await
            .map_err(|error| format!("unable to open application file: {error:?}"))?;
        let mut remote = BufReader::with_capacity(TRANSFER_BUFFER_BYTES, remote);
        let mut local = file_io.create_writer(&temporary).await?;
        let transfer_result = progress
            .copy(&mut remote, &mut local)
            .await
            .map_err(|error| format!("unable to export application file: {error}"));
        let close_result = remote.into_inner().close().await;
        close_result.map_err(|error| format!("unable to close application file: {error:?}"))?;
        let bytes = transfer_result?;
        if bytes != expected_size {
            return Err("application file changed while it was being exported".into());
        }
        local
            .flush()
            .await
            .map_err(|error| format!("unable to flush export file: {error}"))?;
        progress.check_cancelled()?;
        local.sync().await?;
        file_io.replace_file(&temporary, destination).await?;
        progress.file();
        Ok(bytes)
    }
    .await;
    if result.is_err() {
        let _ = file_io.remove_file(&temporary).await;
    }
    result
}

async fn export_directory<FileIo>(
    client: &mut AfcClient,
    scope: AppStorageScope,
    path: &str,
    destination: &FileIo::Path,
    progress: &mut TransferProgress,
    file_io: &FileIo,
) -> Result<AppDocumentTransfer, String>
where
    FileIo: HostFileIo,
{
    file_io.validate_new_export_directory(destination).await?;
    progress.check_cancelled()?;
    let temporary = file_io.temporary_sibling(destination, "app-export-dir")?;
    file_io.create_directory(&temporary).await?;
    progress.directory();
    let result = async {
        let mut transfer = AppDocumentTransfer {
            directories_transferred: 1,
            ..AppDocumentTransfer::default()
        };
        let mut entries_seen = 0usize;
        let mut pending = vec![(path.to_owned(), temporary.clone(), 0usize)];
        while let Some((remote_directory, local_directory, depth)) = pending.pop() {
            progress.check_cancelled()?;
            if depth >= MAX_TRANSFER_DEPTH {
                return Err(
                    "application directory export exceeds the maximum nesting depth".into(),
                );
            }
            let names = client
                .list_dir(afc_path(scope, &remote_directory))
                .await
                .map_err(|error| {
                    format!("unable to list application directory during export: {error:?}")
                })?;
            for name in names.into_iter().filter(|name| name != "." && name != "..") {
                progress.check_cancelled()?;
                validate_name(&name)?;
                entries_seen += 1;
                if entries_seen > MAX_TRANSFER_ENTRIES {
                    return Err("application directory export contains too many entries".into());
                }
                let remote_path = join_path(&remote_directory, &name)?;
                let local_path = file_io.child(&local_directory, &name)?;
                let info = client
                    .get_file_info(afc_path(scope, &remote_path))
                    .await
                    .map_err(|error| {
                        format!("unable to inspect application entry during export: {error:?}")
                    })?;
                if info.st_link_target.is_some() {
                    return Err(format!("symbolic link cannot be exported: {remote_path}"));
                }
                match info.st_ifmt.as_str() {
                    "S_IFDIR" => {
                        file_io.create_directory(&local_path).await?;
                        transfer.directories_transferred += 1;
                        progress.directory();
                        pending.push((remote_path, local_path, depth + 1));
                    }
                    "S_IFREG" => {
                        transfer.bytes_transferred += export_regular_file(
                            client,
                            scope,
                            &remote_path,
                            &local_path,
                            info.size as u64,
                            progress,
                            file_io,
                        )
                        .await?;
                        transfer.files_transferred += 1;
                    }
                    _ => {
                        return Err(format!(
                            "unsupported application entry cannot be exported: {remote_path}"
                        ));
                    }
                }
            }
        }
        progress.check_cancelled()?;
        file_io.rename(&temporary, destination).await?;
        Ok(transfer)
    }
    .await;
    if result.is_err() {
        let _ = file_io.remove_tree(&temporary).await;
    }
    result
}

async fn import_path<FileIo>(
    transport: &mut AppStorageTransport,
    bundle_id: &str,
    scope: AppStorageScope,
    directory: &str,
    source: &FileIo::Path,
    progress: &mut TransferProgress,
    file_io: &FileIo,
) -> Result<AppDocumentEntry, String>
where
    FileIo: HostFileIo,
{
    progress.check_cancelled()?;
    let directory = normalize_path(directory, true)?;
    let source_metadata = file_io.metadata(source).await?;
    if !matches!(
        source_metadata.kind,
        HostFileKind::File | HostFileKind::Directory
    ) {
        return Err("import source must be a regular file or directory".into());
    }
    let source = file_io.canonicalize(source).await?;
    let name = file_io.file_name(&source)?;
    validate_name(&name)?;
    let target = join_path(&directory, &name)?;
    let mut client = transport.connect(bundle_id, scope).await?;
    ensure_no_symlink_components(&mut client, scope, &directory).await?;
    ensure_name_available(&mut client, scope, &directory, &name).await?;

    let temporary_name = format!(".devicehub-import-{}", uuid::Uuid::new_v4());
    let temporary = join_path(&directory, &temporary_name)?;
    let result = async {
        if source_metadata.kind == HostFileKind::File {
            progress.set_total(source_metadata.len);
            upload_regular_file(&mut client, scope, &source, &temporary, progress, file_io).await?;
        } else {
            import_directory(&mut client, scope, &source, &temporary, progress, file_io).await?;
        }
        progress.check_cancelled()?;
        client
            .rename(afc_path(scope, &temporary), afc_path(scope, &target))
            .await
            .map_err(|error| format!("unable to finish application storage import: {error:?}"))?;
        let info = client
            .get_file_info(afc_path(scope, &target))
            .await
            .map_err(|error| format!("unable to inspect imported application item: {error:?}"))?;
        Ok(entry_from_info(name, target, &info))
    }
    .await;
    if result.is_err() {
        let _ = client.remove_all(afc_path(scope, &temporary)).await;
    }
    result
}

async fn upload_regular_file<FileIo>(
    client: &mut AfcClient,
    scope: AppStorageScope,
    source: &FileIo::Path,
    target: &str,
    progress: &mut TransferProgress,
    file_io: &FileIo,
) -> Result<u64, String>
where
    FileIo: HostFileIo,
{
    progress.check_cancelled()?;
    let metadata = file_io.metadata(source).await?;
    if metadata.kind != HostFileKind::File {
        return Err("import source must contain only regular files and directories".into());
    }
    let mut local = file_io.open_reader(source).await?;
    let mut remote = client
        .open(afc_path(scope, target), AfcFopenMode::WrOnly)
        .await
        .map_err(|error| format!("unable to create application file: {error:?}"))?;
    let transfer_result: Result<u64, String> = async {
        let bytes = progress
            .copy(&mut local, &mut remote)
            .await
            .map_err(|error| format!("unable to import application file: {error}"))?;
        if bytes != metadata.len {
            return Err("import source changed while it was being transferred".into());
        }
        remote
            .shutdown()
            .await
            .map_err(|error| format!("unable to flush imported application file: {error}"))?;
        progress.check_cancelled()?;
        Ok(bytes)
    }
    .await;
    let close_result = remote.close().await;
    close_result
        .map_err(|error| format!("unable to close imported application file: {error:?}"))?;
    let bytes = transfer_result?;
    progress.file();
    Ok(bytes)
}

async fn import_directory<FileIo>(
    client: &mut AfcClient,
    scope: AppStorageScope,
    source: &FileIo::Path,
    target: &str,
    progress: &mut TransferProgress,
    file_io: &FileIo,
) -> Result<AppDocumentTransfer, String>
where
    FileIo: HostFileIo,
{
    progress.check_cancelled()?;
    client
        .mk_dir(afc_path(scope, target))
        .await
        .map_err(|error| format!("unable to create application directory: {error:?}"))?;
    progress.directory();
    let mut transfer = AppDocumentTransfer {
        directories_transferred: 1,
        ..AppDocumentTransfer::default()
    };
    let mut entries_seen = 0usize;
    let mut pending = vec![(source.to_owned(), target.to_owned(), 0usize)];
    while let Some((local_directory, remote_directory, depth)) = pending.pop() {
        progress.check_cancelled()?;
        if depth >= MAX_TRANSFER_DEPTH {
            return Err("import directory exceeds the maximum nesting depth".into());
        }
        for entry in file_io.read_directory(&local_directory).await? {
            progress.check_cancelled()?;
            entries_seen += 1;
            if entries_seen > MAX_TRANSFER_ENTRIES {
                return Err("import directory contains too many entries".into());
            }
            let name = entry.name;
            validate_name(&name)?;
            let remote_path = join_path(&remote_directory, &name)?;
            if entry.metadata.kind == HostFileKind::Symlink {
                return Err("import directories cannot contain symbolic links".into());
            }
            if entry.metadata.kind == HostFileKind::Directory {
                client
                    .mk_dir(afc_path(scope, &remote_path))
                    .await
                    .map_err(|error| {
                        format!("unable to create application directory: {error:?}")
                    })?;
                transfer.directories_transferred += 1;
                progress.directory();
                pending.push((entry.path, remote_path, depth + 1));
            } else if entry.metadata.kind == HostFileKind::File {
                transfer.bytes_transferred += upload_regular_file(
                    client,
                    scope,
                    &entry.path,
                    &remote_path,
                    progress,
                    file_io,
                )
                .await?;
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
    transport: &mut AppStorageTransport,
    bundle_id: &str,
    scope: AppStorageScope,
    directory: &str,
    name: &str,
) -> Result<(), String> {
    let directory = normalize_path(directory, true)?;
    validate_name(name)?;
    let target = join_path(&directory, name)?;
    let mut client = transport.connect(bundle_id, scope).await?;
    ensure_no_symlink_components(&mut client, scope, &directory).await?;
    ensure_name_available(&mut client, scope, &directory, name).await?;
    client
        .mk_dir(afc_path(scope, &target))
        .await
        .map_err(|error| format!("unable to create application directory: {error:?}"))
}

async fn rename_document(
    transport: &mut AppStorageTransport,
    bundle_id: &str,
    scope: AppStorageScope,
    path: &str,
    name: &str,
) -> Result<(), String> {
    let path = normalize_path(path, false)?;
    validate_name(name)?;
    let parent = parent_path(&path);
    let target = join_path(&parent, name)?;
    let mut client = transport.connect(bundle_id, scope).await?;
    ensure_no_symlink_components(&mut client, scope, &path).await?;
    ensure_name_available(&mut client, scope, &parent, name).await?;
    client
        .rename(afc_path(scope, &path), afc_path(scope, &target))
        .await
        .map_err(|error| format!("unable to rename application document: {error:?}"))
}

async fn delete_document(
    transport: &mut AppStorageTransport,
    bundle_id: &str,
    scope: AppStorageScope,
    path: &str,
    recursive: bool,
) -> Result<(), String> {
    let path = normalize_path(path, false)?;
    let mut client = transport.connect(bundle_id, scope).await?;
    ensure_no_symlink_components(&mut client, scope, &path).await?;
    let info = client
        .get_file_info(afc_path(scope, &path))
        .await
        .map_err(|error| format!("unable to inspect application storage item: {error:?}"))?;
    if info.st_link_target.is_some() {
        return Err("symbolic links cannot be deleted".into());
    }
    match info.st_ifmt.as_str() {
        "S_IFREG" => remove_checked(&mut client, scope, &path, AppDocumentKind::File).await,
        "S_IFDIR" if recursive => {
            let plan = build_recursive_delete_plan(&mut client, scope, &path).await?;
            for entry in plan {
                remove_checked(&mut client, scope, &entry.path, entry.kind).await?;
            }
            Ok(())
        }
        "S_IFDIR" => client
            .remove(afc_path(scope, &path))
            .await
            .map_err(|error| {
                format!("unable to delete application directory; it must be empty: {error:?}")
            }),
        _ => Err("unsupported application storage item cannot be deleted".into()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeletePlanEntry {
    path: String,
    kind: AppDocumentKind,
}

async fn build_recursive_delete_plan(
    client: &mut AfcClient,
    scope: AppStorageScope,
    root: &str,
) -> Result<Vec<DeletePlanEntry>, String> {
    let mut plan = Vec::new();
    let mut entries_seen = 0usize;
    let mut pending = vec![(root.to_owned(), false, 0usize)];
    while let Some((path, expanded, depth)) = pending.pop() {
        if expanded {
            plan.push(DeletePlanEntry {
                path,
                kind: AppDocumentKind::Directory,
            });
            continue;
        }
        let info = client
            .get_file_info(afc_path(scope, &path))
            .await
            .map_err(|error| {
                format!("unable to inspect application entry before deletion: {error:?}")
            })?;
        if info.st_link_target.is_some() {
            return Err(format!(
                "symbolic link cannot be recursively deleted: {path}"
            ));
        }
        match info.st_ifmt.as_str() {
            "S_IFREG" => plan.push(DeletePlanEntry {
                path,
                kind: AppDocumentKind::File,
            }),
            "S_IFDIR" => {
                if depth >= MAX_TRANSFER_DEPTH {
                    return Err(
                        "application directory deletion exceeds the maximum nesting depth".into(),
                    );
                }
                let names = client
                    .list_dir(afc_path(scope, &path))
                    .await
                    .map_err(|error| {
                        format!("unable to list application directory before deletion: {error:?}")
                    })?;
                pending.push((path.clone(), true, depth));
                for name in names.into_iter().filter(|name| name != "." && name != "..") {
                    validate_name(&name)?;
                    entries_seen += 1;
                    if entries_seen > MAX_TRANSFER_ENTRIES {
                        return Err(
                            "application directory deletion contains too many entries".into()
                        );
                    }
                    pending.push((join_path(&path, &name)?, false, depth + 1));
                }
            }
            _ => {
                return Err(format!(
                    "unsupported application entry cannot be recursively deleted: {path}"
                ));
            }
        }
    }
    Ok(plan)
}

async fn remove_checked(
    client: &mut AfcClient,
    scope: AppStorageScope,
    path: &str,
    expected: AppDocumentKind,
) -> Result<(), String> {
    ensure_no_symlink_components(client, scope, path).await?;
    let info = client
        .get_file_info(afc_path(scope, path))
        .await
        .map_err(|error| format!("unable to revalidate application entry: {error:?}"))?;
    let actual = match info.st_ifmt.as_str() {
        "S_IFREG" if info.st_link_target.is_none() => AppDocumentKind::File,
        "S_IFDIR" if info.st_link_target.is_none() => AppDocumentKind::Directory,
        _ => return Err("application entry changed during recursive deletion".into()),
    };
    if actual != expected {
        return Err("application entry changed during recursive deletion".into());
    }
    client
        .remove(afc_path(scope, path))
        .await
        .map_err(|error| format!("unable to delete application storage item: {error:?}"))
}

async fn ensure_name_available(
    client: &mut AfcClient,
    scope: AppStorageScope,
    directory: &str,
    name: &str,
) -> Result<(), String> {
    let entries = client
        .list_dir(afc_path(scope, directory))
        .await
        .map_err(|error| format!("unable to inspect application directory: {error:?}"))?;
    if entries.iter().any(|entry| entry == name) {
        Err("an application document with this name already exists".into())
    } else {
        Ok(())
    }
}

async fn ensure_no_symlink_components(
    client: &mut AfcClient,
    scope: AppStorageScope,
    path: &str,
) -> Result<(), String> {
    let mut components = path
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .peekable();
    let mut current = String::new();
    while let Some(component) = components.next() {
        current.push('/');
        current.push_str(component);
        let info = client
            .get_file_info(afc_path(scope, &current))
            .await
            .map_err(|error| format!("unable to inspect application storage path: {error:?}"))?;
        if info.st_link_target.is_some() {
            return Err("application storage paths cannot traverse symbolic links".into());
        }
        if components.peek().is_some() && info.st_ifmt != "S_IFDIR" {
            return Err("application storage path contains a non-directory component".into());
        }
    }
    Ok(())
}

fn validate_bundle_id(bundle_id: &str) -> Result<(), String> {
    if bundle_id.len() > 255
        || !bundle_id.contains('.')
        || bundle_id.split('.').any(|part| {
            part.is_empty()
                || part.len() > 63
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return Err("invalid application bundle identifier".into());
    }
    Ok(())
}

fn normalize_path(path: &str, allow_root: bool) -> Result<String, String> {
    if path.len() > MAX_PATH_BYTES || path.contains(['\0', '\\']) {
        return Err("invalid application document path".into());
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
            Err("the application storage root cannot be modified".into())
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
        Err("invalid application document name".into())
    } else {
        Ok(name)
    }
}

fn join_path(directory: &str, name: &str) -> Result<String, String> {
    validate_name(name)?;
    let joined = if directory == "/" {
        format!("/{name}")
    } else {
        format!("{directory}/{name}")
    };
    normalize_path(&joined, false)
}

fn parent_path(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
        .unwrap_or("/")
        .to_owned()
}

fn afc_path(scope: AppStorageScope, path: &str) -> String {
    match scope {
        AppStorageScope::Documents if path == "/" => "/Documents".into(),
        AppStorageScope::Documents => format!("/Documents{path}"),
        AppStorageScope::Container => path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_paths_are_confined_to_the_vended_root() {
        assert_eq!(normalize_path("/", true).unwrap(), "/");
        assert_eq!(
            normalize_path("/Save Games/slot 1", false).unwrap(),
            "/Save Games/slot 1"
        );
        assert_eq!(afc_path(AppStorageScope::Documents, "/"), "/Documents");
        assert_eq!(
            afc_path(AppStorageScope::Documents, "/Save Games"),
            "/Documents/Save Games"
        );
        assert_eq!(afc_path(AppStorageScope::Container, "/"), "/");
        assert_eq!(afc_path(AppStorageScope::Container, "/Library"), "/Library");
        for path in [
            "..",
            "/safe/../escape",
            r"/safe\escape",
            "/safe/./file",
            "/a\0b",
        ] {
            assert!(normalize_path(path, true).is_err(), "accepted {path:?}");
        }
        assert!(normalize_path("/", false).is_err());
    }

    #[test]
    fn storage_scope_is_explicit_and_symlinks_are_non_actionable() {
        assert_eq!(AppStorageScope::default(), AppStorageScope::Documents);
        assert_eq!(
            serde_json::from_str::<AppStorageScope>(r#""container""#).unwrap(),
            AppStorageScope::Container
        );
        assert!(
            storage_unavailable(
                AppStorageScope::Container,
                &["CoreDevice RSD: InstallationLookupFailed".into()],
            )
            .contains("developer-signed")
        );
        let timestamp = "1970-01-01T00:00:00".parse().unwrap();
        let entry = entry_from_info(
            "linked-library".into(),
            "/linked-library".into(),
            &idevice::afc::FileInfo {
                size: 0,
                blocks: 0,
                creation: timestamp,
                modified: timestamp,
                st_nlink: "1".into(),
                st_ifmt: "S_IFDIR".into(),
                st_link_target: Some("/Library".into()),
            },
        );
        assert!(matches!(entry.kind, AppDocumentKind::Other));
    }

    #[test]
    fn document_names_cannot_introduce_paths() {
        for name in ["", ".", "..", "a/b", r"a\b", "a\0b"] {
            assert!(validate_name(name).is_err(), "accepted {name:?}");
        }
        assert_eq!(join_path("/Saves", "slot.dat").unwrap(), "/Saves/slot.dat");
        assert_eq!(parent_path("/Saves/slot.dat"), "/Saves");
        assert_eq!(parent_path("/slot.dat"), "/");
    }

    #[test]
    fn validates_bundle_identifiers() {
        assert!(validate_bundle_id("com.example.game").is_ok());
        for bundle_id in [
            "",
            "game",
            "com..game",
            "com.example.bad value",
            "com/example/game",
        ] {
            assert!(validate_bundle_id(bundle_id).is_err());
        }
    }

    #[test]
    fn transfer_counts_serialize_for_api_clients() {
        let transfer = AppDocumentTransfer {
            bytes_transferred: 42,
            files_transferred: 2,
            directories_transferred: 1,
        };
        let value = serde_json::to_value(transfer).unwrap();
        assert_eq!(value["bytes_transferred"], 42);
        assert_eq!(value["files_transferred"], 2);
        assert_eq!(value["directories_transferred"], 1);
    }

    #[test]
    fn activity_tracks_only_the_requested_application() {
        let slot = AppDocumentActivitySlot::default();
        let id = slot.start(
            "com.example.game",
            AppStorageScope::Documents,
            AppDocumentActivityKind::Export,
            "/Saves/slot.dat".into(),
            Some(100),
        );
        slot.update(
            id,
            AppDocumentTransfer {
                bytes_transferred: 42,
                files_transferred: 0,
                directories_transferred: 0,
            },
        );
        let running = slot.get("com.example.game");
        assert_eq!(running.state, AppDocumentActivityState::Running);
        assert_eq!(running.bytes_transferred, 42);
        assert_eq!(running.bytes_total, Some(100));
        assert_eq!(
            slot.get("com.example.other").state,
            AppDocumentActivityState::Idle
        );

        slot.finish(id, &Ok(()));
        let completed = slot.get("com.example.game");
        assert_eq!(completed.state, AppDocumentActivityState::Succeeded);
        assert_eq!(completed.bytes_transferred, 100);
    }

    #[test]
    fn stale_activity_updates_cannot_replace_a_new_transfer() {
        let slot = AppDocumentActivitySlot::default();
        let stale = slot.start(
            "com.example.game",
            AppStorageScope::Documents,
            AppDocumentActivityKind::Export,
            "/old".into(),
            None,
        );
        let current = slot.start(
            "com.example.game",
            AppStorageScope::Container,
            AppDocumentActivityKind::Import,
            "/new".into(),
            None,
        );
        slot.update(
            stale,
            AppDocumentTransfer {
                bytes_transferred: 99,
                ..AppDocumentTransfer::default()
            },
        );
        slot.finish(stale, &Err("stale failure".into()));

        let view = slot.get("com.example.game");
        assert_eq!(view.id, current);
        assert_eq!(view.kind, Some(AppDocumentActivityKind::Import));
        assert_eq!(view.state, AppDocumentActivityState::Running);
        assert_eq!(view.bytes_transferred, 0);
        assert_eq!(view.error, None);
    }

    #[test]
    fn transfer_cancellation_is_scoped_to_the_running_application() {
        let slot = AppDocumentActivitySlot::default();
        assert!(!slot.cancel("com.example.game"));

        let cancelled = slot.start(
            "com.example.game",
            AppStorageScope::Documents,
            AppDocumentActivityKind::Export,
            "/old".into(),
            None,
        );
        assert!(!slot.cancel("com.example.other"));
        assert!(slot.cancel("com.example.game"));
        assert!(slot.is_cancelled(cancelled));
        slot.finish(cancelled, &Err(TRANSFER_CANCELLED.into()));
        assert_eq!(
            slot.get("com.example.game").state,
            AppDocumentActivityState::Cancelled
        );
        assert!(!slot.cancel("com.example.game"));

        let current = slot.start(
            "com.example.game",
            AppStorageScope::Container,
            AppDocumentActivityKind::Import,
            "/new".into(),
            None,
        );
        assert!(!slot.is_cancelled(cancelled));
        assert!(!slot.is_cancelled(current));
    }

    #[tokio::test]
    async fn transfer_copy_stops_when_cancelled() {
        let slot = AppDocumentActivitySlot::default();
        let id = slot.start(
            "com.example.game",
            AppStorageScope::Documents,
            AppDocumentActivityKind::Export,
            "/Saves/slot.dat".into(),
            None,
        );
        let mut progress = TransferProgress::new(slot.clone(), id);
        assert!(slot.cancel("com.example.game"));

        let error = progress
            .copy(&mut tokio::io::empty(), &mut tokio::io::sink())
            .await
            .unwrap_err();
        assert_eq!(error, TRANSFER_CANCELLED);
    }

    #[tokio::test]
    #[ignore = "requires a connected physical device with a file-sharing application"]
    async fn lists_storage_scopes_from_hardware() {
        use idevice::IdeviceService;
        use idevice::core_device_proxy::CoreDeviceProxy;
        use idevice::installation_proxy::InstallationProxyClient;
        use idevice::usbmuxd::{UsbmuxdAddr, UsbmuxdConnection};

        let mut usbmuxd = UsbmuxdConnection::default().await.unwrap();
        let device = usbmuxd
            .get_devices()
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("no connected device");
        let provider: Arc<dyn IdeviceProvider> =
            Arc::new(device.to_provider(UsbmuxdAddr::default(), "devicehub-mask-document-test"));
        let apps = InstallationProxyClient::connect(provider.as_ref())
            .await
            .unwrap()
            .get_apps(Some("User"), None)
            .await
            .unwrap();
        let documents_bundle_id = apps
            .iter()
            .find_map(|(bundle_id, value)| {
                let fields = value.as_dictionary()?;
                let shared = ["UIFileSharingEnabled", "UISupportsDocumentBrowser"]
                    .into_iter()
                    .any(|key| {
                        fields
                            .get(key)
                            .and_then(plist::Value::as_boolean)
                            .unwrap_or(false)
                    });
                shared.then(|| bundle_id.clone())
            })
            .expect("device has no file-sharing application");
        let container_bundle_id = apps.iter().find_map(|(bundle_id, value)| {
            let fields = value.as_dictionary()?;
            let developer = fields
                .get("IsXcodeManaged")
                .and_then(plist::Value::as_boolean)
                .unwrap_or(false)
                || fields
                    .get("SignerIdentity")
                    .and_then(plist::Value::as_string)
                    .is_some_and(|signer| signer.contains("Development"));
            developer.then(|| bundle_id.clone())
        });
        let proxy = CoreDeviceProxy::connect(provider.as_ref()).await.unwrap();
        let rsd_port = proxy.tunnel_info().server_rsd_port;
        let adapter = proxy.create_software_tunnel().unwrap();
        let mut adapter = adapter.to_async_handle();
        let stream = adapter.connect(rsd_port).await.unwrap();
        let handshake = RsdHandshake::new(stream).await.unwrap();
        let mut transport = AppStorageTransport::new(provider, ConnKind::Usb, adapter, handshake);

        let documents = list_documents(
            &mut transport,
            &documents_bundle_id,
            AppStorageScope::Documents,
            "/",
        )
        .await
        .unwrap();
        if let Some(container_bundle_id) = container_bundle_id {
            let container = list_documents(
                &mut transport,
                &container_bundle_id,
                AppStorageScope::Container,
                "/",
            )
            .await
            .unwrap();
            println!(
                "listed {} Documents entries for {documents_bundle_id} and {} container entries for {container_bundle_id}",
                documents.entries.len(),
                container.entries.len()
            );
        } else {
            println!(
                "listed {} Documents entries for {documents_bundle_id}; no developer-signed app is installed for a container probe",
                documents.entries.len()
            );
        }
    }
}
