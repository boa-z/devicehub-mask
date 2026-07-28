//! Routing from host-facing session commands to supervised device services.

use std::sync::Arc;
use std::time::Duration;

use devicehub_core::{AppOperationSlot, DeviceDetails};
use idevice::provider::IdeviceProvider;
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use tokio::sync::mpsc;

use super::commands::DeviceSessionCommand;
use super::services::DeviceServicePorts;
use crate::applications::{AppClientSet, AppManagement, AppServiceTransport};
use crate::device::{
    DevicePowerAction, DevicePowerController, delete_crash_report, execute_developer_mode,
    is_developer_image_mounted, list_crash_reports, read_activation_state, read_crash_report,
    read_device_battery, read_device_details, read_device_developer_mode_status, rename_device,
};
use crate::{
    AppIconCommand, BluetoothCaptureCommand, CompanionDeviceCommand, CrashReportExportCommand,
    DeveloperImageMountCommand, DeviceBackupCommand, DeviceConditionCommand, DeviceFileCommand,
    HomeScreenCommand, LocationCommand, LogArchiveCommand, NetworkCaptureCommand,
    ScreenCaptureCommand, SysdiagnoseCommand,
};

const BOOTSTRAP_METADATA_TIMEOUT: Duration = Duration::from_secs(5);
const BOOTSTRAP_INSTALLATION_PROXY_TIMEOUT: Duration = Duration::from_secs(5);

/// Routes management commands while returning HID and clipboard commands that
/// require capabilities owned by the host session loop. Filesystem operations
/// cross bounded host-service ports rather than executing in this router.
pub(crate) struct DeviceSessionRouter<HostPath> {
    provider: Arc<dyn IdeviceProvider>,
    power: DevicePowerController,
    details: Option<DeviceDetails>,
    apps: AppManagement,
    services: DeviceServicePorts<HostPath>,
}

/// First phase of device management startup. Lockdown identity and the legacy
/// Installation Proxy fallback are available before the CoreDevice tunnel is
/// established, preserving connection order while keeping client types private.
pub(crate) struct DeviceManagementBootstrap {
    provider: Arc<dyn IdeviceProvider>,
    app_operation: AppOperationSlot,
    details: Option<DeviceDetails>,
    app_clients: AppClientSet,
}

impl DeviceManagementBootstrap {
    pub(crate) async fn prepare(
        provider: Arc<dyn IdeviceProvider>,
        requested_udid: String,
        app_operation: AppOperationSlot,
    ) -> Self {
        let details = match tokio::time::timeout(
            BOOTSTRAP_METADATA_TIMEOUT,
            read_device_details(provider.as_ref(), requested_udid),
        )
        .await
        {
            Ok(details) => details,
            Err(_) => {
                tracing::warn!(
                    timeout_ms = BOOTSTRAP_METADATA_TIMEOUT.as_millis() as u64,
                    "initial device metadata timed out; continuing session startup"
                );
                None
            }
        };
        if let Some(details) = &details {
            tracing::info!(
                product_type = %details.product_type,
                product_version = %details.product_version,
                "connected device identity"
            );
        }
        let app_clients = match tokio::time::timeout(
            BOOTSTRAP_INSTALLATION_PROXY_TIMEOUT,
            AppClientSet::connect_installation_proxy(provider.as_ref()),
        )
        .await
        {
            Ok(clients) => clients,
            Err(_) => {
                tracing::warn!(
                    timeout_ms = BOOTSTRAP_INSTALLATION_PROXY_TIMEOUT.as_millis() as u64,
                    "installation proxy bootstrap timed out; continuing without fallback"
                );
                AppClientSet::unavailable()
            }
        };
        Self {
            provider,
            app_operation,
            details,
            app_clients,
        }
    }

    pub(crate) fn bind_transport(
        self,
        adapter: AdapterHandle,
        handshake: RsdHandshake,
    ) -> DeviceManagementSession {
        DeviceManagementSession {
            provider: self.provider,
            app_operation: self.app_operation,
            details: self.details,
            app_clients: self.app_clients,
            app_transport: AppServiceTransport::new(adapter, handshake),
        }
    }
}

