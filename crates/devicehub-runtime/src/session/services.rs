//! Command ports exposed by services owned by one connected device session.

use std::sync::Arc;

use devicehub_core::{ConnKind, LocationStatus, LocationStatusSlot};
use idevice::provider::IdeviceProvider;
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use tokio::sync::mpsc;

use crate::applications::{
    serve_app_console, serve_app_icons, serve_app_lifecycle, serve_running_processes,
    serve_wda_automation, serve_wda_runner,
};
use crate::capture::{
    BluetoothCaptureTransport, CaptureFileIo, NetworkCaptureTransport, serve_bluetooth_capture,
    serve_network_capture,
};
use crate::device::{
    ScreenCaptureTransport, serve_companion_devices, serve_crash_report_exports,
    serve_developer_image_mount, serve_home_screen, serve_screen_capture,
    supervise_device_conditions, supervise_device_events, supervise_device_logs,
    supervise_location, supervise_provisioning,
};
use crate::diagnostics::{
    DeviceBackupTransport, serve_device_backup, serve_log_archive, serve_sysdiagnose,
};
use crate::performance::{
    supervise_performance_app_activity, supervise_performance_energy,
    supervise_performance_graphics, supervise_performance_network, supervise_performance_system,
};
use crate::storage::{
    AppStorageTransport, DeviceFileTransport, HostFileIo, serve_app_documents, serve_device_files,
};
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
    PerformanceSlot, ServiceRegistry, ServiceSupervisor,
};

/// Location endpoint retained by session input dispatch and status reporting.
pub(crate) struct LocationServicePort {
    pub(crate) sender: mpsc::Sender<LocationCommand>,
    pub(crate) status: LocationStatusSlot,
}

/// Device-management command endpoints with host paths kept generic.
///
/// A host creates exactly one value per connected session and transfers it to
/// the sole command dispatcher. The runtime model therefore does not implement
/// `Clone`, preventing accidental duplicate dispatch ownership.
pub(crate) struct DeviceServicePorts<HostPath> {
    pub(crate) location: LocationServicePort,
    pub(crate) icons: mpsc::Sender<AppIconCommand>,
    pub(crate) companions: mpsc::Sender<CompanionDeviceCommand>,
    pub(crate) home_screen: mpsc::Sender<HomeScreenCommand>,
    pub(crate) running_processes: mpsc::Sender<RunningProcessCommand>,
    pub(crate) app_lifecycle: mpsc::Sender<AppLifecycleCommand>,
    pub(crate) wda: mpsc::Sender<WdaAutomationCommand>,
    pub(crate) wda_runner: mpsc::Sender<WdaRunnerCommand>,
    pub(crate) app_console: mpsc::Sender<AppConsoleCommand>,
    pub(crate) documents: mpsc::Sender<AppDocumentCommand<HostPath>>,
    pub(crate) device_files: mpsc::Sender<DeviceFileCommand<HostPath>>,
    pub(crate) screen_capture: mpsc::Sender<ScreenCaptureCommand>,
    pub(crate) network_capture: mpsc::Sender<NetworkCaptureCommand<HostPath>>,
    pub(crate) bluetooth_capture: mpsc::Sender<BluetoothCaptureCommand<HostPath>>,
    pub(crate) device_backup: mpsc::Sender<DeviceBackupCommand<HostPath>>,
    pub(crate) sysdiagnose: mpsc::Sender<SysdiagnoseCommand<HostPath>>,
    pub(crate) log_archive: mpsc::Sender<LogArchiveCommand<HostPath>>,
    pub(crate) developer_image: mpsc::Sender<DeveloperImageMountCommand<HostPath>>,
    pub(crate) device_conditions: mpsc::Sender<DeviceConditionCommand>,
    pub(crate) provisioning: mpsc::Sender<ProvisioningCommand<HostPath>>,
    pub(crate) crash_report_exports: mpsc::Sender<CrashReportExportCommand<HostPath>>,
}

