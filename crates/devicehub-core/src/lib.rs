//! Host-independent DeviceHub domain models and policy.
//!
//! This crate deliberately excludes device transports, async runtimes, HTTP,
//! MCP, desktop APIs, and sidecar processes. Public values are normalized and
//! bounded before runtime or host adapters exchange them.

mod applications;
mod capture;
mod clipboard;
mod device;
mod device_conditions;
mod device_events;
mod device_logs;
mod diagnostics;
mod home_screen;
mod input;
mod location;
mod media;
mod performance;
mod provisioning;
mod state;

pub use applications::{
    AppLifecycleStatus, AppLifecycleWaitResult, AppOperationKind, AppOperationState,
    AppOperationView, AppSigningKind, DeviceApp, RunningProcess, RunningProcessList,
    RunningProcessStatus, RunningProcessWaitResult, WDA_DEFAULT_SOURCE_CHARS,
    WDA_MAX_ATTRIBUTE_BYTES, WDA_MAX_ATTRIBUTE_CHARACTERS, WDA_MAX_BACKGROUND_DURATION_MS,
    WDA_MAX_ELEMENTS, WDA_MAX_HOLD_DURATION_MS, WDA_MAX_SELECTOR_BYTES, WDA_MAX_SOURCE_CHARS,
    WDA_MAX_TEXT_BYTES, WDA_MAX_TEXT_CHARACTERS, WDA_MAX_WAIT_TIMEOUT_MS,
    WDA_MIN_BACKGROUND_DURATION_MS, WDA_MIN_HOLD_DURATION_MS, WdaBoundedText, WdaDeviceState,
    WdaElement, WdaElementDetails, WdaElementWaitResult, WdaElementWaitState, WdaOrientation,
    WdaRect, WdaRunnerPhase, WdaRunnerStatus, WdaSize, WdaStatus, WdaUiTree, WdaUnlockResult,
    parse_wda_wait_state, process_executable_belongs_to_app, validate_wda_background_duration,
    validate_wda_hold_duration, validate_wda_runner_bundle_id, validate_wda_scroll_direction,
    validate_wda_selector, validate_wda_text, validate_wda_wait_timeout,
};
pub use capture::{
    BluetoothCaptureState, BluetoothCaptureStatus, BluetoothCaptureStopReason, NetworkCaptureState,
    NetworkCaptureStatus, NetworkCaptureStopReason,
};
pub use clipboard::{ClipboardContentKind, ClipboardEvent, clipboard_preview, validate_paste_text};
pub use device::{
    CompanionDevice, ConnKind, DeviceActivationState, DeviceBattery, DeviceDetails, DeviceInfo,
    DevicePairingState, DeviceRegionalSettings, DeviceStorage, ForgetDeviceOutcome,
    ForgetDeviceResult, PairDeviceOutcome, PairDeviceResult, device_selector, validate_device_name,
};
pub use device_conditions::{
    ActiveDeviceCondition, DeviceConditionGroup, DeviceConditionProfile, DeviceConditionStatus,
};
pub use device_events::{DeviceEvent, DeviceEventKind};
pub use device_logs::{
    DeviceLogBatch, DeviceLogEntry, DeviceLogLevel, DeviceLogSource, MAX_DEVICE_LOG_BATCH_ENTRIES,
};
pub use diagnostics::{
    CrashReportFormat, CrashReportKind, DeviceBackupState, DeviceBackupStatus, DeviceCrashReport,
    DeviceCrashReportContent, DeviceCrashReportList, DeviceCrashReportSummary, LogArchiveState,
    LogArchiveStatus, SysdiagnoseState, SysdiagnoseStatus, build_crash_report_content,
    device_id_fingerprint, validate_crash_report_path,
};
pub use home_screen::{
    HomeScreenAppLocation, HomeScreenContainer, HomeScreenFolderStep, HomeScreenIconMetrics,
    HomeScreenLayout, WallpaperKind,
};
pub use input::{
    DeviceInputCommand, HARDWARE_BUTTON_NAMES, HardwareButton, KeyMods, Orientation, RotateDir,
    TouchContact, ascii_key_usage, hardware_button, modifier_key_usages, norm, unrotate_norm,
};
pub use location::{LocationBackend, LocationStatus};
pub use media::{AUDIO_CHANNELS, AUDIO_SAMPLE_RATE};
pub use performance::{
    AppActivityEvent, DeviceNetworkInterface, DeviceNetworkInterfaceKind, PerformanceSnapshot,
    ProcessEnergy, ProcessPerformance,
};
pub use provisioning::ProvisioningProfile;
pub use state::{
    ActiveSlot, AppOperationSlot, DeviceListSlot, ErrorSlot, LocationStatusSlot, OrientationSlot,
    StatusSlot, VideoCounterSnapshot, VideoCounters,
};