/// Opaque App management capability bound to one CoreDevice session.
pub(crate) struct DeviceManagementSession {
    provider: Arc<dyn IdeviceProvider>,
    app_operation: AppOperationSlot,
    details: Option<DeviceDetails>,
    app_clients: AppClientSet,
    app_transport: AppServiceTransport,
}

impl DeviceManagementSession {
    pub(crate) fn details(&self) -> Option<&DeviceDetails> {
        self.details.as_ref()
    }

    pub(crate) async fn connect_app_service(
        &mut self,
        adapter: &mut AdapterHandle,
        handshake: &mut RsdHandshake,
    ) {
        self.app_clients
            .connect_app_service(adapter, handshake)
            .await;
    }

    pub(crate) fn into_router<HostPath>(
        self,
        services: DeviceServicePorts<HostPath>,
    ) -> DeviceSessionRouter<HostPath> {
        DeviceSessionRouter::new(
            self.provider,
            self.app_operation,
            self.details,
            self.app_clients,
            self.app_transport,
            services,
        )
    }
}

impl<HostPath> DeviceSessionRouter<HostPath> {
    fn new(
        provider: Arc<dyn IdeviceProvider>,
        app_operation: AppOperationSlot,
        details: Option<DeviceDetails>,
        app_clients: AppClientSet,
        app_service_transport: AppServiceTransport,
        services: DeviceServicePorts<HostPath>,
    ) -> Self {
        let apps = AppManagement::new(
            provider.clone(),
            app_operation,
            app_clients,
            app_service_transport,
        );
        let power = DevicePowerController::new(provider.clone());
        Self {
            provider,
            power,
            details,
            apps,
            services,
        }
    }

