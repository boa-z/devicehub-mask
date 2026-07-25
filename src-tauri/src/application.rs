//! Application services shared by HTTP, WebSocket, and MCP adapters.

use std::fmt;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;

use crate::browser_video::BrowserVideoSlot;
use crate::device_events::DeviceEventSlot;
use crate::device_logs::{DeviceLogDemand, DeviceLogSlot};
use crate::performance::{PerformanceDemand, PerformanceSlot};
use crate::protocol::{
    ActiveSlot, ControlCmd, DeviceListSlot, ErrorSlot, InputCmd, InputSink, LocationStatusSlot,
    OrientationSlot, StatusSlot,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceControlError {
    Unavailable,
    SessionEnded,
    Timeout(&'static str),
    Operation(String),
}

impl fmt::Display for DeviceControlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => formatter.write_str("no active device session"),
            Self::SessionEnded => formatter.write_str("device session ended"),
            Self::Timeout(operation) => write!(formatter, "{operation} timed out"),
            Self::Operation(error) => formatter.write_str(error),
        }
    }
}

impl std::error::Error for DeviceControlError {}

/// Stable device identity and connection state shared by every transport
/// adapter. These slots are created once by the backend composition root.
#[derive(Clone, Default)]
pub struct DeviceStateSlots {
    pub(crate) orientation: OrientationSlot,
    pub(crate) devices: DeviceListSlot,
    pub(crate) active: ActiveSlot,
    pub(crate) error: ErrorSlot,
    pub(crate) status: StatusSlot,
    pub(crate) location: LocationStatusSlot,
}

/// Observability resources shared by HTTP and MCP without either adapter owning
/// the sampling lifecycle. Demand leases keep expensive probes off while idle.
#[derive(Clone, Default)]
pub struct ObservabilitySlots {
    pub(crate) device_events: DeviceEventSlot,
    pub(crate) device_conditions: crate::device_conditions::DeviceConditionSlot,
    pub(crate) performance: PerformanceSlot,
    pub(crate) performance_demand: PerformanceDemand,
    pub(crate) device_logs: DeviceLogSlot,
    pub(crate) device_log_demand: DeviceLogDemand,
}

#[derive(Clone)]
pub struct DeviceControlService {
    browser_frames: BrowserVideoSlot,
    input: InputSink,
}

/// Application-level facade consumed by HTTP, WebSocket, and MCP adapters.
///
/// Keeping this composition outside adapters ensures they observe the same
/// active session and demand counters. Protocol-specific request validation and
/// response formatting remain in the adapters.
#[derive(Clone)]
pub struct ApplicationServices {
    pub(crate) device_control: DeviceControlService,
    pub(crate) orientation: OrientationSlot,
    pub(crate) devices: DeviceListSlot,
    pub(crate) active: ActiveSlot,
    pub(crate) error: ErrorSlot,
    pub(crate) status: StatusSlot,
    pub(crate) location: LocationStatusSlot,
    pub(crate) device_events: DeviceEventSlot,
    pub(crate) device_conditions: crate::device_conditions::DeviceConditionSlot,
    pub(crate) performance: PerformanceSlot,
    pub(crate) performance_demand: PerformanceDemand,
    pub(crate) device_logs: DeviceLogSlot,
    pub(crate) device_log_demand: DeviceLogDemand,
    pub(crate) control: UnboundedSender<ControlCmd>,
}

impl ApplicationServices {
    pub fn new(
        device_control: DeviceControlService,
        state: DeviceStateSlots,
        observability: ObservabilitySlots,
        control: UnboundedSender<ControlCmd>,
    ) -> Self {
        let DeviceStateSlots {
            orientation,
            devices,
            active,
            error,
            status,
            location,
        } = state;
        let ObservabilitySlots {
            device_events,
            device_conditions,
            performance,
            performance_demand,
            device_logs,
            device_log_demand,
        } = observability;
        Self {
            device_control,
            orientation,
            devices,
            active,
            error,
            status,
            location,
            device_events,
            device_conditions,
            performance,
            performance_demand,
            device_logs,
            device_log_demand,
            control,
        }
    }
}

