//! Desktop composition for the runtime-owned session manager.

use std::path::PathBuf;

use tokio::sync::mpsc::UnboundedReceiver;

use super::{clipboard, diagnostics, services};
use crate::device_runtime::AudioPublisher;
use crate::protocol::{
    ActiveSlot, AppOperationSlot, ClipboardSlot, ControlCmd, DeviceListSlot, ErrorSlot, InputSink,
    LocationStatusSlot, OrientationSlot, StatusSlot, VideoCounters,
};
use crate::supervisor;
use devicehub_runtime::{
    ConnectedSessionViews, CoreTunnelConfig, DeviceDiscovery, RuntimeHostServiceViews,
    RuntimeServiceViews, SessionManagerHost, SessionManagerViews, run_session_manager,
};

/// Bind desktop filesystem, process, and clipboard capabilities to the shared
/// runtime manager. Selection, trust, reconnect, and teardown policy stay in
/// `devicehub-runtime`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn manage(
    initial_udid: Option<String>,
    pairing_dir: PathBuf,
    transport: super::DeviceTransportConfig,
    preferences: crate::device_runtime::RuntimePreferences,
    video_counters: VideoCounters,
    browser_frames: crate::browser_video::BrowserVideoSlot,
    audio: AudioPublisher,
    audio_decoder: crate::decode::AudioDecoderConfig,
    session_diagnostics: crate::device_runtime::RuntimeSessionDiagnostics<PathBuf>,
    status: StatusSlot,
    clipboard: ClipboardSlot,
    device_events: devicehub_runtime::DeviceEventSlot,
    network_capture: crate::network_capture::NetworkCaptureSlot,
    bluetooth_capture: crate::bluetooth_capture::BluetoothCaptureSlot,
    device_backup: crate::device_backup::DeviceBackupSlot,
    sysdiagnose: crate::sysdiagnose::SysdiagnoseSlot,
    log_archive: crate::log_archive::LogArchiveSlot,
    developer_image: crate::developer_image::DeveloperImageMountSlot,
    device_conditions: devicehub_runtime::DeviceConditionSlot,
    orientation: OrientationSlot,
    device_list: DeviceListSlot,
    active: ActiveSlot,
    error: ErrorSlot,
    app_operation: AppOperationSlot,
    app_document_activity: crate::app_documents::AppDocumentActivitySlot,
    device_file_activity: crate::device_files::DeviceFileActivitySlot,
    location: LocationStatusSlot,
    performance: devicehub_runtime::PerformanceSlot,
    performance_demand: devicehub_runtime::PerformanceDemand,
    device_logs: devicehub_runtime::DeviceLogSlot,
    device_log_demand: devicehub_runtime::DeviceLogDemand,
    service_registry: supervisor::ServiceRegistry,
    input: InputSink,
    control_rx: UnboundedReceiver<ControlCmd>,
) {
    let tunnel = CoreTunnelConfig::from_host(pairing_dir.clone(), transport.system_usbmuxd);
    let sidecar = crate::netmuxd::NetmuxdSupervisor::new(pairing_dir.clone(), transport.netmuxd);
    let wifi_store = match crate::wifi_devices::HostWifiPairingStore::new(pairing_dir) {
        Ok(store) => Some(store),
        Err(error) => {
            tracing::warn!(%error, "Wi-Fi pairing storage unavailable; continuing with usbmuxd");
            None
        }
    };
    let discovery = DeviceDiscovery::new(sidecar, wifi_store, tunnel.clone());
    let runtime_services = RuntimeServiceViews {
        performance,
        performance_demand,
        device_logs,
        device_log_demand,
        services: service_registry,
        device_events,
        location,
        device_conditions,
    };
    let host_services = RuntimeHostServiceViews {
        app_documents: app_document_activity,
        device_files: device_file_activity,
        network_capture,
        bluetooth_capture,
        device_backup,
        sysdiagnose,
        log_archive,
        developer_image,
    };

    run_session_manager(
        initial_udid,
        preferences,
        session_diagnostics,
        SessionManagerHost {
            discovery,
            tunnel,
            audio: crate::decode::FfmpegAudioPipelineFactory::new(audio, audio_decoder),
            diagnostic_sinks: diagnostics::TokioDiagnosticDumpSinks,
            clipboard: clipboard::ArboardClipboardProvider,
            services: services::adapters(),
        },
        SessionManagerViews {
            connected: ConnectedSessionViews {
                status,
                orientation,
                error,
                app_operation,
                clipboard,
                video_counters,
                browser_frames,
                runtime_services,
                host_services,
            },
            devices: device_list,
            active,
            commands: input,
        },
        control_rx,
    )
    .await;
}
