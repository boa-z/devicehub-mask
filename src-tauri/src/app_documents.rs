//! Sandboxed application Documents access through House Arrest and AFC.

use std::path::{Path, PathBuf};
use std::time::Duration;

use idevice::RsdService;
use idevice::afc::AfcClient;
use idevice::afc::opcode::AfcFopenMode;
use idevice::house_arrest::HouseArrestClient;
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use serde::Serialize;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::{mpsc, oneshot, watch};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const METADATA_TIMEOUT: Duration = Duration::from_secs(15);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const MAX_DIRECTORY_ENTRIES: usize = 500;
const MAX_PATH_BYTES: usize = 1_024;

#[derive(Debug, Clone, Serialize)]
pub struct AppDocumentEntry {
    pub name: String,
    pub path: String,
    pub kind: AppDocumentKind,
    pub size_bytes: u64,
    pub modified: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
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

#[derive(Debug)]
pub enum AppDocumentCommand {
    List {
        bundle_id: String,
        path: String,
        reply: oneshot::Sender<Result<AppDocumentList, String>>,
    },
    Export {
        bundle_id: String,
        path: String,
        destination: PathBuf,
        reply: oneshot::Sender<Result<u64, String>>,
    },
    Import {
        bundle_id: String,
        directory: String,
        source: PathBuf,
        reply: oneshot::Sender<Result<AppDocumentEntry, String>>,
    },
    CreateDirectory {
        bundle_id: String,
        directory: String,
        name: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Rename {
        bundle_id: String,
        path: String,
        name: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Delete {
        bundle_id: String,
        path: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
}

pub async fn serve(
    mut adapter: AdapterHandle,
    mut handshake: RsdHandshake,
    mut commands: mpsc::Receiver<AppDocumentCommand>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
            }
            command = commands.recv() => {
                let Some(command) = command else { return };
                handle(command, &mut adapter, &mut handshake).await;
            }
        }
    }
}

async fn handle(
    command: AppDocumentCommand,
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
) {
    match command {
        AppDocumentCommand::List {
            bundle_id,
            path,
            reply,
        } => {
            let result = tokio::time::timeout(
                METADATA_TIMEOUT,
                list_documents(adapter, handshake, &bundle_id, &path),
            )
            .await
            .unwrap_or_else(|_| Err("application document listing timed out".into()));
            let _ = reply.send(result);
        }
        AppDocumentCommand::Export {
            bundle_id,
            path,
            destination,
            reply,
        } => {
            let result = tokio::time::timeout(
                TRANSFER_TIMEOUT,
                export_document(adapter, handshake, &bundle_id, &path, &destination),
            )
            .await
            .unwrap_or_else(|_| Err("application document export timed out".into()));
            let _ = reply.send(result);
        }
        AppDocumentCommand::Import {
            bundle_id,
            directory,
            source,
            reply,
        } => {
            let result = tokio::time::timeout(
                TRANSFER_TIMEOUT,
                import_document(adapter, handshake, &bundle_id, &directory, &source),
            )
            .await
            .unwrap_or_else(|_| Err("application document upload timed out".into()));
            let _ = reply.send(result);
        }
        AppDocumentCommand::CreateDirectory {
            bundle_id,
            directory,
            name,
            reply,
        } => {
            let result = tokio::time::timeout(
                METADATA_TIMEOUT,
                create_directory(adapter, handshake, &bundle_id, &directory, &name),
            )
            .await
            .unwrap_or_else(|_| Err("application directory creation timed out".into()));
            let _ = reply.send(result);
        }
        AppDocumentCommand::Rename {
            bundle_id,
            path,
            name,
            reply,
        } => {
            let result = tokio::time::timeout(
                METADATA_TIMEOUT,
                rename_document(adapter, handshake, &bundle_id, &path, &name),
            )
            .await
            .unwrap_or_else(|_| Err("application document rename timed out".into()));
            let _ = reply.send(result);
        }
        AppDocumentCommand::Delete {
            bundle_id,
            path,
            reply,
        } => {
            let result = tokio::time::timeout(
                METADATA_TIMEOUT,
                delete_document(adapter, handshake, &bundle_id, &path),
            )
            .await
            .unwrap_or_else(|_| Err("application document deletion timed out".into()));
            let _ = reply.send(result);
        }
    }
}

async fn connect_documents(
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
    bundle_id: &str,
) -> Result<AfcClient, String> {
    validate_bundle_id(bundle_id)?;
    let house_arrest = tokio::time::timeout(
        CONNECT_TIMEOUT,
        HouseArrestClient::connect_rsd(adapter, handshake),
    )
    .await
    .map_err(|_| "House Arrest connection timed out".to_string())?
    .map_err(|error| format!("House Arrest service unavailable: {error:?}"))?;
    house_arrest
        .vend_documents(bundle_id.to_owned())
        .await
        .map_err(|error| format!("application does not expose Documents: {error:?}"))
}

async fn list_documents(
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
    bundle_id: &str,
    path: &str,
) -> Result<AppDocumentList, String> {
    let path = normalize_path(path, true)?;
    let mut client = connect_documents(adapter, handshake, bundle_id).await?;
    let device_path = afc_path(&path);
    let mut names = client
        .list_dir(device_path)
        .await
        .map_err(|error| format!("unable to list application Documents: {error:?}"))?;
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
        let info = match client.get_file_info(afc_path(&entry_path)).await {
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
        "S_IFREG" => AppDocumentKind::File,
        "S_IFDIR" => AppDocumentKind::Directory,
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

async fn export_document(
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
    bundle_id: &str,
    path: &str,
    destination: &Path,
) -> Result<u64, String> {
    let path = normalize_path(path, false)?;
    let mut client = connect_documents(adapter, handshake, bundle_id).await?;
    let info = client
        .get_file_info(afc_path(&path))
        .await
        .map_err(|error| format!("unable to inspect application document: {error:?}"))?;
    if info.st_ifmt != "S_IFREG" {
        return Err("only regular application documents can be exported".into());
    }
    let parent = destination
        .parent()
        .ok_or_else(|| "export destination has no parent directory".to_string())?;
    tokio::fs::metadata(parent)
        .await
        .map_err(|error| format!("export destination is unavailable: {error}"))?;
    let temporary = temporary_sibling(destination, "export")?;
    let result = async {
        let mut remote = client
            .open(afc_path(&path), AfcFopenMode::RdOnly)
            .await
            .map_err(|error| format!("unable to open application document: {error:?}"))?;
        let local = tokio::fs::File::create(&temporary)
            .await
            .map_err(|error| format!("unable to create export file: {error}"))?;
        let mut local = BufWriter::new(local);
        let bytes = tokio::io::copy(&mut remote, &mut local)
            .await
            .map_err(|error| format!("unable to export application document: {error}"))?;
        local
            .flush()
            .await
            .map_err(|error| format!("unable to flush export file: {error}"))?;
        remote
            .close()
            .await
            .map_err(|error| format!("unable to close application document: {error:?}"))?;
        replace_local_file(&temporary, destination).await?;
        Ok(bytes)
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    result
}

async fn import_document(
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
    bundle_id: &str,
    directory: &str,
    source: &Path,
) -> Result<AppDocumentEntry, String> {
    let directory = normalize_path(directory, true)?;
    let source = tokio::fs::canonicalize(source)
        .await
        .map_err(|error| format!("upload source is unavailable: {error}"))?;
    let metadata = tokio::fs::metadata(&source)
        .await
        .map_err(|error| format!("unable to inspect upload source: {error}"))?;
    if !metadata.is_file() {
        return Err("upload source must be a regular file".into());
    }
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "upload source has an unsupported file name".to_string())?
        .to_owned();
    validate_name(&name)?;
    let target = join_path(&directory, &name)?;
    let mut client = connect_documents(adapter, handshake, bundle_id).await?;
    let existing = client
        .list_dir(afc_path(&directory))
        .await
        .map_err(|error| format!("unable to inspect upload directory: {error:?}"))?;
    if existing.iter().any(|entry| entry == &name) {
        return Err("an application document with this name already exists".into());
    }

    let temporary_name = format!(".devicehub-upload-{}", uuid::Uuid::new_v4());
    let temporary = join_path(&directory, &temporary_name)?;
    let result = async {
        let mut local = tokio::fs::File::open(&source)
            .await
            .map_err(|error| format!("unable to open upload source: {error}"))?;
        let mut remote = client
            .open(afc_path(&temporary), AfcFopenMode::WrOnly)
            .await
            .map_err(|error| format!("unable to create remote temporary file: {error:?}"))?;
        tokio::io::copy(&mut local, &mut remote)
            .await
            .map_err(|error| format!("unable to upload application document: {error}"))?;
        remote
            .shutdown()
            .await
            .map_err(|error| format!("unable to flush application document: {error}"))?;
        remote
            .close()
            .await
            .map_err(|error| format!("unable to close application document: {error:?}"))?;
        client
            .rename(afc_path(&temporary), afc_path(&target))
            .await
            .map_err(|error| format!("unable to finish application document upload: {error:?}"))?;
        let info = client
            .get_file_info(afc_path(&target))
            .await
            .map_err(|error| format!("unable to inspect uploaded document: {error:?}"))?;
        Ok(entry_from_info(name, target, &info))
    }
    .await;
    if result.is_err() {
        let _ = client.remove(afc_path(&temporary)).await;
    }
    result
}

async fn create_directory(
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
    bundle_id: &str,
    directory: &str,
    name: &str,
) -> Result<(), String> {
    let directory = normalize_path(directory, true)?;
    validate_name(name)?;
    let target = join_path(&directory, name)?;
    let mut client = connect_documents(adapter, handshake, bundle_id).await?;
    ensure_name_available(&mut client, &directory, name).await?;
    client
        .mk_dir(afc_path(&target))
        .await
        .map_err(|error| format!("unable to create application directory: {error:?}"))
}

async fn rename_document(
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
    bundle_id: &str,
    path: &str,
    name: &str,
) -> Result<(), String> {
    let path = normalize_path(path, false)?;
    validate_name(name)?;
    let parent = parent_path(&path);
    let target = join_path(&parent, name)?;
    let mut client = connect_documents(adapter, handshake, bundle_id).await?;
    ensure_name_available(&mut client, &parent, name).await?;
    client
        .rename(afc_path(&path), afc_path(&target))
        .await
        .map_err(|error| format!("unable to rename application document: {error:?}"))
}

async fn delete_document(
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
    bundle_id: &str,
    path: &str,
) -> Result<(), String> {
    let path = normalize_path(path, false)?;
    let mut client = connect_documents(adapter, handshake, bundle_id).await?;
    client.remove(afc_path(&path)).await.map_err(|error| {
        format!("unable to delete application document; directories must be empty: {error:?}")
    })
}

async fn ensure_name_available(
    client: &mut AfcClient,
    directory: &str,
    name: &str,
) -> Result<(), String> {
    let entries = client
        .list_dir(afc_path(directory))
        .await
        .map_err(|error| format!("unable to inspect application directory: {error:?}"))?;
    if entries.iter().any(|entry| entry == name) {
        Err("an application document with this name already exists".into())
    } else {
        Ok(())
    }
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
            Err("the application Documents root cannot be modified".into())
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

fn afc_path(path: &str) -> String {
    if path == "/" {
        "/Documents".into()
    } else {
        format!("/Documents{path}")
    }
}

fn temporary_sibling(destination: &Path, operation: &str) -> Result<PathBuf, String> {
    let parent = destination
        .parent()
        .ok_or_else(|| "destination has no parent directory".to_string())?;
    Ok(parent.join(format!(
        ".devicehub-{operation}-{}-{}.part",
        std::process::id(),
        uuid::Uuid::new_v4()
    )))
}

async fn replace_local_file(temporary: &Path, destination: &Path) -> Result<(), String> {
    let backup = temporary_sibling(destination, "backup")?;
    let had_destination = match tokio::fs::metadata(destination).await {
        Ok(metadata) if metadata.is_file() => {
            tokio::fs::rename(destination, &backup)
                .await
                .map_err(|error| format!("unable to preserve existing export file: {error}"))?;
            true
        }
        Ok(_) => return Err("export destination is not a regular file".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(format!("unable to inspect export destination: {error}")),
    };
    match tokio::fs::rename(temporary, destination).await {
        Ok(()) => {
            if had_destination {
                let _ = tokio::fs::remove_file(backup).await;
            }
            Ok(())
        }
        Err(error) => {
            if had_destination {
                let _ = tokio::fs::rename(&backup, destination).await;
            }
            Err(format!(
                "unable to finish application document export: {error}"
            ))
        }
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
        assert_eq!(afc_path("/"), "/Documents");
        assert_eq!(afc_path("/Save Games"), "/Documents/Save Games");
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

    #[tokio::test]
    async fn local_export_replacement_preserves_new_contents() {
        let directory = std::env::temp_dir().join(format!(
            "devicehub-mask-document-test-{}",
            uuid::Uuid::new_v4()
        ));
        tokio::fs::create_dir(&directory).await.unwrap();
        let destination = directory.join("save.dat");
        let temporary = directory.join("incoming.part");
        tokio::fs::write(&destination, b"old").await.unwrap();
        tokio::fs::write(&temporary, b"new").await.unwrap();

        replace_local_file(&temporary, &destination).await.unwrap();

        assert_eq!(tokio::fs::read(&destination).await.unwrap(), b"new");
        assert!(!temporary.exists());
        tokio::fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires a connected physical device with a file-sharing application"]
    async fn lists_documents_from_hardware() {
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
        let provider = device.to_provider(UsbmuxdAddr::default(), "devicehub-mask-document-test");
        let apps = InstallationProxyClient::connect(&provider)
            .await
            .unwrap()
            .get_apps(Some("User"), None)
            .await
            .unwrap();
        let bundle_id = apps
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
        let proxy = CoreDeviceProxy::connect(&provider).await.unwrap();
        let rsd_port = proxy.tunnel_info().server_rsd_port;
        let adapter = proxy.create_software_tunnel().unwrap();
        let mut adapter = adapter.to_async_handle();
        let stream = adapter.connect(rsd_port).await.unwrap();
        let mut handshake = RsdHandshake::new(stream).await.unwrap();

        let listing = list_documents(&mut adapter, &mut handshake, &bundle_id, "/")
            .await
            .unwrap();
        println!(
            "listed {} Documents entries for {bundle_id}",
            listing.entries.len()
        );
    }
}
