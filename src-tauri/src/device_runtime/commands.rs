//! Commands owned by the concrete device runtime.
//!
//! Unlike domain models, these messages may contain Tokio reply channels and
//! concrete service commands. Adapters dispatch them through typed handles;
//! they are not part of the host-independent `devicehub-core` data model.

use std::path::PathBuf;

use devicehub_runtime::{AppCommand, DeviceInputCommand};
use tokio::sync::oneshot;

use crate::domain::{
    CompanionDevice, DeviceCrashReportContent, DeviceCrashReportList, DeviceDetails,
    ForgetDeviceResult, PairDeviceResult,
};

/// A control command from an adapter to the active device session.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) enum InputCmd {
    DeviceInput(DeviceInputCommand),
    PasteText {
        text: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    SetLocation {
        latitude: f64,
        longitude: f64,
        reply: oneshot::Sender<Result<(), String>>,
    },
    ClearLocation {
        reply: oneshot::Sender<Result<(), String>>,
    },
    GetDeviceDetails(oneshot::Sender<Result<DeviceDetails, String>>),
    RenameDevice {
        name: String,
        reply: oneshot::Sender<Result<String, String>>,
    },
    DeveloperMode(devicehub_runtime::DeveloperModeCommand),
    DeveloperImageMount(crate::developer_image::DeveloperImageMountCommand),
    Apps(AppCommand),
    ListCompanionDevices(oneshot::Sender<Result<Vec<CompanionDevice>, String>>),
    GetHomeScreenLayout(oneshot::Sender<Result<crate::domain::HomeScreenLayout, String>>),
    GetWallpaper {
        kind: crate::domain::WallpaperKind,
        reply: oneshot::Sender<Result<Vec<u8>, String>>,
    },
    RunningProcess(devicehub_runtime::RunningProcessCommand),
    AppLifecycle(devicehub_runtime::AppLifecycleCommand),
    WdaAutomation(devicehub_runtime::WdaAutomationCommand),
    WdaRunner(devicehub_runtime::WdaRunnerCommand),
    AppConsole(devicehub_runtime::AppConsoleCommand),
    GetAppIcon {
        bundle_id: String,
        reply: oneshot::Sender<Result<Vec<u8>, String>>,
    },
    TakeScreenshot(oneshot::Sender<Result<Vec<u8>, String>>),
    NetworkCapture(crate::network_capture::NetworkCaptureCommand),
    BluetoothCapture(crate::bluetooth_capture::BluetoothCaptureCommand),
    DeviceBackup(crate::device_backup::DeviceBackupCommand),
    Sysdiagnose(crate::sysdiagnose::SysdiagnoseCommand),
    LogArchive(crate::log_archive::LogArchiveCommand),
    DeviceFiles(crate::device_files::DeviceFileCommand),
    DeviceCondition(devicehub_runtime::DeviceConditionCommand),
    AppDocuments(crate::app_documents::AppDocumentCommand),
    LockDevice(oneshot::Sender<Result<(), String>>),
    RestartDevice(oneshot::Sender<Result<(), String>>),
    ShutdownDevice(oneshot::Sender<Result<(), String>>),
    Provisioning(crate::provisioning::ProvisioningCommand),
    ListCrashReports(oneshot::Sender<Result<DeviceCrashReportList, String>>),
    ReadCrashReport {
        device_path: String,
        max_bytes: usize,
        reply: oneshot::Sender<Result<DeviceCrashReportContent, String>>,
    },
    ExportCrashReport {
        device_path: String,
        destination: PathBuf,
        reply: oneshot::Sender<Result<u64, String>>,
    },
    DeleteCrashReport {
        device_path: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Shutdown,
}

/// A control command from an adapter to the session manager.
#[derive(Debug)]
pub(crate) enum ControlCmd {
    Refresh,
    Connect(String),
    Reconnect(String),
    Pair {
        selection_id: String,
        reply: oneshot::Sender<PairDeviceResult>,
    },
    Forget {
        selection_id: String,
        reply: oneshot::Sender<ForgetDeviceResult>,
    },
    Quit,
}