impl DeviceControlService {
    pub fn new(browser_frames: BrowserVideoSlot, input: InputSink) -> Self {
        Self {
            browser_frames,
            input,
        }
    }

    pub fn send(&self, command: InputCmd) -> Result<(), DeviceControlError> {
        self.input
            .try_send(command)
            .then_some(())
            .ok_or(DeviceControlError::Unavailable)
    }

    pub fn frame_version(&self) -> u64 {
        self.browser_frames.version()
    }

    pub fn browser_dimensions(&self) -> Option<(u32, u32)> {
        self.browser_frames.dimensions()
    }

    pub async fn capture_screenshot(
        &self,
        timeout: Duration,
    ) -> Result<Vec<u8>, DeviceControlError> {
        let (reply, response) = tokio::sync::oneshot::channel();
        self.send(InputCmd::TakeScreenshot(reply))?;
        tokio::time::timeout(timeout, response)
            .await
            .map_err(|_| DeviceControlError::Timeout("device screenshot request"))?
            .map_err(|_| DeviceControlError::SessionEnded)?
            .map_err(DeviceControlError::Operation)
    }

    pub async fn wait_for_frame(&self, after: u64, timeout: Duration) -> bool {
        if self.frame_version() > after {
            return true;
        }
        let mut browser = self.browser_frames.subscribe();
        // Close the publication race between the initial version check and
        // installing the compressed-frame subscription.
        if self.frame_version() > after {
            return true;
        }
        tokio::time::timeout(timeout, async {
            loop {
                let changed = browser.recv().await;
                if matches!(
                    changed,
                    Err(tokio::sync::broadcast::error::RecvError::Closed)
                ) {
                    return false;
                }
                if self.frame_version() > after {
                    return true;
                }
            }
        })
        .await
        .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::InputSink;
    use tokio::sync::mpsc::unbounded_channel;

    fn service() -> (
        DeviceControlService,
        tokio::sync::mpsc::UnboundedReceiver<InputCmd>,
    ) {
        let input = InputSink::default();
        let (sender, receiver) = unbounded_channel();
        input.set(Some(sender));
        (
            DeviceControlService::new(BrowserVideoSlot::default(), input),
            receiver,
        )
    }

    #[test]
    fn application_facade_clones_share_state_and_demand_ownership() {
        let state = DeviceStateSlots::default();
        let observability = ObservabilitySlots::default();
        let (control, mut commands) = unbounded_channel();
        let services = ApplicationServices::new(
            DeviceControlService::new(BrowserVideoSlot::default(), InputSink::default()),
            state.clone(),
            observability.clone(),
            control,
        );
        let adapter_clone = services.clone();

        state.status.set("connected");
        assert_eq!(adapter_clone.status.get(), "connected");
        let demand = services.performance_demand.acquire();
        assert!(observability.performance_demand.enabled());
        drop(demand);
        assert!(!adapter_clone.performance_demand.enabled());

        services.control.send(ControlCmd::Refresh).unwrap();
        assert!(matches!(commands.try_recv(), Ok(ControlCmd::Refresh)));
    }

    #[tokio::test]
    async fn screenshot_dispatches_through_the_active_session() {
        let (service, mut commands) = service();
        let request = tokio::spawn({
            let service = service.clone();
            async move { service.capture_screenshot(Duration::from_secs(1)).await }
        });
        let InputCmd::TakeScreenshot(reply) = commands.recv().await.unwrap() else {
            panic!("expected screenshot command");
        };
        reply.send(Ok(vec![1, 2, 3])).unwrap();
        assert_eq!(request.await.unwrap().unwrap(), vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn frame_wait_is_woken_by_browser_publication() {
        let browser = BrowserVideoSlot::default();
        let service = DeviceControlService::new(browser.clone(), InputSink::default());
        let waiter = tokio::spawn({
            let service = service.clone();
            async move { service.wait_for_frame(0, Duration::from_secs(1)).await }
        });
        tokio::task::yield_now().await;
        browser.publish(0, true, 100, 200, vec![0, 0, 0, 1, 0x26]);
        assert!(waiter.await.unwrap());
    }
}
