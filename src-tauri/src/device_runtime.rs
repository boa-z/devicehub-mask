//! Host-independent ownership boundary for the Apple device runtime.
//!
//! This module owns the dedicated thread and the single session manager. It
//! intentionally starts no HTTP, MCP, Tauri, or frontend task; hosts compose
//! those adapters from the cloneable compatibility services returned here.

pub(crate) mod commands;
pub(crate) mod state;

use std::path::PathBuf;
use std::sync::Mutex;
use std::thread::JoinHandle;

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use self::commands::ControlCmd;
use self::state::{
    ActiveSlot, AppOperationSlot, ClipboardSlot, DeviceListSlot, ErrorSlot, InputSink,
    LocationStatusSlot, OrientationSlot, StatusSlot, VideoCounters,
};
pub(crate) use devicehub_runtime::{AudioPublisher, PcmAudioConsumer, RuntimePreferences};

// RSD handshakes decode nested XPC dictionaries recursively. The owner also
// hosts a LocalSet for non-Send DVT channels, so platform thread defaults are
// insufficient for larger iOS service catalogs.
const DEVICE_THREAD_STACK_BYTES: usize = 16 * 1024 * 1024;

pub(crate) struct RuntimeConfig {
    pub(crate) initial_udid: Option<String>,
    pub(crate) pairing_dir: PathBuf,
    pub(crate) transport: crate::session::DeviceTransportConfig,
    pub(crate) preferences: RuntimePreferences,
    pub(crate) audio: AudioPublisher,
    pub(crate) audio_decoder: crate::decode::AudioDecoderConfig,
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
    control: UnboundedSender<ControlCmd>,
    control_rx: UnboundedReceiver<ControlCmd>,
}

pub(crate) struct DeviceRuntime {
    services: RuntimeServices,
    control: UnboundedSender<ControlCmd>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl DeviceRuntime {
    pub(crate) fn start(config: RuntimeConfig) -> Result<Self, String> {
        let parts = RuntimeParts::new();
        let services = parts.services.clone();
        let control = parts.control.clone();
        let thread = spawn_device_thread(config, parts)?;
        Ok(Self::from_parts(services, control, thread))
    }

    pub(crate) fn services(&self) -> RuntimeServices {
        self.services.clone()
    }

    pub(crate) fn request_shutdown(&self) {
        let _ = self.control.send(ControlCmd::Quit);
    }

    pub(crate) fn stop(&self) {
        self.request_shutdown();
        if let Some(thread) = self.thread.lock().unwrap().take() {
            let _ = thread.join();
        }
    }

    fn from_parts(
        services: RuntimeServices,
        control: UnboundedSender<ControlCmd>,
        thread: JoinHandle<()>,
    ) -> Self {
        Self {
            services,
            control,
            thread: Mutex::new(Some(thread)),
        }
    }
}

impl Drop for DeviceRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

impl RuntimeParts {
    fn new() -> Self {
        let (control, control_rx) = mpsc::unbounded_channel();
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
            control,
            control_rx,
        }
    }
}

fn spawn_device_thread(
    config: RuntimeConfig,
    parts: RuntimeParts,
) -> Result<JoinHandle<()>, String> {
    std::thread::Builder::new()
        .name("devicehub-coredevice".into())
        .stack_size(DEVICE_THREAD_STACK_BYTES)
        .spawn(move || {
            tracing::info!(
                stack_bytes = DEVICE_THREAD_STACK_BYTES,
                "CoreDevice owner thread started"
            );
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("build CoreDevice runtime");
            let local = tokio::task::LocalSet::new();
            let services = parts.services;
            runtime.block_on(local.run_until(crate::session::manage(
                config.initial_udid,
                config.pairing_dir,
                config.transport,
                config.preferences,
                services.video_counters,
                services.browser_frames,
                config.audio,
                config.audio_decoder,
                parts.status,
                services.clipboard,
                parts.device_events,
                services.network_capture,
                services.bluetooth_capture,
                services.device_backup,
                services.sysdiagnose,
                services.log_archive,
                services.developer_image,
                services.device_conditions,
                parts.orientation,
                parts.devices,
                parts.active,
                parts.error,
                services.app_operation,
                services.app_document_activity,
                services.device_file_activity,
                parts.location,
                services.performance,
                services.performance_demand,
                services.device_logs,
                services.device_log_demand,
                services.service_registry,
                services.input,
                parts.control_rx,
            )));
            tracing::info!("CoreDevice owner thread stopped");
        })
        .map_err(|error| format!("cannot start CoreDevice thread: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn runtime_services_share_the_owner_control_plane() {
        let mut parts = RuntimeParts::new();

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

    #[test]
    fn runtime_shutdown_is_idempotent_and_joins_its_owner() {
        let parts = RuntimeParts::new();
        let services = parts.services.clone();
        let control = parts.control.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let owner_stopped = stopped.clone();
        let mut receiver = parts.control_rx;
        let thread = std::thread::spawn(move || {
            while let Some(command) = receiver.blocking_recv() {
                if matches!(command, ControlCmd::Quit) {
                    break;
                }
            }
            owner_stopped.store(true, Ordering::Release);
        });
        let runtime = DeviceRuntime::from_parts(services, control, thread);

        runtime.stop();
        runtime.stop();

        assert!(stopped.load(Ordering::Acquire));
        assert!(runtime.thread.lock().unwrap().is_none());
    }
}
