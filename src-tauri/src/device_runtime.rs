//! Host-independent ownership boundary for the Apple device runtime.
//!
//! This module owns the dedicated thread and the single session manager. It
//! intentionally starts no HTTP, MCP, Tauri, or frontend task; hosts compose
//! those adapters from the cloneable compatibility services returned here.

pub(crate) mod commands;
pub(crate) mod state;

use std::path::PathBuf;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use self::commands::ControlCmd;
use self::state::{
    ActiveSlot, AppOperationSlot, ClipboardSlot, DeviceListSlot, ErrorSlot, InputSink,
    LocationStatusSlot, OrientationSlot, StatusSlot, VideoCounters,
};
pub(crate) use devicehub_runtime::{AudioPublisher, PcmAudioConsumer, RuntimePreferences};

/// Host-resolved diagnostics applied to each device session.
///
/// Environment variables are parsed once by the desktop composition root. The
/// device thread only receives immutable values, which keeps session lifecycle
/// code independent from the host process environment.
pub(crate) use devicehub_runtime::SessionDiagnostics as RuntimeSessionDiagnostics;

pub(crate) struct RuntimeConfig {
    pub(crate) initial_udid: Option<String>,
    pub(crate) pairing_dir: PathBuf,
    pub(crate) transport: crate::session::DeviceTransportConfig,
    pub(crate) preferences: RuntimePreferences,
    pub(crate) audio: AudioPublisher,
    pub(crate) audio_decoder: crate::decode::AudioDecoderConfig,
    pub(crate) session_diagnostics: RuntimeSessionDiagnostics<PathBuf>,
}

/// Compatibility surface consumed by the current HTTP and MCP adapters.
///
/// Later extraction stages replace raw slots with typed `devicehub-core`
/// services. Keeping them in one value already prevents hosts from creating a
/// second, divergent set of device state owners.
#[derive(Clone)]
pub(crate) struct RuntimeServices {
    pub(crate) application: crate::application::ApplicationServices,
    pub(crate) browser_frames: crate::browser_video::BrowserVideoSlot,
    pub(crate) video_counters: VideoCounters,
    pub(crate) clipboard: ClipboardSlot,
    pub(crate) network_capture: crate::network_capture::NetworkCaptureSlot,
    pub(crate) bluetooth_capture: crate::bluetooth_capture::BluetoothCaptureSlot,
    pub(crate) device_backup: crate::device_backup::DeviceBackupSlot,
    pub(crate) sysdiagnose: crate::sysdiagnose::SysdiagnoseSlot,
    pub(crate) log_archive: crate::log_archive::LogArchiveSlot,
    pub(crate) developer_image: crate::developer_image::DeveloperImageMountSlot,
    pub(crate) device_conditions: devicehub_runtime::DeviceConditionSlot,
    pub(crate) app_operation: AppOperationSlot,
    pub(crate) app_document_activity: crate::app_documents::AppDocumentActivitySlot,
    pub(crate) device_file_activity: crate::device_files::DeviceFileActivitySlot,
    pub(crate) performance: devicehub_runtime::PerformanceSlot,
    pub(crate) performance_demand: devicehub_runtime::PerformanceDemand,
    pub(crate) device_logs: devicehub_runtime::DeviceLogSlot,
    pub(crate) device_log_demand: devicehub_runtime::DeviceLogDemand,
    pub(crate) service_registry: crate::supervisor::ServiceRegistry,
    pub(crate) input: InputSink,
}

struct RuntimeParts {
    services: RuntimeServices,
    status: StatusSlot,
    orientation: OrientationSlot,
    devices: DeviceListSlot,
    active: ActiveSlot,
    error: ErrorSlot,
    location: LocationStatusSlot,
    device_events: devicehub_runtime::DeviceEventSlot,
    control_rx: UnboundedReceiver<ControlCmd>,
}

pub(crate) struct DeviceRuntime {
    services: RuntimeServices,
    owner: devicehub_runtime::CoreRuntime,
}

