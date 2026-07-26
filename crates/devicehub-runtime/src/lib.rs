//! Host-independent runtime capabilities for DeviceHub.
//!
//! Desktop, HTTP, and MCP adapters inject implementations through these ports;
//! none of those host frameworks may become dependencies of this crate.

mod applications;
mod audio;
mod capture;
mod clipboard;
mod demand;
mod device;
mod diagnostics;
mod input;
mod media;
mod performance;
mod preferences;
mod session;
mod storage;
mod supervisor;
mod transport;

pub use applications::{
    APP_CONTROL_REQUEST_TIMEOUT, APP_LIST_REQUEST_TIMEOUT, AppClientSet, AppCommand,
    AppConsoleCommand, AppConsoleLine, AppConsolePhase, AppConsoleSnapshot, AppIconCommand,
    AppLifecycleCommand, AppManagement, AppServiceTransport, DEFAULT_SOURCE_CHARS,
    MAX_ATTRIBUTE_BYTES, MAX_ATTRIBUTE_CHARACTERS, MAX_BACKGROUND_DURATION_MS, MAX_ELEMENTS,
    MAX_HOLD_DURATION_MS, MAX_SELECTOR_BYTES, MAX_SOURCE_CHARS, MAX_TEXT_BYTES,
    MAX_TEXT_CHARACTERS, MAX_WAIT_TIMEOUT_MS, MIN_BACKGROUND_DURATION_MS, MIN_HOLD_DURATION_MS,
    RunningProcessCommand, WdaAutomationCommand, WdaBoundedText, WdaDeviceState, WdaElement,
    WdaElementDetails, WdaElementWaitResult, WdaElementWaitState, WdaOrientation, WdaRect,
    WdaRunnerCommand, WdaRunnerPhase, WdaRunnerStatus, WdaSize, WdaStatus, WdaUiTree,
    WdaUnlockResult, parse_wait_state, serve_app_console, serve_app_icons, serve_app_lifecycle,
    serve_running_processes, serve_wda_automation, serve_wda_runner, validate_background_duration,
    validate_hold_duration, validate_runner_bundle_id, validate_scroll_direction,
    validate_selector, validate_text, validate_wait_timeout,
};
pub use audio::{AudioPublisher, PcmAudioConsumer};
pub use capture::{
    BluetoothCaptureCommand, BluetoothCaptureSlot, BluetoothCaptureState, BluetoothCaptureStatus,
    BluetoothCaptureStopReason, BluetoothCaptureTransport, CaptureFileFuture, CaptureFileIo,
    CaptureFileKind, CaptureFileWriter, MAX_BLUETOOTH_CAPTURE_DURATION_SECONDS,
    MAX_NETWORK_CAPTURE_DURATION_SECONDS, MIN_BLUETOOTH_CAPTURE_DURATION_SECONDS,
    MIN_NETWORK_CAPTURE_DURATION_SECONDS, NetworkCaptureCommand, NetworkCaptureSlot,
    NetworkCaptureState, NetworkCaptureStatus, NetworkCaptureStopReason, NetworkCaptureTransport,
    serve_bluetooth_capture, serve_network_capture, validate_bluetooth_capture_duration,
    validate_network_capture_duration,
};
pub use clipboard::ClipboardSlot;
pub use demand::{Demand, DemandLease};
pub use device::{
    CompanionDeviceCommand, CrashReportExportCommand, DeveloperImageAssetFuture,
    DeveloperImageAssetLoader, DeveloperImageMountCommand, DeveloperImageMountRequest,
    DeveloperImageMountSlot, DeveloperImageMountState, DeveloperImageMountStatus,
    DeveloperModeCommand, DeveloperModePreparation, DeviceConditionCommand, DeviceConditionSlot,
    DeviceEventSlot, DeviceLogBatch, DeviceLogDemand, DeviceLogEntry, DeviceLogLevel,
    DeviceLogSlot, DeviceLogSource, DevicePowerAction, DevicePowerController, HomeScreenCommand,
    LocationCommand, MAX_BATCH_ENTRIES, MAX_CRASH_REPORT_READ_BYTES,
    MAX_PROVISIONING_PROFILE_BYTES, ProvisioningCommand, ProvisioningFailure, ProvisioningInstall,
    ScreenCaptureCommand, ScreenCaptureTransport, delete_crash_report,
    developer_image_type_for_version, download_crash_report, execute_developer_mode,
    is_developer_image_mounted, is_developer_image_mounted_for_device, list_crash_reports,
    parse_provisioning_profile, prepare_provisioning_install, profiles_from_raw,
    read_activation_state, read_crash_report, read_developer_mode_status, read_device_battery,
    read_device_details, read_device_developer_mode_status, read_device_product_version,
    rename_device, serve_companion_devices, serve_developer_image_mount, serve_home_screen,
    serve_screen_capture, supervise_device_conditions, supervise_device_events,
    supervise_device_logs, supervise_location, supervise_provisioning, unreadable_profile,
    validate_crash_report_path, validate_device_condition_identifiers,
};
pub use diagnostics::{
    ALLOWED_LOG_ARCHIVE_AGE_LIMIT_HOURS, DeviceBackupCommand, DeviceBackupExecutor,
    DeviceBackupFuture, DeviceBackupPrepareFuture, DeviceBackupSlot, DeviceBackupState,
    DeviceBackupStatus, DeviceBackupTransport, LogArchiveCommand, LogArchiveSlot, LogArchiveState,
    LogArchiveStatus, SysdiagnoseCommand, SysdiagnoseSlot, SysdiagnoseState, SysdiagnoseStatus,
    serve_device_backup, serve_log_archive, serve_sysdiagnose,
    validate_log_archive_age_limit_hours,
};
pub use input::{DeviceInputCommand, DeviceInputDispatcher, TouchContact, UniversalHidClient};
pub use media::{
    AccessUnitAssembler, BrowserFrameDecision, BrowserVideoFrame, BrowserVideoSlot, FrameCredit,
    FramePacer, FramePacerMetrics, HEVC_QUEUE_MAX_BYTES, HevcAccessUnit, HevcQueue, HevcQueuePush,
    HevcQueueSnapshot, MediaSessionConfig, MediaSessionRuntime, RtcpOptions, RtcpShared,
    RtpVideoClock, RunningStats, ScreenMediaStream, VideoRtpOptions, audio_decoder_restart_backoff,
    browser_frame_decision, configured_in_flight_frames, duration_average_ms, encode_packet,
    forward_keyframe_requests, hevc_dimensions, publish_hevc_queue, receive_audio_rtp,
    receive_rtcp, receive_video_rtp, send_rtcp, stall_watchdog, start_screen_media_stream,
};
pub use performance::{
    PerformanceDemand, PerformanceSlot, supervise_performance_app_activity,
    supervise_performance_energy, supervise_performance_graphics, supervise_performance_network,
    supervise_performance_system,
};
pub use preferences::RuntimePreferences;
pub use session::{
    ClipboardWriteFuture, DeviceClipboard, DeviceServicePorts, DeviceSessionCommand,
    DeviceSessionRouter, LocationServicePort, OrientationWatcher, RuntimeDeviceServicePorts,
    RuntimeServiceViews, RuntimeSessionServices, SessionControlCommand, SessionFailureAction,
    SessionRetry, SessionRetryPolicy, run_device_command_loop, run_management_command_loop,
    supervise_heartbeat,
};
pub use storage::{
    APP_DOCUMENT_TRANSFER_CANCELLED, AppDocumentActivityKind, AppDocumentActivitySlot,
    AppDocumentActivityState, AppDocumentActivityView, AppDocumentCommand, AppDocumentEntry,
    AppDocumentKind, AppDocumentList, AppDocumentTransfer, AppStorageScope, AppStorageTransport,
    DeviceFileActivityKind, DeviceFileActivitySlot, DeviceFileActivityState,
    DeviceFileActivityView, DeviceFileCommand, DeviceFileEntry, DeviceFileKind, DeviceFileList,
    DeviceFileTransfer, DeviceFileTransport, HostDirectoryEntry, HostFileFuture, HostFileIo,
    HostFileKind, HostFileMetadata, HostFileReader, HostFileWrite, HostFileWriter,
    TRANSFER_CANCELLED, is_app_document_transfer_cancelled, is_transfer_cancelled,
    serve_app_documents, serve_device_files,
};
pub use supervisor::{
    ServiceHealth, ServicePhase, ServiceRegistry, ServiceReporter, ServiceSupervisor,
    reconnect_backoff, wait_for_retry,
};
pub use transport::{
    SessionEndpoint, SystemUsbmuxdConfig, UsbmuxdEndpoint, WIFI_REAUTHORIZE_REQUIRED, WifiEndpoint,
    connect_core_tunnel, connect_provider, connection_kind, connection_kind_priority,
    connection_priority, remove_remote_pairing_credentials, resolve_device_selection,
    select_preferred_usbmuxd_device, uses_usbmuxd_core_proxy, wifi_provider,
};
