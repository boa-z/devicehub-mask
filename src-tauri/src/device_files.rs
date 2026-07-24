//! Read-only access to the device's public AFC media container.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use idevice::afc::AfcClient;
use idevice::afc::opcode::AfcFopenMode;
use idevice::provider::IdeviceProvider;
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use idevice::{IdeviceService, RsdService};
use serde::Serialize;
use tokio::io::{AsyncWriteExt, BufReader, BufWriter};
use tokio::sync::{mpsc, oneshot, watch};

use crate::protocol::ConnKind;
use crate::supervisor::ServiceReporter;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const METADATA_TIMEOUT: Duration = Duration::from_secs(30);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const MAX_DIRECTORY_ENTRIES: usize = 1_000;
const MAX_PATH_BYTES: usize = 1_024;
const TRANSFER_BUFFER_BYTES: usize = 64 * 1024;

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

#[derive(Debug)]
pub enum DeviceFileCommand {
    List {
        path: String,
        reply: oneshot::Sender<Result<DeviceFileList, String>>,
    },
    Export {
        path: String,
        destination: PathBuf,
        reply: oneshot::Sender<Result<u64, String>>,
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
    reporter: ServiceReporter,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut client = None;
    let mut attempt = 0;
    reporter.stopped(attempt);
    loop {
        let command = tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { return; }
                continue;
            }
            command = commands.recv() => command,
        };
        let Some(command) = command else { return };
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
        let result = handle(client.as_mut().expect("AFC client initialized"), command).await;
        if result.is_err() {
            client.take();
            reporter.stopped(attempt);
        }
    }
}

async fn handle(client: &mut AfcClient, command: DeviceFileCommand) -> Result<(), ()> {
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
            let result =
                tokio::time::timeout(TRANSFER_TIMEOUT, export_file(client, &path, &destination))
                    .await
                    .unwrap_or_else(|_| Err("device file export timed out".into()));
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
) -> Result<u64, String> {
    let path = normalize_path(path, false)?;
    validate_export_destination(destination).await?;
    let info = client
        .get_file_info(path.clone())
        .await
        .map_err(|error| format!("unable to inspect device file: {error:?}"))?;
    if info.st_ifmt != "S_IFREG" || info.st_link_target.is_some() {
        return Err("only regular device files can be exported".into());
    }
    let expected_size = info.size as u64;
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
        let bytes = tokio::io::copy(&mut remote, &mut local)
            .await
            .map_err(|error| format!("unable to export device file: {error}"))?;
        if bytes != expected_size {
            return Err("device file changed while it was being exported".into());
        }
        local
            .flush()
            .await
            .map_err(|error| format!("unable to flush export file: {error}"))?;
        remote
            .into_inner()
            .close()
            .await
            .map_err(|error| format!("unable to close device file: {error:?}"))?;
        crate::app_documents::replace_local_file(&temporary, destination).await?;
        Ok(bytes)
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    result
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
