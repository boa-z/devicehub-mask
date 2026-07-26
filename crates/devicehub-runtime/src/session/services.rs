//! Command ports exposed by services owned by one connected device session.

use std::future::Future;
use std::sync::Arc;

use devicehub_core::{ConnKind, LocationStatus, LocationStatusSlot};
use idevice::provider::IdeviceProvider;
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use tokio::sync::{mpsc, watch};

use crate::{
    AppConsoleCommand, AppDocumentCommand, AppIconCommand, AppLifecycleCommand,
    BluetoothCaptureCommand, CompanionDeviceCommand, CrashReportExportCommand,
    DeveloperImageMountCommand, DeviceBackupCommand, DeviceConditionCommand, DeviceFileCommand,
    HomeScreenCommand, LocationCommand, LogArchiveCommand, NetworkCaptureCommand,
    ProvisioningCommand, RunningProcessCommand, ScreenCaptureCommand, SysdiagnoseCommand,
    WdaAutomationCommand, WdaRunnerCommand,
};

use crate::{
    DeviceConditionSlot, DeviceEventSlot, DeviceLogDemand, DeviceLogSlot, PerformanceDemand,
    PerformanceSlot, ServiceRegistry, ServiceReporter, ServiceSupervisor,
};

/// Location endpoint retained by session input dispatch and status reporting.
pub struct LocationServicePort {
    pub sender: mpsc::Sender<LocationCommand>,
    pub status: LocationStatusSlot,
}

/// Device-management command endpoints with host paths kept generic.
///
/// A host creates exactly one value per connected session and transfers it to
/// the sole command dispatcher. The runtime model therefore does not implement
/// `Clone`, preventing accidental duplicate dispatch ownership.
pub struct DeviceServicePorts<HostPath> {
    pub location: LocationServicePort,
    pub icons: mpsc::Sender<AppIconCommand>,
    pub companions: mpsc::Sender<CompanionDeviceCommand>,
    pub home_screen: mpsc::Sender<HomeScreenCommand>,
    pub running_processes: mpsc::Sender<RunningProcessCommand>,
    pub app_lifecycle: mpsc::Sender<AppLifecycleCommand>,
    pub wda: mpsc::Sender<WdaAutomationCommand>,
    pub wda_runner: mpsc::Sender<WdaRunnerCommand>,
    pub app_console: mpsc::Sender<AppConsoleCommand>,
    pub documents: mpsc::Sender<AppDocumentCommand<HostPath>>,
    pub device_files: mpsc::Sender<DeviceFileCommand<HostPath>>,
    pub screen_capture: mpsc::Sender<ScreenCaptureCommand>,
    pub network_capture: mpsc::Sender<NetworkCaptureCommand<HostPath>>,
    pub bluetooth_capture: mpsc::Sender<BluetoothCaptureCommand<HostPath>>,
    pub device_backup: mpsc::Sender<DeviceBackupCommand<HostPath>>,
    pub sysdiagnose: mpsc::Sender<SysdiagnoseCommand<HostPath>>,
    pub log_archive: mpsc::Sender<LogArchiveCommand<HostPath>>,
    pub developer_image: mpsc::Sender<DeveloperImageMountCommand<HostPath>>,
    pub device_conditions: mpsc::Sender<DeviceConditionCommand>,
    pub provisioning: mpsc::Sender<ProvisioningCommand<HostPath>>,
    pub crash_report_exports: mpsc::Sender<CrashReportExportCommand<HostPath>>,
}

/// State and demand ports consumed by runtime-owned services.
#[derive(Clone)]
pub struct RuntimeServiceViews {
    pub performance: PerformanceSlot,
    pub performance_demand: PerformanceDemand,
    pub device_logs: DeviceLogSlot,
    pub device_log_demand: DeviceLogDemand,
    pub services: ServiceRegistry,
    pub device_events: DeviceEventSlot,
    pub location: LocationStatusSlot,
    pub device_conditions: DeviceConditionSlot,
}