impl DeviceRuntime {
    pub(crate) fn start(config: RuntimeConfig) -> Result<Self, String> {
        let (owner, services) =
            devicehub_runtime::CoreRuntime::start(move |control, control_rx| {
                let parts = RuntimeParts::new(control, control_rx);
                let services = parts.services.clone();
                let task = move || -> devicehub_runtime::CoreRuntimeFuture {
                    let session_services = parts.services;
                    Box::pin(crate::session::manage(
                        config.initial_udid,
                        config.pairing_dir,
                        config.transport,
                        config.preferences,
                        session_services.video_counters,
                        session_services.browser_frames,
                        config.audio,
                        config.audio_decoder,
                        config.session_diagnostics,
                        parts.status,
                        session_services.clipboard,
                        parts.device_events,
                        session_services.network_capture,
                        session_services.bluetooth_capture,
                        session_services.device_backup,
                        session_services.sysdiagnose,
                        session_services.log_archive,
                        session_services.developer_image,
                        session_services.device_conditions,
                        parts.orientation,
                        parts.devices,
                        parts.active,
                        parts.error,
                        session_services.app_operation,
                        session_services.app_document_activity,
                        session_services.device_file_activity,
                        parts.location,
                        session_services.performance,
                        session_services.performance_demand,
                        session_services.device_logs,
                        session_services.device_log_demand,
                        session_services.service_registry,
                        session_services.input,
                        parts.control_rx,
                    ))
                };
                (services, task)
            })?;
        Ok(Self { services, owner })
    }

    pub(crate) fn services(&self) -> RuntimeServices {
        self.services.clone()
    }

    pub(crate) fn stop(&self) {
        self.owner.stop();
    }
}

impl RuntimeParts {
    fn new(
        control: UnboundedSender<ControlCmd>,
        control_rx: UnboundedReceiver<ControlCmd>,
    ) -> Self {
        let browser_frames = crate::browser_video::BrowserVideoSlot::default();
        let video_counters = VideoCounters::default();
        let status = StatusSlot::default();
        let clipboard = ClipboardSlot::default();
        let device_events = devicehub_runtime::DeviceEventSlot::default();
        let network_capture = crate::network_capture::NetworkCaptureSlot::default();
        let bluetooth_capture = crate::bluetooth_capture::BluetoothCaptureSlot::default();
        let device_backup = crate::device_backup::DeviceBackupSlot::default();
        let sysdiagnose = crate::sysdiagnose::SysdiagnoseSlot::default();
        let log_archive = crate::log_archive::LogArchiveSlot::default();
        let developer_image = crate::developer_image::DeveloperImageMountSlot::default();
        let device_conditions = devicehub_runtime::DeviceConditionSlot::default();
        let orientation = OrientationSlot::default();
        let devices = DeviceListSlot::default();
        let active = ActiveSlot::default();
        let error = ErrorSlot::default();
        let input = InputSink::default();
        let app_operation = AppOperationSlot::default();
        let app_document_activity = crate::app_documents::AppDocumentActivitySlot::default();
        let device_file_activity = crate::device_files::DeviceFileActivitySlot::default();
        let location = LocationStatusSlot::default();
        let performance = devicehub_runtime::PerformanceSlot::default();
        let performance_demand = devicehub_runtime::PerformanceDemand::default();
        let device_logs = devicehub_runtime::DeviceLogSlot::default();
        let device_log_demand = devicehub_runtime::DeviceLogDemand::default();
        let service_registry = crate::supervisor::ServiceRegistry::default();
        let device_control =
            crate::application::DeviceControlService::new(browser_frames.clone(), input.clone());
        let application = crate::application::ApplicationServices::new(
            device_control,
            crate::application::DeviceStateSlots {
                orientation: orientation.clone(),
                devices: devices.clone(),
                active: active.clone(),
                error: error.clone(),
                status: status.clone(),
                location: location.clone(),
            },
            crate::application::ObservabilitySlots {
                device_events: device_events.clone(),
                device_conditions: device_conditions.clone(),
                performance: performance.clone(),
                performance_demand: performance_demand.clone(),
                device_logs: device_logs.clone(),
                device_log_demand: device_log_demand.clone(),
            },
            control.clone(),
        );
        let services = RuntimeServices {
            application,
            browser_frames,
            video_counters,
            clipboard,
            network_capture,
            bluetooth_capture,
            device_backup,
            sysdiagnose,
            log_archive,
            developer_image,
            device_conditions,
            app_operation,
            app_document_activity,
            device_file_activity,
            performance,
            performance_demand,
            device_logs,
            device_log_demand,
            service_registry,
            input,
        };
        Self {
            services,
            status,
            orientation,
            devices,
            active,
            error,
            location,
            device_events,
            control_rx,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_services_share_the_owner_control_plane() {
        let (control, control_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut parts = RuntimeParts::new(control, control_rx);

        parts
            .services
            .application
            .control
            .send(ControlCmd::Refresh)
            .unwrap();

        assert!(matches!(
            parts.control_rx.blocking_recv(),
            Some(ControlCmd::Refresh)
        ));
    }
}
