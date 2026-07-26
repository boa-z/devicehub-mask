//! Host-independent runtime capabilities for DeviceHub.
//!
//! Desktop, HTTP, and MCP adapters inject implementations through these ports;
//! none of those host frameworks may become dependencies of this crate.

mod applications;
mod audio;
mod capture;
mod client;
mod clipboard;
mod demand;
mod device;
mod diagnostics;
mod input;
mod media;
mod performance;
mod preferences;
mod runtime;
mod session;
mod storage;
mod supervisor;
mod transport;

pub use applications::{
    APP_CONTROL_REQUEST_TIMEOUT, APP_LIST_REQUEST_TIMEOUT, AppCommand, AppConsoleCommand,
    AppConsoleLine, AppConsolePhase, AppConsoleSnapshot, AppIconCommand, AppLifecycleCommand,
    DEFAULT_SOURCE_CHARS, MAX_ATTRIBUTE_BYTES, MAX_ATTRIBUTE_CHARACTERS,
    MAX_BACKGROUND_DURATION_MS, MAX_ELEMENTS, MAX_HOLD_DURATION_MS, MAX_SELECTOR_BYTES,
    MAX_SOURCE_CHARS, MAX_TEXT_BYTES, MAX_TEXT_CHARACTERS, MAX_WAIT_TIMEOUT_MS,
    MIN_BACKGROUND_DURATION_MS, MIN_HOLD_DURATION_MS, RunningProcessCommand, WdaAutomationCommand,
    WdaBoundedText, WdaDeviceState, WdaElement, WdaElementDetails, WdaElementWaitResult,
    WdaElementWaitState, WdaOrientation, WdaRect, WdaRunnerCommand, WdaRunnerPhase,
    WdaRunnerStatus, WdaSize, WdaStatus, WdaUiTree, WdaUnlockResult, parse_wait_state,
    validate_background_duration, validate_hold_duration, validate_runner_bundle_id,
    validate_scroll_direction, validate_selector, validate_text, validate_wait_timeout,
};
pub use audio::{
    AudioPublisher, DeviceAudioFuture, DeviceAudioPipeline, DeviceAudioPipelineFactory,
    DeviceAudioSource, PcmAudioConsumer,
};
pub use capture::{
    BluetoothCaptureCommand, BluetoothCaptureSlot, BluetoothCaptureState, BluetoothCaptureStatus,
    BluetoothCaptureStopReason, CaptureFileFuture, CaptureFileIo, CaptureFileKind,
    CaptureFileWriter, MAX_BLUETOOTH_CAPTURE_DURATION_SECONDS,
    MAX_NETWORK_CAPTURE_DURATION_SECONDS, MIN_BLUETOOTH_CAPTURE_DURATION_SECONDS,
    MIN_NETWORK_CAPTURE_DURATION_SECONDS, NetworkCaptureCommand, NetworkCaptureSlot,
    NetworkCaptureState, NetworkCaptureStatus, NetworkCaptureStopReason,
    validate_bluetooth_capture_duration, validate_network_capture_duration,
};
pub use client::{DeviceControlError, DeviceControlService, RuntimeClient};
pub use clipboard::{ClipboardImage, ClipboardSlot, HostClipboard, HostClipboardProvider};
pub use demand::{Demand, DemandLease};
pub use device::{
    CompanionDeviceCommand, CrashReportExportCommand, DeveloperImageAssetFuture,
    DeveloperImageAssetLoader, DeveloperImageMountCommand, DeveloperImageMountRequest,
    DeveloperImageMountSlot, DeveloperImageMountState, DeveloperImageMountStatus,
    DeveloperModeCommand, DeveloperModePreparation, DeviceConditionCommand, DeviceConditionSlot,
    DeviceEventSlot, DeviceLogBatch, DeviceLogDemand, DeviceLogEntry, DeviceLogLevel,
    DeviceLogSlot, DeviceLogSource, HomeScreenCommand, LocationCommand, MAX_BATCH_ENTRIES,
    MAX_CRASH_REPORT_READ_BYTES, MAX_PROVISIONING_PROFILE_BYTES, ProvisioningCommand,
    ProvisioningFailure, ProvisioningInstall, ProvisioningProfileFuture, ProvisioningProfileLoader,
    ScreenCaptureCommand, developer_image_type_for_version, parse_provisioning_profile,
    prepare_provisioning_install, profiles_from_raw, unreadable_profile,
    validate_crash_report_path, validate_device_condition_identifiers,
};
pub use diagnostics::{
    ALLOWED_LOG_ARCHIVE_AGE_LIMIT_HOURS, DeviceBackupCommand, DeviceBackupExecutor,
    DeviceBackupFuture, DeviceBackupPrepareFuture, DeviceBackupSlot, DeviceBackupState,
    DeviceBackupStatus, LogArchiveCommand, LogArchiveSlot, LogArchiveState, LogArchiveStatus,
    SysdiagnoseCommand, SysdiagnoseSlot, SysdiagnoseState, SysdiagnoseStatus,
    validate_log_archive_age_limit_hours,
};
pub use input::{DeviceInputCommand, DeviceInputDispatcher, TouchContact};
pub use media::{
    BrowserFrameDecision, BrowserVideoFrame, BrowserVideoSlot, FrameCredit, FramePacer,
    FramePacerMetrics, RtcpOptions, audio_decoder_restart_backoff, browser_frame_decision,
    configured_in_flight_frames, duration_average_ms, encode_packet,
};
pub use performance::{PerformanceDemand, PerformanceSlot};
pub use preferences::RuntimePreferences;
pub use runtime::{CoreRuntime, CoreRuntimeFuture, CoreRuntimeState, OWNER_THREAD_STACK_BYTES};
pub use session::{
    DeviceSessionCommand, DiagnosticDumpSinkFactory, DiagnosticDumpSinkFuture,
    RuntimeSessionHostAdapters, SessionCommandSlot, SessionControlCommand, SessionDiagnostics,
    SessionManager,
};
pub use storage::{
    APP_DOCUMENT_TRANSFER_CANCELLED, AppDocumentActivityKind, AppDocumentActivitySlot,
    AppDocumentActivityState, AppDocumentActivityView, AppDocumentCommand, AppDocumentEntry,
    AppDocumentKind, AppDocumentList, AppDocumentTransfer, AppStorageScope, DeviceFileActivityKind,
    DeviceFileActivitySlot, DeviceFileActivityState, DeviceFileActivityView, DeviceFileCommand,
    DeviceFileEntry, DeviceFileKind, DeviceFileList, DeviceFileTransfer, HostDirectoryEntry,
    HostFileFuture, HostFileIo, HostFileKind, HostFileMetadata, HostFileReader, HostFileWrite,
    HostFileWriter, TRANSFER_CANCELLED, is_app_document_transfer_cancelled, is_transfer_cancelled,
};
pub use supervisor::{
    ServiceHealth, ServicePhase, ServiceRegistry, ServiceReporter, ServiceSupervisor,
    reconnect_backoff, wait_for_retry,
};
pub use transport::{
    CoreTunnelConfig, MuxSidecar, MuxSidecarFuture, RemotePairingStore, StoredWifiPairingRecord,
    SystemUsbmuxdConfig, WIFI_REAUTHORIZE_REQUIRED, WifiPairingStore,
};
pub(crate) use transport::{
    SessionEndpoint, UsbmuxdEndpoint, connect_provider, resolve_device_selection,
};