/// Command ports for services that require no host filesystem implementation.
pub struct RuntimeDeviceServicePorts {
    pub location: LocationServicePort,
    pub icons: mpsc::Sender<AppIconCommand>,
    pub companions: mpsc::Sender<CompanionDeviceCommand>,
    pub home_screen: mpsc::Sender<HomeScreenCommand>,
    pub running_processes: mpsc::Sender<RunningProcessCommand>,
    pub app_lifecycle: mpsc::Sender<AppLifecycleCommand>,
    pub wda: mpsc::Sender<WdaAutomationCommand>,
    pub wda_runner: mpsc::Sender<WdaRunnerCommand>,
    pub app_console: mpsc::Sender<AppConsoleCommand>,
    pub screen_capture: mpsc::Sender<ScreenCaptureCommand>,
    pub device_conditions: mpsc::Sender<DeviceConditionCommand>,
}

/// Owns runtime-native services for exactly one connected device session.
/// Host adapters may register filesystem-backed workers into the same shutdown
/// tree without gaining direct ownership of the supervisor.
pub struct RuntimeSessionServices {
    supervisor: ServiceSupervisor,
    device_ports: Option<RuntimeDeviceServicePorts>,
}

impl RuntimeSessionServices {
    pub fn start(
        provider: Arc<dyn IdeviceProvider>,
        connection: ConnKind,
        adapter: AdapterHandle,
        handshake: RsdHandshake,
        views: RuntimeServiceViews,
    ) -> Self {
        views.performance.reset();
        views.device_logs.reset();
        views.device_events.reset();

        let mut supervisor = ServiceSupervisor::new(views.services);
        supervisor.spawn(crate::supervise_heartbeat(
            provider.clone(),
            supervisor.reporter("device.heartbeat"),
            supervisor.shutdown_receiver(),
        ));
        supervisor.spawn(crate::supervise_device_logs(
            adapter.clone(),
            handshake.clone(),
            views.device_logs,
            supervisor.reporter("device.logs"),
            views.device_log_demand.subscribe(),
            supervisor.shutdown_receiver(),
        ));
        supervisor.spawn(crate::supervise_device_events(
            adapter.clone(),
            handshake.clone(),
            views.device_events,
            supervisor.reporter("device.notifications"),
            supervisor.shutdown_receiver(),
        ));
        supervisor.spawn(crate::supervise_performance_system(
            adapter.clone(),
            handshake.clone(),
            views.performance.clone(),
            supervisor.reporter("performance.system"),
            views.performance_demand.subscribe(),
            supervisor.shutdown_receiver(),
        ));
        supervisor.spawn(crate::supervise_performance_graphics(
            adapter.clone(),
            handshake.clone(),
            views.performance.clone(),
            supervisor.reporter("performance.graphics"),
            views.performance_demand.subscribe(),
            supervisor.shutdown_receiver(),
        ));
        supervisor.spawn(crate::supervise_performance_network(
            adapter.clone(),
            handshake.clone(),
            views.performance.clone(),
            supervisor.reporter("performance.network"),
            views.performance_demand.subscribe(),
            supervisor.shutdown_receiver(),
        ));
        supervisor.spawn(crate::supervise_performance_energy(
            adapter.clone(),
            handshake.clone(),
            views.performance.clone(),
            supervisor.reporter("performance.energy"),
            views.performance_demand.subscribe(),
            supervisor.shutdown_receiver(),
        ));
        supervisor.spawn(crate::supervise_performance_app_activity(
            adapter.clone(),
            handshake.clone(),
            views.performance,
            supervisor.reporter("performance.app_activity"),
            views.performance_demand.subscribe(),
            supervisor.shutdown_receiver(),
        ));

        views.location.set(LocationStatus::default());
        let (location_sender, location_receiver) = mpsc::channel(8);
        supervisor.spawn(crate::supervise_location(
            adapter.clone(),
            handshake.clone(),
            provider.clone(),
            location_receiver,
            views.location.clone(),
            supervisor.reporter("location"),
            supervisor.shutdown_receiver(),
        ));
        let location = LocationServicePort {
            sender: location_sender,
            status: views.location,
        };

        let (icons, icon_commands) = mpsc::channel(16);
        supervisor.spawn(crate::serve_app_icons(
            adapter.clone(),
            handshake.clone(),
            icon_commands,
            supervisor.shutdown_receiver(),
        ));
        let (companions, companion_commands) = mpsc::channel(2);
        supervisor.spawn(crate::serve_companion_devices(
            adapter.clone(),
            handshake.clone(),
            companion_commands,
            supervisor.reporter("device.companions"),
            supervisor.shutdown_receiver(),
        ));
        let (home_screen, home_screen_commands) = mpsc::channel(2);
        supervisor.spawn(crate::serve_home_screen(
            adapter.clone(),
            handshake.clone(),
            home_screen_commands,
            supervisor.reporter("device.home_screen"),
            supervisor.shutdown_receiver(),
        ));
        let (running_processes, running_process_commands) = mpsc::channel(2);
        supervisor.spawn(crate::serve_running_processes(
            adapter.clone(),
            handshake.clone(),
            running_process_commands,
            supervisor.reporter("performance.process_inventory"),
            supervisor.shutdown_receiver(),
        ));
        let (app_lifecycle, app_lifecycle_commands) = mpsc::channel(2);
        supervisor.spawn(crate::serve_app_lifecycle(
            adapter.clone(),
            handshake.clone(),
            app_lifecycle_commands,
            supervisor.reporter("device.app_lifecycle"),
            supervisor.shutdown_receiver(),
        ));
        let (wda, wda_commands) = mpsc::channel(4);
        supervisor.spawn(crate::serve_wda_automation(
            provider.clone(),
            wda_commands,
            supervisor.reporter("device.wda"),
            supervisor.shutdown_receiver(),
        ));
        let (wda_runner, wda_runner_commands) = mpsc::channel(2);
        supervisor.spawn(crate::serve_wda_runner(
            provider.clone(),
            wda_runner_commands,
            supervisor.reporter("device.wda_runner"),
            supervisor.shutdown_receiver(),
        ));
        let (app_console, app_console_commands) = mpsc::channel(4);
        supervisor.spawn(crate::serve_app_console(
            adapter.clone(),
            handshake.clone(),
            app_console_commands,
            supervisor.reporter("device.app_console"),
            supervisor.shutdown_receiver(),
        ));
        let (screen_capture, screen_capture_commands) = mpsc::channel(1);
        supervisor.spawn(crate::serve_screen_capture(
            crate::ScreenCaptureTransport::new(
                provider,
                connection,
                adapter.clone(),
                handshake.clone(),
            ),
            screen_capture_commands,
            supervisor.shutdown_receiver(),
        ));
        let (device_conditions, device_condition_commands) = mpsc::channel(4);
        supervisor.spawn(crate::supervise_device_conditions(
            adapter,
            handshake,
            device_condition_commands,
            views.device_conditions,
            supervisor.reporter("device.conditions"),
            supervisor.shutdown_receiver(),
        ));

        Self {
            supervisor,
            device_ports: Some(RuntimeDeviceServicePorts {
                location,
                icons,
                companions,
                home_screen,
                running_processes,
                app_lifecycle,
                wda,
                wda_runner,
                app_console,
                screen_capture,
                device_conditions,
            }),
        }
    }

    pub fn take_device_ports(&mut self) -> RuntimeDeviceServicePorts {
        self.device_ports
            .take()
            .expect("runtime device service ports already taken")
    }

    pub fn reporter(&self, name: &'static str) -> ServiceReporter {
        self.supervisor.reporter(name)
    }

    pub fn shutdown_receiver(&self) -> watch::Receiver<bool> {
        self.supervisor.shutdown_receiver()
    }

    pub fn spawn_host_task(&mut self, task: impl Future<Output = ()> + 'static) {
        self.supervisor.spawn(task);
    }

    pub async fn shutdown(mut self) {
        drop(self.device_ports.take());
        self.supervisor.shutdown().await;
    }
}
