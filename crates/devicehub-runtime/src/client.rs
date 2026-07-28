//! Cloneable runtime client shared by host protocol adapters.

mod control;
mod registry;

use tokio::sync::mpsc::UnboundedSender;

use devicehub_core::{
    ActiveSlot, AppDocumentActivitySlot, AppOperationSlot, BluetoothCaptureSlot,
    DeveloperImageMountSlot, DeviceBackupSlot, DeviceConditionSlot, DeviceFileActivitySlot,
    DeviceListSlot, DeviceLogSlot, ErrorSlot, LocationStatusSlot, LogArchiveSlot,
    NetworkCaptureSlot, OrientationSlot, PerformanceSlot, ServiceRegistry, StatusSlot,
    SysdiagnoseSlot, VideoCounters,
};

pub use control::{DeviceControlError, DeviceControlService};
pub use registry::DeviceSessionRegistry;

use crate::runtime::CoreRuntimeState;
use crate::{
    BrowserVideoSlot, ClipboardSlot, DeviceEventSlot, DeviceLogDemand, PerformanceDemand,
    SessionCommandSlot, SessionControlCommand, SessionMediaDemand,
};

/// Device inventory, selection, and lifecycle commands owned by the outer
/// runtime manager rather than any connected device session.
#[derive(Clone)]
pub struct RuntimeManagerClient {
    pub devices: DeviceListSlot,
    pub active: ActiveSlot,
    pub control: UnboundedSender<SessionControlCommand>,
}

/// State and commands scoped to the runtime's current device session.
///
/// Keeping this surface separate from [`RuntimeManagerClient`] makes device
/// ownership explicit before a host starts retaining several sessions.
pub struct DeviceSessionClient<HostPath> {
    pub device_control: DeviceControlService<HostPath>,
    pub orientation: OrientationSlot,
    pub error: ErrorSlot,
    pub status: StatusSlot,
    pub location: LocationStatusSlot,
    pub browser_frames: BrowserVideoSlot,
    pub media_demand: SessionMediaDemand,
    pub video_counters: VideoCounters,
    pub clipboard: ClipboardSlot,
    pub device_events: DeviceEventSlot,
    pub network_capture: NetworkCaptureSlot,
    pub bluetooth_capture: BluetoothCaptureSlot,
    pub device_backup: DeviceBackupSlot,
    pub sysdiagnose: SysdiagnoseSlot,
    pub log_archive: LogArchiveSlot,
    pub developer_image: DeveloperImageMountSlot,
    pub device_conditions: DeviceConditionSlot,
    pub app_operation: AppOperationSlot,
    pub app_documents: AppDocumentActivitySlot,
    pub device_files: DeviceFileActivitySlot,
    pub performance: PerformanceSlot,
    pub performance_demand: PerformanceDemand,
    pub device_logs: DeviceLogSlot,
    pub device_log_demand: DeviceLogDemand,
    pub service_registry: ServiceRegistry,
    pub commands: SessionCommandSlot<HostPath>,
}

impl<HostPath> Clone for DeviceSessionClient<HostPath> {
    fn clone(&self) -> Self {
        Self {
            device_control: self.device_control.clone(),
            orientation: self.orientation.clone(),
            error: self.error.clone(),
            status: self.status.clone(),
            location: self.location.clone(),
            browser_frames: self.browser_frames.clone(),
            media_demand: self.media_demand.clone(),
            video_counters: self.video_counters.clone(),
            clipboard: self.clipboard.clone(),
            device_events: self.device_events.clone(),
            network_capture: self.network_capture.clone(),
            bluetooth_capture: self.bluetooth_capture.clone(),
            device_backup: self.device_backup.clone(),
            sysdiagnose: self.sysdiagnose.clone(),
            log_archive: self.log_archive.clone(),
            developer_image: self.developer_image.clone(),
            device_conditions: self.device_conditions.clone(),
            app_operation: self.app_operation.clone(),
            app_documents: self.app_documents.clone(),
            device_files: self.device_files.clone(),
            performance: self.performance.clone(),
            performance_demand: self.performance_demand.clone(),
            device_logs: self.device_logs.clone(),
            device_log_demand: self.device_log_demand.clone(),
            service_registry: self.service_registry.clone(),
            commands: self.commands.clone(),
        }
    }
}

impl<HostPath> DeviceSessionClient<HostPath> {
    pub(crate) fn from_state(state: &crate::runtime::DeviceSessionState<HostPath>) -> Self {
        Self {
            device_control: DeviceControlService::new(
                state.browser_frames.clone(),
                state.commands.clone(),
            ),
            orientation: state.orientation.clone(),
            error: state.error.clone(),
            status: state.status.clone(),
            location: state.location.clone(),
            browser_frames: state.browser_frames.clone(),
            media_demand: state.media_demand.clone(),
            video_counters: state.video_counters.clone(),
            clipboard: state.clipboard.clone(),
            device_events: state.device_events.clone(),
            network_capture: state.network_capture.clone(),
            bluetooth_capture: state.bluetooth_capture.clone(),
            device_backup: state.device_backup.clone(),
            sysdiagnose: state.sysdiagnose.clone(),
            log_archive: state.log_archive.clone(),
            developer_image: state.developer_image.clone(),
            device_conditions: state.device_conditions.clone(),
            app_operation: state.app_operation.clone(),
            app_documents: state.app_documents.clone(),
            device_files: state.device_files.clone(),
            performance: state.performance.clone(),
            performance_demand: state.performance_demand.clone(),
            device_logs: state.device_logs.clone(),
            device_log_demand: state.device_log_demand.clone(),
            service_registry: state.services.clone(),
            commands: state.commands.clone(),
        }
    }
}

