//! Dedicated owner thread for the CoreDevice runtime.
//!
//! CoreDevice sessions include non-`Send` DVT channels and deeply nested XPC
//! decoding. One deliberately sized thread and one `LocalSet` therefore own the
//! complete manager lifecycle across desktop and future headless hosts.

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::thread::JoinHandle;

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use devicehub_core::{
    ActiveSlot, AppOperationSlot, DeviceListSlot, ErrorSlot, LocationStatusSlot, OrientationSlot,
    StatusSlot, VideoCounters,
};

use crate::session::{
    ConnectedSessionViews, RuntimeHostServiceViews, RuntimeServiceViews, SessionManagerViews,
};
use crate::{
    AppDocumentActivitySlot, BluetoothCaptureSlot, BrowserVideoSlot, ClipboardSlot,
    DeveloperImageMountSlot, DeviceBackupSlot, DeviceConditionSlot, DeviceEventSlot,
    DeviceFileActivitySlot, DeviceLogDemand, DeviceLogSlot, LogArchiveSlot, NetworkCaptureSlot,
    PerformanceDemand, PerformanceSlot, RuntimeClient, ServiceRegistry, SessionCommandSlot,
    SessionControlCommand, SysdiagnoseSlot,
};

const OWNER_THREAD_NAME: &str = "devicehub-coredevice";
pub const OWNER_THREAD_STACK_BYTES: usize = 16 * 1024 * 1024;

/// Non-`Send` session-manager future created after entering the owner thread.
pub(crate) type CoreRuntimeFuture = Pin<Box<dyn Future<Output = ()> + 'static>>;

/// Internal state graph owned by one runtime and observed through clients.
///
/// The host path remains opaque to the runtime. All slots are created together
/// so desktop, HTTP, MCP, and future headless adapters cannot accidentally
/// observe a second, divergent device state graph.
pub(crate) struct CoreRuntimeState<HostPath> {
    pub(crate) status: StatusSlot,
    pub(crate) orientation: OrientationSlot,
    pub(crate) devices: DeviceListSlot,
    pub(crate) active: ActiveSlot,
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

impl<HostPath> Default for CoreRuntimeState<HostPath> {
    fn default() -> Self {
        Self {
            status: StatusSlot::default(),
            orientation: OrientationSlot::default(),
            devices: DeviceListSlot::default(),
            active: ActiveSlot::default(),
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
            connected: ConnectedSessionViews {
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
            },
            devices: self.devices.clone(),
            active: self.active.clone(),
            commands: self.commands.clone(),
        }
    }
}

/// Owns the session manager's control channel and dedicated executor thread.
pub struct CoreRuntime {
    control: UnboundedSender<SessionControlCommand>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl CoreRuntime {
    /// Create host-facing state and an owner-thread task around one shared
    /// control channel. `task` is invoked on the new thread so its future may
    /// retain non-`Send` CoreDevice clients safely inside the `LocalSet`.
    pub(crate) fn start<State, Build, Task>(build: Build) -> Result<(Self, State), String>
    where
        Build: FnOnce(
            UnboundedSender<SessionControlCommand>,
            UnboundedReceiver<SessionControlCommand>,
        ) -> (State, Task),
        Task: FnOnce() -> CoreRuntimeFuture + Send + 'static,
    {
        let (control, control_rx) = mpsc::unbounded_channel();
        let (state, task) = build(control.clone(), control_rx);
        let thread = std::thread::Builder::new()
            .name(OWNER_THREAD_NAME.into())
            .stack_size(OWNER_THREAD_STACK_BYTES)
            .spawn(move || run_owner(task))
            .map_err(|error| format!("cannot start CoreDevice thread: {error}"))?;
        Ok((
            Self {
                control,
                thread: Mutex::new(Some(thread)),
            },
            state,
        ))
    }

    pub fn request_shutdown(&self) {
        let _ = self.control.send(SessionControlCommand::Quit);
    }

    /// Request shutdown and join the owner exactly once.
    pub fn stop(&self) {
        self.request_shutdown();
        if let Some(thread) = self.thread.lock().unwrap().take() {
            let _ = thread.join();
        }
    }

    #[cfg(test)]
    fn is_stopped(&self) -> bool {
        self.thread.lock().unwrap().is_none()
    }
}

impl Drop for CoreRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_owner<Task>(task: Task)
where
    Task: FnOnce() -> CoreRuntimeFuture,
{
    tracing::info!(
        stack_bytes = OWNER_THREAD_STACK_BYTES,
        "CoreDevice owner thread started"
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build CoreDevice runtime");
    let local = tokio::task::LocalSet::new();
    runtime.block_on(local.run_until(task()));
    tracing::info!("CoreDevice owner thread stopped");
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{CoreRuntime, CoreRuntimeFuture, CoreRuntimeState};
    use crate::SessionControlCommand;

    #[test]
    fn shutdown_is_idempotent_and_joins_the_owner() {
        let stopped = Arc::new(AtomicBool::new(false));
        let owner_stopped = stopped.clone();
        let (runtime, ()) = CoreRuntime::start(|_control, mut receiver| {
            let task = move || -> CoreRuntimeFuture {
                Box::pin(async move {
                    while let Some(command) = receiver.recv().await {
                        if matches!(command, SessionControlCommand::Quit) {
                            break;
                        }
                    }
                    owner_stopped.store(true, Ordering::Release);
                })
            };
            ((), task)
        })
        .expect("start runtime");

        runtime.stop();
        runtime.stop();

        assert!(stopped.load(Ordering::Acquire));
        assert!(runtime.is_stopped());
    }

    #[test]
    fn manager_views_share_the_single_runtime_state_graph() {
        let state = CoreRuntimeState::<String>::default();
        let views = state.manager_views();

        state.status.set("connected");
        state
            .active
            .set_selected("device".into(), "device::usb".into());

        assert_eq!(views.connected.status.get(), "connected");
        assert_eq!(views.active.selection_id().as_deref(), Some("device::usb"));
        assert!(
            !state
                .commands
                .try_send(crate::DeviceSessionCommand::Shutdown)
        );
    }
}
