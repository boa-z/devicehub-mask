//! Host-independent ownership boundary for the Apple device runtime.
//!
//! This module owns the dedicated thread and the single session manager. It
//! intentionally starts no HTTP, MCP, Tauri, or frontend task; hosts compose
//! those adapters from the cloneable runtime client returned here.

use std::path::PathBuf;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use devicehub_core::{AppOperationSlot, VideoCounters};
use devicehub_runtime::ClipboardSlot;
pub(crate) use devicehub_runtime::{AudioPublisher, PcmAudioConsumer, RuntimePreferences};

/// Desktop host-path bindings for runtime-owned commands and command slots.
pub(crate) type InputCmd = devicehub_runtime::DeviceSessionCommand<PathBuf>;
pub(crate) type InputSink = devicehub_runtime::SessionCommandSlot<PathBuf>;
pub(crate) type ControlCmd = devicehub_runtime::SessionControlCommand;

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
    pub(crate) application: devicehub_runtime::RuntimeClient<PathBuf>,
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
    state: devicehub_runtime::CoreRuntimeState<PathBuf>,
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
                    Box::pin(crate::session::manage(
                        config.initial_udid,
                        config.pairing_dir,
                        config.transport,
                        config.preferences,
                        config.audio,
                        config.audio_decoder,
                        config.session_diagnostics,
                        parts.state,
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
        let state = devicehub_runtime::CoreRuntimeState::<PathBuf>::default();
        let application = state.client(control.clone());
        let services = RuntimeServices {
            application,
            browser_frames: state.browser_frames.clone(),
            video_counters: state.video_counters.clone(),
            clipboard: state.clipboard.clone(),
            network_capture: state.network_capture.clone(),
            bluetooth_capture: state.bluetooth_capture.clone(),
            device_backup: state.device_backup.clone(),
            sysdiagnose: state.sysdiagnose.clone(),
            log_archive: state.log_archive.clone(),
            developer_image: state.developer_image.clone(),
            device_conditions: state.device_conditions.clone(),
            app_operation: state.app_operation.clone(),
            app_document_activity: state.app_documents.clone(),
            device_file_activity: state.device_files.clone(),
            performance: state.performance.clone(),
            performance_demand: state.performance_demand.clone(),
            device_logs: state.device_logs.clone(),
            device_log_demand: state.device_log_demand.clone(),
            service_registry: state.services.clone(),
            input: state.commands.clone(),
        };
        Self {
            services,
            state,
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