/// Shared surface consumed by HTTP, WebSocket, MCP, and future headless hosts.
///
/// The runtime creates both views from one state graph. Hosts may clone them,
/// but cannot construct a second manager or divergent device session.
#[derive(Clone)]
pub struct RuntimeClient<HostPath> {
    pub manager: RuntimeManagerClient,
    /// All connected or connecting device sessions, keyed by transport-aware
    /// selection ID. `device` remains the single-session migration view until
    /// every host adapter resolves a session explicitly.
    pub sessions: DeviceSessionRegistry<HostPath>,
    pub device: DeviceSessionClient<HostPath>,
}

impl<HostPath> RuntimeClient<HostPath> {
    pub(crate) fn from_state(
        state: &CoreRuntimeState<HostPath>,
        control: UnboundedSender<SessionControlCommand>,
    ) -> Self {
        Self {
            manager: RuntimeManagerClient {
                devices: state.manager.devices.clone(),
                active: state.manager.active.clone(),
                control,
            },
            device: DeviceSessionClient::from_state(&state.device),
            sessions: state.sessions.clone(),
        }
    }
}

#[cfg(feature = "test-support")]
pub struct RuntimeClientFixture<HostPath> {
    state: CoreRuntimeState<HostPath>,
}

#[cfg(feature = "test-support")]
impl<HostPath> Default for RuntimeClientFixture<HostPath> {
    fn default() -> Self {
        Self {
            state: CoreRuntimeState::default(),
        }
    }
}

#[cfg(feature = "test-support")]
impl<HostPath> RuntimeClientFixture<HostPath> {
    pub fn with_session(
        self,
        selection_id: impl Into<String>,
        session: DeviceSessionClient<HostPath>,
    ) -> Self {
        self.state.sessions.insert(selection_id.into(), session);
        self
    }

    pub fn with_browser_frames(mut self, value: BrowserVideoSlot) -> Self {
        self.state.device.browser_frames = value;
        self
    }

    pub fn with_commands(mut self, value: SessionCommandSlot<HostPath>) -> Self {
        self.state.device.commands = value;
        self
    }

    pub fn with_orientation(mut self, value: OrientationSlot) -> Self {
        self.state.device.orientation = value;
        self
    }

    pub fn with_devices(mut self, value: DeviceListSlot) -> Self {
        self.state.manager.devices = value;
        self
    }

    pub fn with_active(mut self, value: ActiveSlot) -> Self {
        self.state.manager.active = value;
        self
    }

    pub fn with_error(mut self, value: ErrorSlot) -> Self {
        self.state.device.error = value;
        self
    }

    pub fn with_status(mut self, value: StatusSlot) -> Self {
        self.state.device.status = value;
        self
    }

    pub fn with_location(mut self, value: LocationStatusSlot) -> Self {
        self.state.device.location = value;
        self
    }

    pub fn with_device_events(mut self, value: DeviceEventSlot) -> Self {
        self.state.device.device_events = value;
        self
    }

    pub fn with_device_conditions(mut self, value: DeviceConditionSlot) -> Self {
        self.state.device.device_conditions = value;
        self
    }

    pub fn with_performance(mut self, value: PerformanceSlot) -> Self {
        self.state.device.performance = value;
        self
    }

    pub fn with_performance_demand(mut self, value: PerformanceDemand) -> Self {
        self.state.device.performance_demand = value;
        self
    }

    pub fn with_device_logs(mut self, value: DeviceLogSlot) -> Self {
        self.state.device.device_logs = value;
        self
    }

    pub fn with_device_log_demand(mut self, value: DeviceLogDemand) -> Self {
        self.state.device.device_log_demand = value;
        self
    }

    pub fn build(
        self,
    ) -> (
        RuntimeClient<HostPath>,
        tokio::sync::mpsc::UnboundedReceiver<SessionControlCommand>,
    ) {
        let (control, receiver) = tokio::sync::mpsc::unbounded_channel();
        (self.state.client(control), receiver)
    }

    pub fn build_with_control(
        self,
        control: UnboundedSender<SessionControlCommand>,
    ) -> RuntimeClient<HostPath> {
        self.state.client(control)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::CoreRuntimeState;

    #[test]
    fn clones_share_state_demand_and_control_ownership() {
        let state = CoreRuntimeState::<String>::default();
        let (control, mut commands) = tokio::sync::mpsc::unbounded_channel();
        let client = state.client(control);
        let adapter_clone = client.clone();

        state.device.status.set("connected");
        assert_eq!(adapter_clone.device.status.get(), "connected");
        let demand = client.device.performance_demand.acquire();
        assert!(state.device.performance_demand.enabled());
        drop(demand);
        assert!(!adapter_clone.device.performance_demand.enabled());

        client
            .manager
            .control
            .send(SessionControlCommand::Refresh)
            .unwrap();
        assert!(matches!(
            commands.try_recv(),
            Ok(SessionControlCommand::Refresh)
        ));
    }
}
