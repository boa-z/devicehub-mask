//! Cloneable runtime client shared by host protocol adapters.

mod control;

use tokio::sync::mpsc::UnboundedSender;

use devicehub_core::{
    ActiveSlot, DeviceListSlot, ErrorSlot, LocationStatusSlot, OrientationSlot, StatusSlot,
};

pub use control::{DeviceControlError, DeviceControlService};

use crate::{
    CoreRuntimeState, DeviceConditionSlot, DeviceEventSlot, DeviceLogDemand, DeviceLogSlot,
    PerformanceDemand, PerformanceSlot, SessionControlCommand,
};

/// Shared state and command surface consumed by HTTP, WebSocket, MCP, and
/// future headless adapters. The runtime creates this value from its sole state
/// graph; hosts can clone it but cannot construct a divergent client.
pub struct RuntimeClient<HostPath> {
    pub device_control: DeviceControlService<HostPath>,
    pub orientation: OrientationSlot,
    pub devices: DeviceListSlot,
    pub active: ActiveSlot,
    pub error: ErrorSlot,
    pub status: StatusSlot,
    pub location: LocationStatusSlot,
    pub device_events: DeviceEventSlot,
    pub device_conditions: DeviceConditionSlot,
    pub performance: PerformanceSlot,
    pub performance_demand: PerformanceDemand,
    pub device_logs: DeviceLogSlot,
    pub device_log_demand: DeviceLogDemand,
    pub control: UnboundedSender<SessionControlCommand>,
}

impl<HostPath> RuntimeClient<HostPath> {
    pub(crate) fn from_state(
        state: &CoreRuntimeState<HostPath>,
        control: UnboundedSender<SessionControlCommand>,
    ) -> Self {
        Self {
            device_control: DeviceControlService::new(
                state.browser_frames.clone(),
                state.commands.clone(),
            ),
            orientation: state.orientation.clone(),
            devices: state.devices.clone(),
            active: state.active.clone(),
            error: state.error.clone(),
            status: state.status.clone(),
            location: state.location.clone(),
            device_events: state.device_events.clone(),
            device_conditions: state.device_conditions.clone(),
            performance: state.performance.clone(),
            performance_demand: state.performance_demand.clone(),
            device_logs: state.device_logs.clone(),
            device_log_demand: state.device_log_demand.clone(),
            control,
        }
    }
}

impl<HostPath> Clone for RuntimeClient<HostPath> {
    fn clone(&self) -> Self {
        Self {
            device_control: self.device_control.clone(),
            orientation: self.orientation.clone(),
            devices: self.devices.clone(),
            active: self.active.clone(),
            error: self.error.clone(),
            status: self.status.clone(),
            location: self.location.clone(),
            device_events: self.device_events.clone(),
            device_conditions: self.device_conditions.clone(),
            performance: self.performance.clone(),
            performance_demand: self.performance_demand.clone(),
            device_logs: self.device_logs.clone(),
            device_log_demand: self.device_log_demand.clone(),
            control: self.control.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CoreRuntimeState;

    #[test]
    fn clones_share_state_demand_and_control_ownership() {
        let state = CoreRuntimeState::<String>::default();
        let (control, mut commands) = tokio::sync::mpsc::unbounded_channel();
        let client = state.client(control);
        let adapter_clone = client.clone();

        state.status.set("connected");
        assert_eq!(adapter_clone.status.get(), "connected");
        let demand = client.performance_demand.acquire();
        assert!(state.performance_demand.enabled());
        drop(demand);
        assert!(!adapter_clone.performance_demand.enabled());

        client.control.send(SessionControlCommand::Refresh).unwrap();
        assert!(matches!(
            commands.try_recv(),
            Ok(SessionControlCommand::Refresh)
        ));
    }
}