/// State and demand ports consumed by runtime-owned services.
#[derive(Clone)]
pub(crate) struct RuntimeServiceViews {
    pub(crate) performance: PerformanceSlot,
    pub(crate) performance_demand: PerformanceDemand,
    pub(crate) device_logs: DeviceLogSlot,
    pub(crate) device_log_demand: DeviceLogDemand,
    pub(crate) services: ServiceRegistry,
    pub(crate) device_events: DeviceEventSlot,
    pub(crate) location: LocationStatusSlot,
    pub(crate) device_conditions: DeviceConditionSlot,
}

/// Command ports for services that require no host filesystem implementation.
pub(crate) struct RuntimeDeviceServicePorts {
    pub(crate) location: LocationServicePort,
    pub(crate) icons: mpsc::Sender<AppIconCommand>,
    pub(crate) companions: mpsc::Sender<CompanionDeviceCommand>,
    pub(crate) home_screen: mpsc::Sender<HomeScreenCommand>,
    pub(crate) running_processes: mpsc::Sender<RunningProcessCommand>,
    pub(crate) app_lifecycle: mpsc::Sender<AppLifecycleCommand>,
    pub(crate) wda: mpsc::Sender<WdaAutomationCommand>,
    pub(crate) wda_runner: mpsc::Sender<WdaRunnerCommand>,
    pub(crate) app_console: mpsc::Sender<AppConsoleCommand>,
    pub(crate) screen_capture: mpsc::Sender<ScreenCaptureCommand>,
    pub(crate) device_conditions: mpsc::Sender<DeviceConditionCommand>,
}

/// Host-visible state shared with filesystem-backed runtime services.
#[derive(Clone)]
pub(crate) struct RuntimeHostServiceViews {
    pub(crate) app_documents: crate::AppDocumentActivitySlot,
    pub(crate) device_files: crate::DeviceFileActivitySlot,
    pub(crate) network_capture: crate::NetworkCaptureSlot,
    pub(crate) bluetooth_capture: crate::BluetoothCaptureSlot,
    pub(crate) device_backup: crate::DeviceBackupSlot,
    pub(crate) sysdiagnose: crate::SysdiagnoseSlot,
    pub(crate) log_archive: crate::LogArchiveSlot,
    pub(crate) developer_image: crate::DeveloperImageMountSlot,
}

/// Host capabilities injected once while the runtime owns service lifecycle.
#[derive(Clone)]
pub struct RuntimeSessionHostAdapters<Files, CaptureFiles, Backup, DeveloperImages, Profiles> {
    pub files: Files,
    pub capture_files: CaptureFiles,
    pub backup: Backup,
    pub developer_images: DeveloperImages,
    pub provisioning_profiles: Profiles,
}

/// Owns the complete service tree and its sole command-port bundle for one
/// connected device session.
pub(crate) struct RuntimeConnectedSessionServices<HostPath> {
    runtime: RuntimeSessionServices,
    management: Option<DeviceServicePorts<HostPath>>,
}

impl<HostPath> RuntimeConnectedSessionServices<HostPath> {
    pub(crate) fn take_management(&mut self) -> DeviceServicePorts<HostPath> {
        self.management
            .take()
            .expect("device management services already taken")
    }

    /// Close command senders before cancelling any active service operation.
    pub(crate) async fn shutdown(self) {
        let Self {
            runtime,
            management,
        } = self;
        drop(management);
        runtime.shutdown().await;
    }
}

/// Owns every service for exactly one connected device session. Hosts inject
/// capabilities through typed adapters without gaining supervisor access.
pub(crate) struct RuntimeSessionServices {
    supervisor: ServiceSupervisor,
    device_ports: Option<RuntimeDeviceServicePorts>,
}

