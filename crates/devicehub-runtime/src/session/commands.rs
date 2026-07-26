//! Commands exchanged between host adapters and an active device session.

use devicehub_core::{
    CompanionDevice, DeviceCrashReportContent, DeviceCrashReportList, DeviceDetails,
    DeviceInputCommand, ForgetDeviceResult, HomeScreenLayout, PairDeviceResult, WallpaperKind,
};
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc::UnboundedSender, oneshot};

use crate::{
    AppCommand, AppConsoleCommand, AppDocumentCommand, AppLifecycleCommand,
    BluetoothCaptureCommand, DeveloperImageMountCommand, DeveloperModeCommand, DeviceBackupCommand,
    DeviceConditionCommand, DeviceFileCommand, LogArchiveCommand, NetworkCaptureCommand,
    ProvisioningCommand, RunningProcessCommand, SysdiagnoseCommand, WdaAutomationCommand,
    WdaRunnerCommand,
};

/// A command from any host adapter to the active device session.
#[derive(Debug)]
pub enum DeviceSessionCommand<HostPath> {
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
    DeveloperMode(DeveloperModeCommand),
    DeveloperImageMount(DeveloperImageMountCommand<HostPath>),
    Apps(AppCommand),
    ListCompanionDevices(oneshot::Sender<Result<Vec<CompanionDevice>, String>>),
    GetHomeScreenLayout(oneshot::Sender<Result<HomeScreenLayout, String>>),
    GetWallpaper {
        kind: WallpaperKind,
        reply: oneshot::Sender<Result<Vec<u8>, String>>,
    },
    RunningProcess(RunningProcessCommand),
    AppLifecycle(AppLifecycleCommand),
    WdaAutomation(WdaAutomationCommand),
    WdaRunner(WdaRunnerCommand),
    AppConsole(AppConsoleCommand),
    GetAppIcon {
        bundle_id: String,
        reply: oneshot::Sender<Result<Vec<u8>, String>>,
    },
    TakeScreenshot(oneshot::Sender<Result<Vec<u8>, String>>),
    NetworkCapture(NetworkCaptureCommand<HostPath>),
    BluetoothCapture(BluetoothCaptureCommand<HostPath>),
    DeviceBackup(DeviceBackupCommand<HostPath>),
    Sysdiagnose(SysdiagnoseCommand<HostPath>),
    LogArchive(LogArchiveCommand<HostPath>),
    DeviceFiles(DeviceFileCommand<HostPath>),
    DeviceCondition(DeviceConditionCommand),
    AppDocuments(AppDocumentCommand<HostPath>),
    LockDevice(oneshot::Sender<Result<(), String>>),
    RestartDevice(oneshot::Sender<Result<(), String>>),
    ShutdownDevice(oneshot::Sender<Result<(), String>>),
    Provisioning(ProvisioningCommand<HostPath>),
    ListCrashReports(oneshot::Sender<Result<DeviceCrashReportList, String>>),
    ReadCrashReport {
        device_path: String,
        max_bytes: usize,
        reply: oneshot::Sender<Result<DeviceCrashReportContent, String>>,
    },
    ExportCrashReport {
        device_path: String,
        destination: HostPath,
        reply: oneshot::Sender<Result<u64, String>>,
    },
    DeleteCrashReport {
        device_path: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Shutdown,
}

/// A command from a host adapter to the outer device-session manager.
#[derive(Debug)]
pub enum SessionControlCommand {
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

/// Active connected-session command endpoint swapped atomically on reconnect.
pub struct SessionCommandSlot<HostPath>(
    Arc<Mutex<Option<UnboundedSender<DeviceSessionCommand<HostPath>>>>>,
);

impl<HostPath> Clone for SessionCommandSlot<HostPath> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<HostPath> Default for SessionCommandSlot<HostPath> {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }
}

impl<HostPath> SessionCommandSlot<HostPath> {
    pub fn set(&self, sender: Option<UnboundedSender<DeviceSessionCommand<HostPath>>>) {
        *self.0.lock().unwrap() = sender;
    }

    pub fn send(&self, command: DeviceSessionCommand<HostPath>) {
        let _ = self.try_send(command);
    }

    pub fn try_send(&self, command: DeviceSessionCommand<HostPath>) -> bool {
        self.0
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|sender| sender.send(command).is_ok())
    }
}
