//! Manager and device-session state owned by one runtime instance.

use tokio::sync::mpsc::UnboundedSender;

use devicehub_core::{
    ActiveSlot, AppDocumentActivitySlot, AppOperationSlot, BluetoothCaptureSlot,
    DeveloperImageMountSlot, DeviceBackupSlot, DeviceConditionSlot, DeviceFileActivitySlot,
    DeviceListSlot, DeviceLogSlot, ErrorSlot, LocationStatusSlot, LogArchiveSlot,
    NetworkCaptureSlot, OrientationSlot, PerformanceSlot, ServiceRegistry, StatusSlot,
    SysdiagnoseSlot, VideoCounters,
};

use crate::session::{
    ConnectedSessionViews, RuntimeHostServiceViews, RuntimeServiceViews, SessionManagerViews,
};
use crate::{
    BrowserVideoSlot, ClipboardSlot, DeviceEventSlot, DeviceLogDemand, DeviceSessionRegistry,
    PerformanceDemand, RuntimeClient, SessionCommandSlot, SessionControlCommand,
};

/// Discovery and selection state owned by the outer runtime manager.
#[derive(Default)]
pub(crate) struct RuntimeManagerState {
    pub(crate) devices: DeviceListSlot,
    pub(crate) active: ActiveSlot,
}

/// State graph associated with the runtime's current device session.
pub(crate) struct DeviceSessionState<HostPath> {
    pub(crate) status: StatusSlot,
    pub(crate) orientation: OrientationSlot,
    pub(crate) error: ErrorSlot,
    pub(crate) location: LocationStatusSlot,
    pub(crate) browser_frames: BrowserVideoSlot,
    pub(crate) video_counters: VideoCounters,
    pub(crate) clipboard: ClipboardSlot,
    pub(crate) device_events: DeviceEventSlot,
    pub(crate) network_capture: NetworkCaptureSlot,
    pub(crate) bluetooth_capture: BluetoothCaptureSlot,
    pub(crate) device_backup: DeviceBackupSlot,
    pub(crate) sysdiagnose: SysdiagnoseSlot,
    pub(crate) log_archive: LogArchiveSlot,
    pub(crate) developer_image: DeveloperImageMountSlot,
    pub(crate) device_conditions: DeviceConditionSlot,
    pub(crate) app_operation: AppOperationSlot,
    pub(crate) app_documents: AppDocumentActivitySlot,
    pub(crate) device_files: DeviceFileActivitySlot,
    pub(crate) performance: PerformanceSlot,
    pub(crate) performance_demand: PerformanceDemand,
    pub(crate) device_logs: DeviceLogSlot,
    pub(crate) device_log_demand: DeviceLogDemand,
    pub(crate) services: ServiceRegistry,
    pub(crate) commands: SessionCommandSlot<HostPath>,
}

impl<HostPath> Default for DeviceSessionState<HostPath> {
    fn default() -> Self {
        Self {
            status: StatusSlot::default(),
            orientation: OrientationSlot::default(),
            error: ErrorSlot::default(),
            location: LocationStatusSlot::default(),
            browser_frames: BrowserVideoSlot::default(),
            video_counters: VideoCounters::default(),
            clipboard: ClipboardSlot::default(),
            device_events: DeviceEventSlot::default(),
            network_capture: NetworkCaptureSlot::default(),
            bluetooth_capture: BluetoothCaptureSlot::default(),
            device_backup: DeviceBackupSlot::default(),
            sysdiagnose: SysdiagnoseSlot::default(),
            log_archive: LogArchiveSlot::default(),
            developer_image: DeveloperImageMountSlot::default(),
            device_conditions: DeviceConditionSlot::default(),
            app_operation: AppOperationSlot::default(),
            app_documents: AppDocumentActivitySlot::default(),
            device_files: DeviceFileActivitySlot::default(),
            performance: PerformanceSlot::default(),
            performance_demand: PerformanceDemand::default(),
            device_logs: DeviceLogSlot::default(),
            device_log_demand: DeviceLogDemand::default(),
            services: ServiceRegistry::default(),
            commands: SessionCommandSlot::default(),
        }
    }
}

impl<HostPath> DeviceSessionState<HostPath> {
    pub(crate) fn client(&self) -> crate::DeviceSessionClient<HostPath> {
        crate::DeviceSessionClient::from_state(self)
    }

    pub(crate) fn connected_views(&self) -> ConnectedSessionViews {
        ConnectedSessionViews {
            status: self.status.clone(),
            orientation: self.orientation.clone(),
            error: self.error.clone(),
            app_operation: self.app_operation.clone(),
            clipboard: self.clipboard.clone(),
            video_counters: self.video_counters.clone(),
            browser_frames: self.browser_frames.clone(),
            runtime_services: RuntimeServiceViews {
                performance: self.performance.clone(),
                performance_demand: self.performance_demand.clone(),
                device_logs: self.device_logs.clone(),
                device_log_demand: self.device_log_demand.clone(),
                services: self.services.clone(),
                device_events: self.device_events.clone(),
                location: self.location.clone(),
                device_conditions: self.device_conditions.clone(),
            },
            host_services: RuntimeHostServiceViews {
                app_documents: self.app_documents.clone(),
                device_files: self.device_files.clone(),
                network_capture: self.network_capture.clone(),
                bluetooth_capture: self.bluetooth_capture.clone(),
                device_backup: self.device_backup.clone(),
                sysdiagnose: self.sysdiagnose.clone(),
                log_archive: self.log_archive.clone(),
                developer_image: self.developer_image.clone(),
            },
        }
    }
}

/// Internal state graph owned by one runtime and observed through clients.
///
/// The host path remains opaque to the runtime. Both ownership groups are
/// created together so hosts cannot observe a divergent manager or device
/// session state graph.
pub(crate) struct CoreRuntimeState<HostPath> {
    pub(crate) manager: RuntimeManagerState,
    pub(crate) device: DeviceSessionState<HostPath>,
    pub(crate) sessions: DeviceSessionRegistry<HostPath>,
}

impl<HostPath> Default for CoreRuntimeState<HostPath> {
    fn default() -> Self {
        Self {
            manager: RuntimeManagerState::default(),
            device: DeviceSessionState::default(),
            sessions: DeviceSessionRegistry::default(),
        }
    }
}

impl<HostPath> CoreRuntimeState<HostPath> {
    /// Create the cloneable host client for this sole runtime state graph.
    pub(crate) fn client(
        &self,
        control: UnboundedSender<SessionControlCommand>,
    ) -> RuntimeClient<HostPath> {
        RuntimeClient::from_state(self, control)
    }

    /// Build the complete manager view from the sole shared state graph.
    pub(crate) fn manager_views(&self) -> SessionManagerViews<HostPath> {
        SessionManagerViews {
            connected: self.device.connected_views(),
            devices: self.manager.devices.clone(),
            active: self.manager.active.clone(),
            sessions: self.sessions.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CoreRuntimeState;

    #[test]
    fn manager_views_share_the_single_runtime_state_graph() {
        let state = CoreRuntimeState::<String>::default();
        let views = state.manager_views();

        state.device.status.set("connected");
        state
            .manager
            .active
            .set_selected("device".into(), "device::usb".into());

        assert_eq!(views.connected.status.get(), "connected");
        assert_eq!(views.active.selection_id().as_deref(), Some("device::usb"));
        assert!(
            !state
                .device
                .commands
                .try_send(crate::DeviceSessionCommand::Shutdown)
        );
    }
}