    pub(crate) async fn handle(
        &mut self,
        command: DeviceSessionCommand<HostPath>,
    ) -> Option<DeviceSessionCommand<HostPath>>
    where
        HostPath: Send + 'static,
    {
        let command = match command {
            DeviceSessionCommand::Apps(command) => {
                self.apps.handle(command).await;
                return None;
            }
            command => command,
        };
        match command {
            DeviceSessionCommand::GetDeviceDetails(reply) => {
                self.refresh_device_details(reply);
                None
            }
            DeviceSessionCommand::RenameDevice { name, reply } => {
                let provider = self.provider.clone();
                tokio::spawn(async move {
                    let result = tokio::time::timeout(
                        Duration::from_secs(6),
                        rename_device(provider.as_ref(), &name),
                    )
                    .await
                    .map_err(|_| "device rename timed out".to_string())
                    .and_then(|result| result);
                    let _ = reply.send(result);
                });
                None
            }
            DeviceSessionCommand::SetLocation {
                latitude,
                longitude,
                reply,
            } => {
                route_location(
                    &self.services.location,
                    LocationCommand::Set {
                        latitude,
                        longitude,
                        reply,
                    },
                );
                None
            }
            DeviceSessionCommand::ClearLocation { reply } => {
                route_location(&self.services.location, LocationCommand::Clear { reply });
                None
            }
            DeviceSessionCommand::DeveloperMode(command) => {
                execute_developer_mode(self.provider.clone(), command);
                None
            }
            DeviceSessionCommand::ListCompanionDevices(reply) => {
                route(
                    &self.services.companions,
                    CompanionDeviceCommand::List { reply },
                    "companion device service is busy",
                    "companion device service is unavailable",
                    |command, reason| match command {
                        CompanionDeviceCommand::List { reply } => {
                            let _ = reply.send(Err(reason.into()));
                        }
                    },
                );
                None
            }
            DeviceSessionCommand::GetHomeScreenLayout(reply) => {
                route_rejectable(
                    &self.services.home_screen,
                    HomeScreenCommand::Get { reply },
                    "home screen service is busy",
                    "home screen service is unavailable",
                );
                None
            }
            DeviceSessionCommand::GetWallpaper { kind, reply } => {
                route_rejectable(
                    &self.services.home_screen,
                    HomeScreenCommand::Wallpaper { kind, reply },
                    "home screen service is busy",
                    "home screen service is unavailable",
                );
                None
            }
            DeviceSessionCommand::RunningProcess(command) => {
                route_rejectable(
                    &self.services.running_processes,
                    command,
                    "running process service is busy",
                    "running process service is unavailable",
                );
                None
            }
            DeviceSessionCommand::AppLifecycle(command) => {
                route_rejectable(
                    &self.services.app_lifecycle,
                    command,
                    "application lifecycle service is busy",
                    "application lifecycle service is unavailable",
                );
                None
            }
            DeviceSessionCommand::WdaAutomation(command) => {
                route_rejectable(
                    &self.services.wda,
                    command,
                    "WDA automation service is busy",
                    "WDA automation service is unavailable",
                );
                None
            }
            DeviceSessionCommand::WdaRunner(command) => {
                route_rejectable(
                    &self.services.wda_runner,
                    command,
                    "WDA runner service is busy",
                    "WDA runner service is unavailable",
                );
                None
            }
            DeviceSessionCommand::AppConsole(command) => {
                route_rejectable(
                    &self.services.app_console,
                    command,
                    "application console service is busy",
                    "application console service is unavailable",
                );
                None
            }
            DeviceSessionCommand::GetAppIcon { bundle_id, reply } => {
                route(
                    &self.services.icons,
                    AppIconCommand { bundle_id, reply },
                    "app icon service is busy",
                    "app icon service is unavailable",
                    |command, reason| {
                        let _ = command.reply.send(Err(reason.into()));
                    },
                );
                None
            }
            DeviceSessionCommand::TakeScreenshot(reply) => {
                route(
                    &self.services.screen_capture,
                    ScreenCaptureCommand { reply },
                    "screen capture service is busy",
                    "screen capture service is unavailable",
                    |command, reason| {
                        let _ = command.reply.send(Err(reason.into()));
                    },
                );
                None
            }
            DeviceSessionCommand::NetworkCapture(command) => {
                route(
                    &self.services.network_capture,
                    command,
                    "packet capture service is busy",
                    "packet capture service is unavailable",
                    reject_network_capture,
                );
                None
            }
            DeviceSessionCommand::BluetoothCapture(command) => {
                route(
                    &self.services.bluetooth_capture,
                    command,
                    "Bluetooth capture service is busy",
                    "Bluetooth capture service is unavailable",
                    reject_bluetooth_capture,
                );
                None
            }
            DeviceSessionCommand::DeviceBackup(command) => {
                route(
                    &self.services.device_backup,
                    command,
                    "device backup service is busy",
                    "device backup service is unavailable",
                    reject_device_backup,
                );
                None
            }
            DeviceSessionCommand::Sysdiagnose(command) => {
                route(
                    &self.services.sysdiagnose,
                    command,
                    "sysdiagnose service is busy",
                    "sysdiagnose service is unavailable",
                    reject_sysdiagnose,
                );
                None
            }
            DeviceSessionCommand::LogArchive(command) => {
                route(
                    &self.services.log_archive,
                    command,
                    "log archive service is busy",
                    "log archive service is unavailable",
                    reject_log_archive,
                );
                None
            }
            DeviceSessionCommand::DeveloperImageMount(command) => {
                route(
                    &self.services.developer_image,
                    command,
                    "developer image service is busy",
                    "developer image service is unavailable",
                    reject_developer_image,
                );
                None
            }
            DeviceSessionCommand::DeviceCondition(command) => {
                route(
                    &self.services.device_conditions,
                    command,
                    "device condition service is busy",
                    "device condition service is unavailable",
                    reject_device_condition,
                );
                None
            }
            DeviceSessionCommand::AppDocuments(command) => {
                route(
                    &self.services.documents,
                    command,
                    "application document service is busy",
                    "application document service is unavailable",
                    reject_app_document,
                );
                None
            }
            DeviceSessionCommand::DeviceFiles(command) => {
                route(
                    &self.services.device_files,
                    command,
                    "device file service is busy",
                    "device file service is unavailable",
                    reject_device_file,
                );
                None
            }
            DeviceSessionCommand::LockDevice(reply) => {
                self.power.start(DevicePowerAction::Lock, reply);
                None
            }
            DeviceSessionCommand::RestartDevice(reply) => {
                self.power.start(DevicePowerAction::Restart, reply);
                None
            }
            DeviceSessionCommand::ShutdownDevice(reply) => {
                self.power.start(DevicePowerAction::Shutdown, reply);
                None
            }
            DeviceSessionCommand::Provisioning(command) => {
                route_rejectable(
                    &self.services.provisioning,
                    command,
                    "provisioning profile service is busy",
                    "provisioning profile service is unavailable",
                );
                None
            }
            DeviceSessionCommand::ListCrashReports(reply) => {
                let provider = self.provider.clone();
                tokio::spawn(async move {
                    let _ = reply.send(list_crash_reports(provider).await);
                });
                None
            }
            DeviceSessionCommand::ReadCrashReport {
                device_path,
                max_bytes,
                reply,
            } => {
                let provider = self.provider.clone();
                tokio::spawn(async move {
                    let result = read_crash_report(provider, device_path, max_bytes).await;
                    let _ = reply.send(result);
                });
                None
            }
            DeviceSessionCommand::ExportCrashReport {
                device_path,
                destination,
                reply,
            } => {
                route_rejectable(
                    &self.services.crash_report_exports,
                    CrashReportExportCommand::Export {
                        device_path,
                        destination,
                        reply,
                    },
                    "crash report export service is busy",
                    "crash report export service is unavailable",
                );
                None
            }
            DeviceSessionCommand::DeleteCrashReport { device_path, reply } => {
                let provider = self.provider.clone();
                tokio::spawn(async move {
                    let result = delete_crash_report(provider, device_path).await;
                    let _ = reply.send(result);
                });
                None
            }
            other => Some(other),
        }
    }

