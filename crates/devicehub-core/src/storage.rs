//! Device storage domain models and transfer activity policy.
//!
//! Paths here are device-relative strings. Host filesystem paths and the AFC
//! or House Arrest implementations remain runtime/host concerns.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

pub const DEVICE_FILE_TRANSFER_CANCELLED: &str = "device file transfer cancelled";
pub const APP_DOCUMENT_TRANSFER_CANCELLED: &str = "application storage transfer cancelled";
const MAX_DEVICE_PATH_BYTES: usize = 1_024;

pub fn validate_app_bundle_id(bundle_id: &str) -> Result<(), String> {
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

pub fn normalize_device_file_path(path: &str, allow_root: bool) -> Result<String, String> {
    normalize_storage_path(
        path,
        allow_root,
        validate_device_file_name,
        "invalid device file path",
        "the AFC root cannot be exported",
    )
}

pub fn validate_device_file_name(name: &str) -> Result<&str, String> {
    validate_storage_name(name, "invalid device file name")
}

pub fn join_device_file_path(directory: &str, name: &str) -> Result<String, String> {
    validate_device_file_name(name)?;
    join_storage_path(directory, name, normalize_device_file_path)
}

pub fn normalize_app_document_path(path: &str, allow_root: bool) -> Result<String, String> {
    normalize_storage_path(
        path,
        allow_root,
        validate_app_document_name,
        "invalid application document path",
        "the application storage root cannot be modified",
    )
}

pub fn validate_app_document_name(name: &str) -> Result<&str, String> {
    validate_storage_name(name, "invalid application document name")
}

pub fn join_app_document_path(directory: &str, name: &str) -> Result<String, String> {
    validate_app_document_name(name)?;
    join_storage_path(directory, name, normalize_app_document_path)
}

fn normalize_storage_path<'a>(
    path: &'a str,
    allow_root: bool,
    validate_name: fn(&'a str) -> Result<&'a str, String>,
    invalid_path: &'static str,
    root_error: &'static str,
) -> Result<String, String> {
    if path.len() > MAX_DEVICE_PATH_BYTES || path.contains(['\0', '\\']) {
        return Err(invalid_path.into());
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
            Err(root_error.into())
        };
    }
    Ok(format!("/{}", components.join("/")))
}

fn validate_storage_name<'a>(name: &'a str, error: &'static str) -> Result<&'a str, String> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.len() > 255
        || name.contains(['/', '\\', '\0'])
    {
        Err(error.into())
    } else {
        Ok(name)
    }
}

