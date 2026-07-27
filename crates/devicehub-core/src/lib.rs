//! Host-independent DeviceHub domain models and policy.
//!
//! This crate deliberately excludes device transports, async runtimes, HTTP,
//! MCP, desktop APIs, and sidecar processes. Public values are normalized and
//! bounded before runtime or host adapters exchange them.

mod applications;
mod capture;
mod clipboard;
mod developer_image;
mod device;
mod device_conditions;
mod device_events;
mod device_logs;
mod diagnostics;
mod home_screen;
mod input;
mod key_mapping;
mod location;
mod media;
mod performance;
mod provisioning;
mod service_health;
mod state;
mod storage;

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
    BluetoothCaptureSlot, BluetoothCaptureState, BluetoothCaptureStatus,
    BluetoothCaptureStopReason, NetworkCaptureSlot, NetworkCaptureState, NetworkCaptureStatus,
    NetworkCaptureStopReason,
};
pub use clipboard::{ClipboardContentKind, ClipboardEvent, clipboard_preview, validate_paste_text};
pub use developer_image::{
    DeveloperImageMountSlot, DeveloperImageMountState, DeveloperImageMountStatus,
    developer_image_type_for_version,
};
pub use device::{
    CompanionDevice, ConnKind, DeviceActivationState, DeviceBattery, DeviceDetails, DeviceInfo,
    DevicePairingState, DeviceRegionalSettings, DeviceStorage, ForgetDeviceOutcome,
    ForgetDeviceResult, PairDeviceOutcome, PairDeviceResult, device_selector, validate_device_name,
};
pub use device_conditions::{
    ActiveDeviceCondition, DeviceConditionGroup, DeviceConditionProfile, DeviceConditionSlot,
    DeviceConditionStatus,
};
pub use device_events::{DeviceEvent, DeviceEventKind};
pub use device_logs::{
    DeviceLogBatch, DeviceLogEntry, DeviceLogLevel, DeviceLogMetadata, DeviceLogSlot,
    DeviceLogSource, MAX_DEVICE_LOG_BATCH_ENTRIES,
};
pub use diagnostics::{
    CrashReportFormat, CrashReportKind, DeviceBackupSlot, DeviceBackupState, DeviceBackupStatus,
    DeviceCrashReport, DeviceCrashReportContent, DeviceCrashReportList, DeviceCrashReportSummary,
    LogArchiveSlot, LogArchiveState, LogArchiveStatus, SysdiagnoseSlot, SysdiagnoseState,
    SysdiagnoseStatus, build_crash_report_content, device_id_fingerprint,
    validate_crash_report_path,
};
pub use home_screen::{
    HomeScreenAppLocation, HomeScreenContainer, HomeScreenFolderStep, HomeScreenIconMetrics,
    HomeScreenLayout, WallpaperKind,
};
pub use input::{
    DeviceInputCommand, HARDWARE_BUTTON_NAMES, HardwareButton, KeyMods, Orientation, RotateDir,
    TouchContact, ascii_key_usage, hardware_button, modifier_key_usages, norm, unrotate_norm,
};
pub use key_mapping::{
    InvalidKeyMappingProfile, KeyMappingProfile, KeyMappingResolution, default_hardware_bindings,
    validate_key_mapping_profile, validate_key_mapping_profile_name,
};
pub use location::{LocationBackend, LocationStatus};
pub use media::{AUDIO_CHANNELS, AUDIO_SAMPLE_RATE};
pub use performance::{
    AppActivityEvent, AppActivityObservation, DeviceNetworkInterface, DeviceNetworkInterfaceKind,
    EnergyMeasurement, MAX_APP_ACTIVITY_EVENTS, MAX_ENERGY_PROCESSES, PerformanceObservation,
    PerformanceSlot, PerformanceSnapshot, ProcessEnergy, ProcessPerformance,
    ProcessPerformanceObservation,
};
pub use provisioning::ProvisioningProfile;
pub use service_health::{ServiceHealth, ServicePhase, ServiceRegistry};
pub use state::{
    ActiveSlot, AppOperationSlot, DeviceListSlot, ErrorSlot, LocationStatusSlot, OrientationSlot,
    SessionPhase, SessionStatus, StatusSlot, VideoCounterSnapshot, VideoCounters,
};
pub use storage::{
    APP_DOCUMENT_TRANSFER_CANCELLED, AppDocumentActivityKind, AppDocumentActivitySlot,
    AppDocumentActivityState, AppDocumentActivityView, AppDocumentEntry, AppDocumentKind,
    AppDocumentList, AppDocumentTransfer, AppStorageScope, DEVICE_FILE_TRANSFER_CANCELLED,
    DeviceFileActivityKind, DeviceFileActivitySlot, DeviceFileActivityState,
    DeviceFileActivityView, DeviceFileEntry, DeviceFileKind, DeviceFileList, DeviceFileTransfer,
    is_app_document_transfer_cancelled, is_device_file_transfer_cancelled, join_app_document_path,
    join_device_file_path, normalize_app_document_path, normalize_device_file_path,
    validate_app_bundle_id, validate_app_document_name, validate_device_file_name,
};