    fn refresh_device_details(
        &self,
        reply: tokio::sync::oneshot::Sender<Result<DeviceDetails, String>>,
    ) {
        let Some(mut details) = self.details.clone() else {
            let _ = reply.send(Err("device metadata is unavailable".into()));
            return;
        };
        let provider = self.provider.clone();
        tokio::spawn(async move {
            let requested_udid = details.udid.clone();
            let (
                details_result,
                battery_result,
                developer_mode_result,
                developer_image_result,
                activation_state_result,
            ) = tokio::join!(
                tokio::time::timeout(
                    Duration::from_secs(3),
                    read_device_details(provider.as_ref(), requested_udid),
                ),
                tokio::time::timeout(
                    Duration::from_secs(3),
                    read_device_battery(provider.as_ref()),
                ),
                tokio::time::timeout(
                    Duration::from_secs(3),
                    read_device_developer_mode_status(provider.as_ref()),
                ),
                tokio::time::timeout(
                    Duration::from_secs(3),
                    is_developer_image_mounted(provider.as_ref(), &details.product_version,),
                ),
                tokio::time::timeout(
                    Duration::from_secs(3),
                    read_activation_state(provider.as_ref()),
                ),
            );
            match details_result {
                Ok(Some(refreshed)) => details = refreshed,
                Ok(None) => tracing::warn!("device metadata refresh unavailable"),
                Err(_) => tracing::warn!("device metadata refresh timed out"),
            }
            match battery_result {
                Ok(Ok(battery)) => {
                    tracing::debug!(
                        level_percent = ?battery.level_percent,
                        is_charging = ?battery.is_charging,
                        cycle_count = ?battery.cycle_count,
                        "device battery diagnostics refreshed"
                    );
                    details.battery = Some(battery);
                }
                Ok(Err(error)) => tracing::warn!(%error, "device battery diagnostics unavailable"),
                Err(_) => tracing::warn!("device battery diagnostics timed out"),
            }
            match developer_mode_result {
                Ok(Ok(enabled)) => {
                    tracing::debug!(enabled, "developer mode status refreshed");
                    details.developer_mode_enabled = Some(enabled);
                }
                Ok(Err(error)) => tracing::warn!(%error, "developer mode status unavailable"),
                Err(_) => tracing::warn!("developer mode status timed out"),
            }
            match developer_image_result {
                Ok(Ok(mounted)) => {
                    tracing::debug!(mounted, "developer image status refreshed");
                    details.developer_image_mounted = Some(mounted);
                }
                Ok(Err(error)) => tracing::warn!(%error, "developer image status unavailable"),
                Err(_) => tracing::warn!("developer image status timed out"),
            }
            match activation_state_result {
                Ok(Ok(state)) => {
                    tracing::debug!(?state, "device activation state refreshed");
                    details.activation_state = Some(state);
                }
                Ok(Err(error)) => tracing::warn!(%error, "device activation state unavailable"),
                Err(_) => tracing::warn!("device activation state timed out"),
            }
            let _ = reply.send(Ok(details));
        });
    }
}