impl RuntimeSessionServices {
    pub(crate) fn start(
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
        supervisor.spawn(super::supervise_heartbeat(
            provider.clone(),
            supervisor.reporter("device.heartbeat"),
            supervisor.shutdown_receiver(),
        ));
        supervisor.spawn(supervise_device_logs(
            adapter.clone(),
            handshake.clone(),
            views.device_logs,
            supervisor.reporter("device.logs"),
            views.device_log_demand.subscribe(),
            supervisor.shutdown_receiver(),
        ));
        supervisor.spawn(supervise_device_events(
            adapter.clone(),
            handshake.clone(),
            views.device_events,
            supervisor.reporter("device.notifications"),
            supervisor.shutdown_receiver(),
        ));
        supervisor.spawn(supervise_performance_system(
            adapter.clone(),
            handshake.clone(),
            views.performance.clone(),
            supervisor.reporter("performance.system"),
            views.performance_demand.subscribe(),
            supervisor.shutdown_receiver(),
        ));
        supervisor.spawn(supervise_performance_graphics(
            adapter.clone(),
            handshake.clone(),
            views.performance.clone(),
            supervisor.reporter("performance.graphics"),
            views.performance_demand.subscribe(),
            supervisor.shutdown_receiver(),
        ));
        supervisor.spawn(supervise_performance_network(
            adapter.clone(),
            handshake.clone(),
            views.performance.clone(),
            supervisor.reporter("performance.network"),
            views.performance_demand.subscribe(),
            supervisor.shutdown_receiver(),
        ));
        supervisor.spawn(supervise_performance_energy(
            adapter.clone(),
            handshake.clone(),
            views.performance.clone(),
            supervisor.reporter("performance.energy"),
            views.performance_demand.subscribe(),
            supervisor.shutdown_receiver(),
        ));
        supervisor.spawn(supervise_performance_app_activity(
            adapter.clone(),
            handshake.clone(),
            views.performance,
            supervisor.reporter("performance.app_activity"),
            views.performance_demand.subscribe(),
            supervisor.shutdown_receiver(),
        ));

        views.location.set(LocationStatus::default());
        let (location_sender, location_receiver) = mpsc::channel(8);
        supervisor.spawn(supervise_location(
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
        supervisor.spawn(serve_app_icons(
            adapter.clone(),
            handshake.clone(),
            icon_commands,
            supervisor.shutdown_receiver(),
        ));
        let (companions, companion_commands) = mpsc::channel(2);
        supervisor.spawn(serve_companion_devices(
            adapter.clone(),
            handshake.clone(),
            companion_commands,
            supervisor.reporter("device.companions"),
            supervisor.shutdown_receiver(),
        ));
        let (home_screen, home_screen_commands) = mpsc::channel(2);
        supervisor.spawn(serve_home_screen(
            adapter.clone(),
            handshake.clone(),
            home_screen_commands,
            supervisor.reporter("device.home_screen"),
            supervisor.shutdown_receiver(),
        ));
        let (running_processes, running_process_commands) = mpsc::channel(2);
        supervisor.spawn(serve_running_processes(
            adapter.clone(),
            handshake.clone(),
            running_process_commands,
            supervisor.reporter("performance.process_inventory"),
            supervisor.shutdown_receiver(),
        ));
        let (app_lifecycle, app_lifecycle_commands) = mpsc::channel(2);
        supervisor.spawn(serve_app_lifecycle(
            adapter.clone(),
            handshake.clone(),
            app_lifecycle_commands,
            supervisor.reporter("device.app_lifecycle"),
            supervisor.shutdown_receiver(),
        ));
        let (wda, wda_commands) = mpsc::channel(4);
        supervisor.spawn(serve_wda_automation(
            provider.clone(),
            wda_commands,
            supervisor.reporter("device.wda"),
            supervisor.shutdown_receiver(),
        ));
        let (wda_runner, wda_runner_commands) = mpsc::channel(2);
        supervisor.spawn(serve_wda_runner(
            provider.clone(),
            wda_runner_commands,
            supervisor.reporter("device.wda_runner"),
            supervisor.shutdown_receiver(),
        ));
        let (app_console, app_console_commands) = mpsc::channel(4);
        supervisor.spawn(serve_app_console(
            adapter.clone(),
            handshake.clone(),
            app_console_commands,
            supervisor.reporter("device.app_console"),
            supervisor.shutdown_receiver(),
        ));
        let (screen_capture, screen_capture_commands) = mpsc::channel(1);
        supervisor.spawn(serve_screen_capture(
            ScreenCaptureTransport::new(provider, connection, adapter.clone(), handshake.clone()),
            screen_capture_commands,
            supervisor.shutdown_receiver(),
        ));
        let (device_conditions, device_condition_commands) = mpsc::channel(4);
        supervisor.spawn(supervise_device_conditions(
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

    fn take_device_ports(&mut self) -> RuntimeDeviceServicePorts {
        self.device_ports
            .take()
            .expect("runtime device service ports already taken")
    }

    /// Attach all host-backed services and return the session's sole complete
    /// command-port bundle.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn attach_host_services<Files, CaptureFiles, Backup, DeveloperImages, Profiles>(
        mut self,
        provider: Arc<dyn IdeviceProvider>,
        connection: ConnKind,
        adapter: AdapterHandle,
        handshake: RsdHandshake,
        source_identifier: String,
        views: RuntimeHostServiceViews,
        adapters: RuntimeSessionHostAdapters<
            Files,
            CaptureFiles,
            Backup,
            DeveloperImages,
            Profiles,
        >,
    ) -> RuntimeConnectedSessionServices<Files::Path>
    where
        Files: HostFileIo,
        CaptureFiles: CaptureFileIo<Destination = Files::Path>,
        Backup: crate::DeviceBackupExecutor<Destination = Files::Path>,
        DeveloperImages: crate::DeveloperImageAssetLoader<Source = Files::Path>,
        Profiles: crate::ProvisioningProfileLoader<Source = Files::Path>,
    {
        let runtime = self.take_device_ports();
        let documents = self.start_app_documents(
            provider.clone(),
            connection,
            adapter.clone(),
            handshake.clone(),
            views.app_documents,
            adapters.files.clone(),
        );
        let device_files = self.start_device_files(
            provider.clone(),
            connection,
            adapter.clone(),
            handshake.clone(),
            views.device_files,
            adapters.files.clone(),
        );
        let network_capture = self.start_network_capture(
            provider.clone(),
            connection,
            adapter.clone(),
            handshake.clone(),
            views.network_capture,
            adapters.capture_files.clone(),
        );
        let bluetooth_capture = self.start_bluetooth_capture(
            adapter.clone(),
            handshake.clone(),
            views.bluetooth_capture,
            adapters.capture_files,
        );
        let device_backup = self.start_device_backup(
            provider.clone(),
            connection,
            adapter.clone(),
            handshake.clone(),
            source_identifier,
            views.device_backup,
            adapters.backup,
        );
        let sysdiagnose = self.start_sysdiagnose(
            adapter.clone(),
            handshake.clone(),
            views.sysdiagnose,
            adapters.files.clone(),
        );
        let log_archive = self.start_log_archive(
            adapter.clone(),
            handshake.clone(),
            views.log_archive,
            adapters.files.clone(),
        );
        let developer_image = self.start_developer_image(
            provider.clone(),
            views.developer_image,
            adapters.developer_images,
        );
        let provisioning = self.start_provisioning(
            adapter,
            handshake,
            provider.clone(),
            adapters.provisioning_profiles,
        );
        let crash_report_exports = self.start_crash_report_exports(provider, adapters.files);

        let management = DeviceServicePorts {
            location: runtime.location,
            icons: runtime.icons,
            companions: runtime.companions,
            home_screen: runtime.home_screen,
            running_processes: runtime.running_processes,
            app_lifecycle: runtime.app_lifecycle,
            wda: runtime.wda,
            wda_runner: runtime.wda_runner,
            app_console: runtime.app_console,
            documents,
            device_files,
            screen_capture: runtime.screen_capture,
            network_capture,
            bluetooth_capture,
            device_backup,
            sysdiagnose,
            log_archive,
            developer_image,
            device_conditions: runtime.device_conditions,
            provisioning,
            crash_report_exports,
        };
        RuntimeConnectedSessionServices {
            runtime: self,
            management: Some(management),
        }
    }

    /// Start sandboxed application storage with host-owned file persistence.
    fn start_app_documents<Files>(
        &mut self,
        provider: Arc<dyn IdeviceProvider>,
        connection: ConnKind,
        adapter: AdapterHandle,
        handshake: RsdHandshake,
        activity: crate::AppDocumentActivitySlot,
        files: Files,
    ) -> mpsc::Sender<AppDocumentCommand<Files::Path>>
    where
        Files: HostFileIo,
    {
        let (sender, commands) = mpsc::channel(8);
        self.supervisor.spawn(serve_app_documents(
            AppStorageTransport::new(provider, connection, adapter, handshake),
            commands,
            activity,
            files,
            self.supervisor.shutdown_receiver(),
        ));
        sender
    }

    /// Start public AFC storage with host-owned file persistence.
    fn start_device_files<Files>(
        &mut self,
        provider: Arc<dyn IdeviceProvider>,
        connection: ConnKind,
        adapter: AdapterHandle,
        handshake: RsdHandshake,
        activity: crate::DeviceFileActivitySlot,
        files: Files,
    ) -> mpsc::Sender<DeviceFileCommand<Files::Path>>
    where
        Files: HostFileIo,
    {
        let (sender, commands) = mpsc::channel(8);
        self.supervisor.spawn(serve_device_files(
            DeviceFileTransport::new(provider, connection, adapter, handshake),
            commands,
            activity,
            files,
            self.supervisor.reporter("device.files"),
            self.supervisor.shutdown_receiver(),
        ));
        sender
    }

    /// Start network packet capture with host-owned atomic file publication.
    fn start_network_capture<Files>(
        &mut self,
        provider: Arc<dyn IdeviceProvider>,
        connection: ConnKind,
        adapter: AdapterHandle,
        handshake: RsdHandshake,
        status: crate::NetworkCaptureSlot,
        files: Files,
    ) -> mpsc::Sender<NetworkCaptureCommand<Files::Destination>>
    where
        Files: CaptureFileIo,
    {
        let (sender, commands) = mpsc::channel(4);
        self.supervisor.spawn(serve_network_capture(
            NetworkCaptureTransport::new(provider, connection, adapter, handshake),
            commands,
            status,
            files,
            self.supervisor.reporter("network.capture"),
            self.supervisor.shutdown_receiver(),
        ));
        sender
    }

    /// Start Bluetooth PacketLogger capture with host-owned atomic file publication.
    fn start_bluetooth_capture<Files>(
        &mut self,
        adapter: AdapterHandle,
        handshake: RsdHandshake,
        status: crate::BluetoothCaptureSlot,
        files: Files,
    ) -> mpsc::Sender<BluetoothCaptureCommand<Files::Destination>>
    where
        Files: CaptureFileIo,
    {
        let (sender, commands) = mpsc::channel(4);
        self.supervisor.spawn(serve_bluetooth_capture(
            BluetoothCaptureTransport::new(adapter, handshake),
            commands,
            status,
            files,
            self.supervisor.reporter("bluetooth.capture"),
            self.supervisor.shutdown_receiver(),
        ));
        sender
    }

    /// Start MobileBackup2 orchestration with a host-confined backup executor.
    #[allow(clippy::too_many_arguments)]
    fn start_device_backup<Executor>(
        &mut self,
        provider: Arc<dyn IdeviceProvider>,
        connection: ConnKind,
        adapter: AdapterHandle,
        handshake: RsdHandshake,
        source_identifier: String,
        status: crate::DeviceBackupSlot,
        executor: Executor,
    ) -> mpsc::Sender<DeviceBackupCommand<Executor::Destination>>
    where
        Executor: crate::DeviceBackupExecutor,
    {
        let (sender, commands) = mpsc::channel(4);
        self.supervisor.spawn(serve_device_backup(
            DeviceBackupTransport::new(provider, connection, adapter, handshake, source_identifier),
            commands,
            status,
            executor,
            self.supervisor.reporter("device.backup"),
            self.supervisor.shutdown_receiver(),
        ));
        sender
    }

    /// Start cancellable sysdiagnose export with host-owned persistence.
    fn start_sysdiagnose<Files>(
        &mut self,
        adapter: AdapterHandle,
        handshake: RsdHandshake,
        status: crate::SysdiagnoseSlot,
        files: Files,
    ) -> mpsc::Sender<SysdiagnoseCommand<Files::Path>>
    where
        Files: HostFileIo,
    {
        let (sender, commands) = mpsc::channel(4);
        self.supervisor.spawn(serve_sysdiagnose(
            adapter,
            handshake,
            commands,
            status,
            files,
            self.supervisor.reporter("device.sysdiagnose"),
            self.supervisor.shutdown_receiver(),
        ));
        sender
    }

    /// Start cancellable unified-log export with host-owned persistence.
    fn start_log_archive<Files>(
        &mut self,
        adapter: AdapterHandle,
        handshake: RsdHandshake,
        status: crate::LogArchiveSlot,
        files: Files,
    ) -> mpsc::Sender<LogArchiveCommand<Files::Path>>
    where
        Files: HostFileIo,
    {
        let (sender, commands) = mpsc::channel(4);
        self.supervisor.spawn(serve_log_archive(
            adapter,
            handshake,
            commands,
            status,
            files,
            self.supervisor.reporter("device.log_archive"),
            self.supervisor.shutdown_receiver(),
        ));
        sender
    }

    /// Start Developer Disk Image operations with host-owned asset loading.
    fn start_developer_image<Assets>(
        &mut self,
        provider: Arc<dyn IdeviceProvider>,
        status: crate::DeveloperImageMountSlot,
        assets: Assets,
    ) -> mpsc::Sender<DeveloperImageMountCommand<Assets::Source>>
    where
        Assets: crate::DeveloperImageAssetLoader,
    {
        let (sender, commands) = mpsc::channel(4);
        self.supervisor.spawn(serve_developer_image_mount(
            provider,
            commands,
            status,
            assets,
            self.supervisor.reporter("device.developer_image"),
            self.supervisor.shutdown_receiver(),
        ));
        sender
    }

    /// Start provisioning profile management with a host-owned source loader.
    fn start_provisioning<Loader>(
        &mut self,
        adapter: AdapterHandle,
        handshake: RsdHandshake,
        provider: Arc<dyn IdeviceProvider>,
        loader: Loader,
    ) -> mpsc::Sender<ProvisioningCommand<Loader::Source>>
    where
        Loader: crate::ProvisioningProfileLoader,
    {
        let (sender, commands) = mpsc::channel(4);
        self.supervisor.spawn(supervise_provisioning(
            adapter,
            handshake,
            provider,
            commands,
            loader,
            self.supervisor.reporter("device.provisioning"),
            self.supervisor.shutdown_receiver(),
        ));
        sender
    }

    /// Add the host-persisted crash report exporter to this session's owned
    /// service tree and return its sole bounded command sender.
    fn start_crash_report_exports<Files>(
        &mut self,
        provider: Arc<dyn IdeviceProvider>,
        files: Files,
    ) -> mpsc::Sender<CrashReportExportCommand<Files::Path>>
    where
        Files: HostFileIo,
    {
        let (sender, commands) = mpsc::channel(2);
        self.supervisor.spawn(serve_crash_report_exports(
            provider,
            commands,
            files,
            self.supervisor.shutdown_receiver(),
        ));
        sender
    }

    async fn shutdown(mut self) {
        drop(self.device_ports.take());
        self.supervisor.shutdown().await;
    }
}
