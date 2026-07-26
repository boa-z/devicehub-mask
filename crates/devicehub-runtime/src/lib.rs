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
    RunningProcessCommand, WdaAutomationCommand, WdaRunnerCommand,
};
pub use audio::{
    AudioPublisher, DeviceAudioFuture, DeviceAudioPipeline, DeviceAudioPipelineFactory,
    DeviceAudioSource, PcmAudioConsumer,
};
pub use capture::{
    BluetoothCaptureCommand, CaptureFileFuture, CaptureFileIo, CaptureFileKind, CaptureFileWriter,
    MAX_BLUETOOTH_CAPTURE_DURATION_SECONDS, MAX_NETWORK_CAPTURE_DURATION_SECONDS,
    MIN_BLUETOOTH_CAPTURE_DURATION_SECONDS, MIN_NETWORK_CAPTURE_DURATION_SECONDS,
    NetworkCaptureCommand, validate_bluetooth_capture_duration, validate_network_capture_duration,
};
#[cfg(feature = "test-support")]
pub use client::RuntimeClientFixture;
pub use client::{DeviceControlError, DeviceControlService, RuntimeClient};
pub use clipboard::{ClipboardImage, ClipboardSlot, HostClipboard, HostClipboardProvider};
pub use demand::{Demand, DemandLease};
pub use device::{
    CompanionDeviceCommand, CrashReportExportCommand, DeveloperImageAssetFuture,
    DeveloperImageAssetLoader, DeveloperImageMountCommand, DeveloperImageMountRequest,
    DeveloperModeCommand, DeveloperModePreparation, DeviceConditionCommand, DeviceEventSlot,
    DeviceLogDemand, HomeScreenCommand, LocationCommand, MAX_CRASH_REPORT_READ_BYTES,
    MAX_PROVISIONING_PROFILE_BYTES, ProvisioningCommand, ProvisioningFailure, ProvisioningInstall,
    ProvisioningProfileFuture, ProvisioningProfileLoader, ScreenCaptureCommand,
    parse_provisioning_profile, prepare_provisioning_install, profiles_from_raw,
    unreadable_profile, validate_device_condition_identifiers,
};
pub use diagnostics::{
    ALLOWED_LOG_ARCHIVE_AGE_LIMIT_HOURS, DeviceBackupCommand, DeviceBackupDestination,
    DeviceBackupPrepareFuture, LogArchiveCommand, SysdiagnoseCommand,
    validate_log_archive_age_limit_hours,
};
pub use media::{
    BrowserFrameDecision, BrowserVideoFrame, BrowserVideoSlot, FrameCredit, FramePacer,
    FramePacerMetrics, RtcpOptions, audio_decoder_restart_backoff, browser_frame_decision,
    configured_in_flight_frames, duration_average_ms, encode_packet,
};
pub use performance::PerformanceDemand;
pub use preferences::RuntimePreferences;
pub use runtime::{CoreRuntime, OWNER_THREAD_STACK_BYTES};
pub use session::{
    DeviceSessionCommand, DiagnosticDumpSinkFactory, DiagnosticDumpSinkFuture, RuntimeHostAdapters,
    RuntimeSessionHostAdapters, SessionCommandSlot, SessionControlCommand, SessionDiagnostics,
    StartedRuntime, start_runtime,
};
pub use storage::{
    AppDocumentCommand, DeviceFileCommand, HostDirectoryEntry, HostFileFuture, HostFileIo,
    HostFileKind, HostFileMetadata, HostFileReader, HostFileWrite, HostFileWriter,
};
pub(crate) use supervisor::{
    ServiceReporter, ServiceSupervisor, reconnect_backoff, wait_for_retry,
};
pub use transport::{
    MuxSidecar, MuxSidecarFuture, PairingStore, StoredLockdownPairingRecord, SystemUsbmuxdConfig,
    WIFI_REAUTHORIZE_REQUIRED,
};
pub(crate) use transport::{
    SessionEndpoint, UsbmuxdEndpoint, connect_provider, resolve_device_selection,
};