fn join_storage_path(
    directory: &str,
    name: &str,
    normalize: fn(&str, bool) -> Result<String, String>,
) -> Result<String, String> {
    let joined = if directory == "/" {
        format!("/{name}")
    } else {
        format!("{directory}/{name}")
    };
    normalize(&joined, false)
}

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
    pub fn start(&self, kind: DeviceFileActivityKind, path: String) -> u64 {
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

    pub fn update(&self, id: u64, transfer: DeviceFileTransfer) {
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

    pub fn set_total(&self, id: u64, bytes_total: u64) {
        let mut view = self
            .view
            .lock()
            .expect("device file activity lock poisoned");
        if view.id == id && view.state == DeviceFileActivityState::Running {
            view.bytes_total = Some(bytes_total);
        }
    }

    pub fn finish(&self, id: u64, result: &Result<(), String>) {
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
            Err(error) if is_device_file_transfer_cancelled(error) => {
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

    pub fn is_cancelled(&self, id: u64) -> bool {
        self.active_id.load(Ordering::Acquire) == id && self.cancelled.load(Ordering::Acquire)
    }

    pub fn reset(&self) {
        let mut view = self
            .view
            .lock()
            .expect("device file activity lock poisoned");
        self.cancelled.store(false, Ordering::Release);
        self.active_id.store(0, Ordering::Release);
        *view = DeviceFileActivityView::default();
    }
}

pub fn is_device_file_transfer_cancelled(error: &str) -> bool {
    error.contains(DEVICE_FILE_TRANSFER_CANCELLED)
}

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
    pub fn start(
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

    pub fn update(&self, id: u64, transfer: AppDocumentTransfer) {
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

    pub fn set_total(&self, id: u64, bytes_total: u64) {
        let mut view = self
            .view
            .lock()
            .expect("app document activity lock poisoned");
        if view.id == id && view.state == AppDocumentActivityState::Running {
            view.bytes_total = Some(bytes_total);
        }
    }

    pub fn finish(&self, id: u64, result: &Result<(), String>) {
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
            Err(error) if is_app_document_transfer_cancelled(error) => {
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

    pub fn is_cancelled(&self, id: u64) -> bool {
        self.active_id.load(Ordering::Acquire) == id && self.cancelled.load(Ordering::Acquire)
    }

    pub fn reset(&self) {
        let mut view = self
            .view
            .lock()
            .expect("app document activity lock poisoned");
        self.cancelled.store(false, Ordering::Release);
        self.active_id.store(0, Ordering::Release);
        *view = AppDocumentActivityView::default();
    }
}

pub fn is_app_document_transfer_cancelled(error: &str) -> bool {
    error.contains(APP_DOCUMENT_TRANSFER_CANCELLED)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_transfer_activity_ignores_stale_updates_and_scopes_cancellation() {
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
        assert_eq!(slot.get().id, current);
        assert_eq!(slot.get().bytes_transferred, 0);

        assert!(slot.cancel());
        assert!(slot.is_cancelled(current));
        slot.finish(current, &Err(DEVICE_FILE_TRANSFER_CANCELLED.into()));
        assert_eq!(slot.get().state, DeviceFileActivityState::Cancelled);
    }

    #[test]
    fn app_transfer_activity_is_scoped_to_bundle_and_completes_totals() {
        let slot = AppDocumentActivitySlot::default();
        let id = slot.start(
            "com.example.game",
            AppStorageScope::Documents,
            AppDocumentActivityKind::Export,
            "/save.dat".into(),
            Some(100),
        );
        slot.update(
            id,
            AppDocumentTransfer {
                bytes_transferred: 42,
                ..AppDocumentTransfer::default()
            },
        );
        assert_eq!(
            slot.get("com.example.other").state,
            AppDocumentActivityState::Idle
        );
        assert!(!slot.cancel("com.example.other"));
        slot.finish(id, &Ok(()));
        let completed = slot.get("com.example.game");
        assert_eq!(completed.state, AppDocumentActivityState::Succeeded);
        assert_eq!(completed.bytes_transferred, 100);
    }

    #[test]
    fn transfer_errors_are_bounded_and_cancellation_is_typed() {
        let slot = DeviceFileActivitySlot::default();
        let id = slot.start(DeviceFileActivityKind::Export, "/file".into());
        slot.finish(id, &Err("x".repeat(1_000)));
        assert_eq!(slot.get().error.unwrap().chars().count(), 512);
        assert!(is_device_file_transfer_cancelled(
            DEVICE_FILE_TRANSFER_CANCELLED
        ));
        assert!(is_app_document_transfer_cancelled(
            APP_DOCUMENT_TRANSFER_CANCELLED
        ));
    }

    #[test]
    fn storage_paths_and_bundle_ids_are_bounded_and_confined() {
        assert_eq!(
            normalize_device_file_path("/DCIM/100APPLE", false).unwrap(),
            "/DCIM/100APPLE"
        );
        assert_eq!(
            join_device_file_path("/DCIM", "IMG_0001.HEIC").unwrap(),
            "/DCIM/IMG_0001.HEIC"
        );
        assert_eq!(
            normalize_app_document_path("/Save Games/slot 1", false).unwrap(),
            "/Save Games/slot 1"
        );
        assert_eq!(
            join_app_document_path("/Saves", "slot.dat").unwrap(),
            "/Saves/slot.dat"
        );
        assert!(validate_app_bundle_id("com.example.game").is_ok());
        for path in ["..", "/safe/../escape", r"/safe\escape", "/safe/./file"] {
            assert!(normalize_device_file_path(path, true).is_err());
            assert!(normalize_app_document_path(path, true).is_err());
        }
        assert!(validate_app_bundle_id("com..example").is_err());
    }
}