fn route_location(port: &super::services::LocationServicePort, command: LocationCommand) {
    if !port.status.get().available {
        command.reject("location simulation is unavailable");
        return;
    }
    route_rejectable(
        &port.sender,
        command,
        "location simulation is busy",
        "location simulation is unavailable",
    );
}

trait RejectableCommand {
    fn reject(self, reason: &str);
}

impl RejectableCommand for LocationCommand {
    fn reject(self, reason: &str) {
        match self {
            Self::Set { reply, .. } | Self::Clear { reply } => {
                let _ = reply.send(Err(reason.into()));
            }
        }
    }
}

impl<HostPath> RejectableCommand for CrashReportExportCommand<HostPath> {
    fn reject(self, reason: &str) {
        CrashReportExportCommand::reject(self, reason);
    }
}

impl RejectableCommand for HomeScreenCommand {
    fn reject(self, reason: &str) {
        HomeScreenCommand::reject(self, reason);
    }
}

impl RejectableCommand for crate::RunningProcessCommand {
    fn reject(self, reason: &str) {
        crate::RunningProcessCommand::reject(self, reason);
    }
}

impl RejectableCommand for crate::AppLifecycleCommand {
    fn reject(self, reason: &str) {
        crate::AppLifecycleCommand::reject(self, reason);
    }
}

impl RejectableCommand for crate::WdaAutomationCommand {
    fn reject(self, reason: &str) {
        crate::WdaAutomationCommand::reject(self, reason);
    }
}

impl RejectableCommand for crate::WdaRunnerCommand {
    fn reject(self, reason: &str) {
        crate::WdaRunnerCommand::reject(self, reason);
    }
}

impl RejectableCommand for crate::AppConsoleCommand {
    fn reject(self, reason: &str) {
        crate::AppConsoleCommand::reject(self, reason);
    }
}

impl<HostPath> RejectableCommand for crate::ProvisioningCommand<HostPath> {
    fn reject(self, reason: &str) {
        crate::ProvisioningCommand::reject(self, reason);
    }
}

fn route_rejectable<T: RejectableCommand>(
    sender: &mpsc::Sender<T>,
    command: T,
    busy: &'static str,
    unavailable: &'static str,
) {
    route(sender, command, busy, unavailable, |command, reason| {
        command.reject(reason)
    });
}

fn route<T>(
    sender: &mpsc::Sender<T>,
    command: T,
    busy: &'static str,
    unavailable: &'static str,
    reject: impl FnOnce(T, &str),
) {
    match sender.try_send(command) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(command)) => reject(command, busy),
        Err(mpsc::error::TrySendError::Closed(command)) => reject(command, unavailable),
    }
}

fn reject_device_condition(command: DeviceConditionCommand, reason: &str) {
    match command {
        DeviceConditionCommand::Apply { reply, .. }
        | DeviceConditionCommand::Clear { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
    }
}

fn reject_network_capture<HostPath>(command: NetworkCaptureCommand<HostPath>, reason: &str) {
    match command {
        NetworkCaptureCommand::Start { reply, .. } | NetworkCaptureCommand::Stop { reply } => {
            let _ = reply.send(Err(reason.into()));
        }
    }
}

fn reject_bluetooth_capture<HostPath>(command: BluetoothCaptureCommand<HostPath>, reason: &str) {
    match command {
        BluetoothCaptureCommand::Start { reply, .. } | BluetoothCaptureCommand::Stop { reply } => {
            let _ = reply.send(Err(reason.into()));
        }
    }
}

fn reject_device_backup<HostPath>(command: DeviceBackupCommand<HostPath>, reason: &str) {
    match command {
        DeviceBackupCommand::Start { reply, .. } | DeviceBackupCommand::Stop { reply } => {
            let _ = reply.send(Err(reason.into()));
        }
    }
}

fn reject_sysdiagnose<HostPath>(command: SysdiagnoseCommand<HostPath>, reason: &str) {
    match command {
        SysdiagnoseCommand::Start { reply, .. } | SysdiagnoseCommand::Stop { reply } => {
            let _ = reply.send(Err(reason.into()));
        }
    }
}

fn reject_log_archive<HostPath>(command: LogArchiveCommand<HostPath>, reason: &str) {
    match command {
        LogArchiveCommand::Start { reply, .. } | LogArchiveCommand::Stop { reply } => {
            let _ = reply.send(Err(reason.into()));
        }
    }
}

fn reject_developer_image<HostPath>(command: DeveloperImageMountCommand<HostPath>, reason: &str) {
    match command {
        DeveloperImageMountCommand::Start { reply, .. }
        | DeveloperImageMountCommand::Stop { reply }
        | DeveloperImageMountCommand::Unmount { reply } => {
            let _ = reply.send(Err(reason.into()));
        }
    }
}

fn reject_app_document<HostPath>(command: crate::AppDocumentCommand<HostPath>, reason: &str) {
    match command {
        crate::AppDocumentCommand::List { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        crate::AppDocumentCommand::Export { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        crate::AppDocumentCommand::Import { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        crate::AppDocumentCommand::CreateDirectory { reply, .. }
        | crate::AppDocumentCommand::Rename { reply, .. }
        | crate::AppDocumentCommand::Delete { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
    }
}

fn reject_device_file<HostPath>(command: DeviceFileCommand<HostPath>, reason: &str) {
    match command {
        DeviceFileCommand::List { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        DeviceFileCommand::Export { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        DeviceFileCommand::Import { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        DeviceFileCommand::CreateDirectory { reply, .. }
        | DeviceFileCommand::Rename { reply, .. }
        | DeviceFileCommand::Delete { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use devicehub_core::{LocationStatus, LocationStatusSlot};

    use super::super::services::LocationServicePort;
    use super::{route, route_location};
    use crate::LocationCommand;

    #[tokio::test]
    async fn bounded_route_reports_busy_and_unavailable_without_losing_command() {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        sender.try_send(1_u8).unwrap();
        let rejection = Arc::new(Mutex::new(None));
        let observed = rejection.clone();
        route(&sender, 2, "busy", "unavailable", move |command, reason| {
            *observed.lock().unwrap() = Some((command, reason.to_string()));
        });
        assert_eq!(*rejection.lock().unwrap(), Some((2, "busy".into())));

        drop(receiver);
        let rejection = Arc::new(Mutex::new(None));
        let observed = rejection.clone();
        route(&sender, 3, "busy", "unavailable", move |command, reason| {
            *observed.lock().unwrap() = Some((command, reason.to_string()));
        });
        assert_eq!(*rejection.lock().unwrap(), Some((3, "unavailable".into())));
    }

    #[tokio::test]
    async fn location_route_rejects_commands_until_the_service_is_ready() {
        let (sender, _receiver) = tokio::sync::mpsc::channel(1);
        let port = LocationServicePort {
            sender,
            status: LocationStatusSlot::default(),
        };
        let (reply, result) = tokio::sync::oneshot::channel();

        route_location(&port, LocationCommand::Clear { reply });

        assert_eq!(
            result.await.unwrap().unwrap_err(),
            "location simulation is unavailable"
        );
    }

    #[tokio::test]
    async fn location_route_forwards_commands_after_the_service_is_ready() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        let status = LocationStatusSlot::default();
        status.set(LocationStatus {
            available: true,
            ..LocationStatus::default()
        });
        let port = LocationServicePort { sender, status };
        let (reply, result) = tokio::sync::oneshot::channel();

        route_location(
            &port,
            LocationCommand::Set {
                latitude: 25.033,
                longitude: 121.5654,
                reply,
            },
        );

        let LocationCommand::Set {
            latitude,
            longitude,
            reply,
        } = receiver.recv().await.unwrap()
        else {
            panic!("expected set location command");
        };
        assert_eq!((latitude, longitude), (25.033, 121.5654));
        reply.send(Ok(())).unwrap();
        assert_eq!(result.await.unwrap(), Ok(()));
    }
}
