//! Per-session device service composition and lifecycle ownership.
//!
//! A connected device exposes many independent, best-effort services. This
//! module creates their bounded command channels, registers every background
//! task with one supervisor, and guarantees that shutdown reaches the whole
//! service tree. The session orchestrator only retains the bridges it needs for
//! input dispatch and no longer knows how each optional service is constructed.

use std::sync::Arc;

use idevice::{provider::IdeviceProvider, rsd::RsdHandshake, tcp::handle::AdapterHandle};

use super::SessionViews;
use crate::location::{self, LocationCommand};
use crate::protocol::{ConnKind, LocationStatus, LocationStatusSlot};
use crate::supervisor::ServiceSupervisor;

pub(super) struct LocationBridge {
    pub(super) sender: tokio::sync::mpsc::Sender<LocationCommand>,
    pub(super) status: LocationStatusSlot,
}

/// Command endpoints consumed by [`super::DeviceManagement`]. Keeping them in
/// one value makes service availability follow the lifetime of a device session
/// instead of leaking individual senders into the outer connection manager.
pub(super) struct DeviceManagementServices {
    pub(super) icons: tokio::sync::mpsc::Sender<crate::app_icons::AppIconCommand>,
    pub(super) companions:
        tokio::sync::mpsc::Sender<crate::companion_devices::CompanionDeviceCommand>,
    pub(super) home_screen: tokio::sync::mpsc::Sender<crate::home_screen::HomeScreenCommand>,
    pub(super) running_processes:
        tokio::sync::mpsc::Sender<crate::running_processes::RunningProcessCommand>,
    pub(super) app_lifecycle: tokio::sync::mpsc::Sender<crate::app_lifecycle::AppLifecycleCommand>,
    pub(super) wda: tokio::sync::mpsc::Sender<crate::wda_automation::WdaAutomationCommand>,
    pub(super) wda_runner: tokio::sync::mpsc::Sender<crate::wda_runner::WdaRunnerCommand>,
    pub(super) app_console: tokio::sync::mpsc::Sender<crate::app_console::AppConsoleCommand>,
    pub(super) documents: tokio::sync::mpsc::Sender<crate::app_documents::AppDocumentCommand>,
    pub(super) device_files: tokio::sync::mpsc::Sender<crate::device_files::DeviceFileCommand>,
    pub(super) screen_capture:
        tokio::sync::mpsc::Sender<crate::screen_capture::ScreenCaptureCommand>,
    pub(super) network_capture:
        tokio::sync::mpsc::Sender<crate::network_capture::NetworkCaptureCommand>,
    pub(super) bluetooth_capture:
        tokio::sync::mpsc::Sender<crate::bluetooth_capture::BluetoothCaptureCommand>,
    pub(super) device_backup: tokio::sync::mpsc::Sender<crate::device_backup::DeviceBackupCommand>,
    pub(super) sysdiagnose: tokio::sync::mpsc::Sender<crate::sysdiagnose::SysdiagnoseCommand>,
    pub(super) log_archive: tokio::sync::mpsc::Sender<crate::log_archive::LogArchiveCommand>,
    pub(super) developer_image:
        tokio::sync::mpsc::Sender<crate::developer_image::DeveloperImageMountCommand>,
    pub(super) device_conditions:
        tokio::sync::mpsc::Sender<crate::device_conditions::DeviceConditionCommand>,
    pub(super) provisioning: tokio::sync::mpsc::Sender<crate::provisioning::ProvisioningCommand>,
}

/// Owns all optional background services for exactly one connected device.
///
/// `management` is handed once to the input dispatcher. The supervisor remains
/// here so both the full screen-control path and management-only fallback use
/// the same deterministic shutdown sequence.
pub(super) struct SessionServices {
    supervisor: ServiceSupervisor,
    location: LocationBridge,
    management: Option<DeviceManagementServices>,
}

impl SessionServices {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn start(
        provider: Arc<dyn IdeviceProvider>,
        connection: ConnKind,
        adapter: AdapterHandle,
        handshake: RsdHandshake,
        requested_udid: String,
        views: &SessionViews,
    ) -> Self {
        views.performance.reset();
        views.device_logs.reset();
        views.device_events.reset();

        let mut supervisor = ServiceSupervisor::new(views.services.clone());
        supervisor.spawn(crate::heartbeat::supervise(
            provider.clone(),
            supervisor.reporter("device.heartbeat"),
            supervisor.shutdown_receiver(),
        ));
        supervisor.spawn(crate::device_logs::supervise(
            adapter.clone(),
            handshake.clone(),
            views.device_logs.clone(),
            supervisor.reporter("device.logs"),
            views.device_log_demand.subscribe(),
            supervisor.shutdown_receiver(),
        ));
        supervisor.spawn(crate::device_events::supervise(
            adapter.clone(),
            handshake.clone(),
            views.device_events.clone(),
            supervisor.reporter("device.notifications"),
            supervisor.shutdown_receiver(),
        ));
        supervisor.spawn(crate::performance::supervise_system(
            adapter.clone(),
            handshake.clone(),
            views.performance.clone(),
            supervisor.reporter("performance.system"),
            views.performance_demand.subscribe(),
            supervisor.shutdown_receiver(),
        ));
        supervisor.spawn(crate::performance::supervise_graphics(
            adapter.clone(),
            handshake.clone(),
            views.performance.clone(),
            supervisor.reporter("performance.graphics"),
            views.performance_demand.subscribe(),
            supervisor.shutdown_receiver(),
        ));
        supervisor.spawn(crate::performance::supervise_network(
            adapter.clone(),
            handshake.clone(),
            views.performance.clone(),
            supervisor.reporter("performance.network"),
            views.performance_demand.subscribe(),
            supervisor.shutdown_receiver(),
        ));
        supervisor.spawn(crate::performance::supervise_energy(
            adapter.clone(),
            handshake.clone(),
            views.performance.clone(),
            supervisor.reporter("performance.energy"),
            views.performance_demand.subscribe(),
            supervisor.shutdown_receiver(),
        ));
        supervisor.spawn(crate::performance::supervise_app_activity(
            adapter.clone(),
            handshake.clone(),
            views.performance.clone(),
            supervisor.reporter("performance.app_activity"),
            views.performance_demand.subscribe(),
            supervisor.shutdown_receiver(),
        ));

        views.location.set(LocationStatus::default());
        let (location_sender, location_receiver) = tokio::sync::mpsc::channel(8);
        supervisor.spawn(location::supervise(
            adapter.clone(),
            handshake.clone(),
            provider.clone(),
            location_receiver,
            views.location.clone(),
            supervisor.reporter("location"),
            supervisor.shutdown_receiver(),
        ));
        let location = LocationBridge {
            sender: location_sender,
            status: views.location.clone(),
        };

        let (icons, icon_commands) = tokio::sync::mpsc::channel(16);
        supervisor.spawn(crate::app_icons::serve(
            adapter.clone(),
            handshake.clone(),
            icon_commands,
            supervisor.shutdown_receiver(),
        ));
        let (companions, companion_commands) = tokio::sync::mpsc::channel(2);
        supervisor.spawn(crate::companion_devices::serve(
            adapter.clone(),
            handshake.clone(),
            companion_commands,
            supervisor.reporter("device.companions"),
            supervisor.shutdown_receiver(),
        ));
        let (home_screen, home_screen_commands) = tokio::sync::mpsc::channel(2);
        supervisor.spawn(crate::home_screen::serve(
            adapter.clone(),
            handshake.clone(),
            home_screen_commands,
            supervisor.reporter("device.home_screen"),
            supervisor.shutdown_receiver(),
        ));
        let (running_processes, running_process_commands) = tokio::sync::mpsc::channel(2);
        supervisor.spawn(crate::running_processes::serve(
            adapter.clone(),
            handshake.clone(),
            running_process_commands,
            supervisor.reporter("performance.process_inventory"),
            supervisor.shutdown_receiver(),
        ));
        let (app_lifecycle, app_lifecycle_commands) = tokio::sync::mpsc::channel(2);
        supervisor.spawn(crate::app_lifecycle::serve(
            adapter.clone(),
            handshake.clone(),
            app_lifecycle_commands,
            supervisor.reporter("device.app_lifecycle"),
            supervisor.shutdown_receiver(),
        ));
        let (wda, wda_commands) = tokio::sync::mpsc::channel(4);
        supervisor.spawn(crate::wda_automation::serve(
            provider.clone(),
            wda_commands,
            supervisor.reporter("device.wda"),
            supervisor.shutdown_receiver(),
        ));
        let (wda_runner, wda_runner_commands) = tokio::sync::mpsc::channel(2);
        supervisor.spawn(crate::wda_runner::serve(
            provider.clone(),
            wda_runner_commands,
            supervisor.reporter("device.wda_runner"),
            supervisor.shutdown_receiver(),
        ));
        let (app_console, app_console_commands) = tokio::sync::mpsc::channel(4);
        supervisor.spawn(crate::app_console::serve(
            adapter.clone(),
            handshake.clone(),
            app_console_commands,
            supervisor.reporter("device.app_console"),
            supervisor.shutdown_receiver(),
        ));
        let (documents, document_commands) = tokio::sync::mpsc::channel(8);
        supervisor.spawn(crate::app_documents::serve(
            crate::app_documents::AppStorageTransport::new(
                provider.clone(),
                connection,
                adapter.clone(),
                handshake.clone(),
            ),
            document_commands,
            views.app_document_activity.clone(),
            supervisor.shutdown_receiver(),
        ));
        let (device_files, device_file_commands) = tokio::sync::mpsc::channel(8);
        supervisor.spawn(crate::device_files::serve(
            crate::device_files::DeviceFileTransport::new(
                provider.clone(),
                connection,
                adapter.clone(),
                handshake.clone(),
            ),
            device_file_commands,
            views.device_file_activity.clone(),
            supervisor.reporter("device.files"),
            supervisor.shutdown_receiver(),
        ));
        let (screen_capture, screen_capture_commands) = tokio::sync::mpsc::channel(1);
        supervisor.spawn(crate::screen_capture::serve(
            crate::screen_capture::ScreenCaptureTransport::new(
                provider.clone(),
                connection,
                adapter.clone(),
                handshake.clone(),
            ),
            screen_capture_commands,
            supervisor.shutdown_receiver(),
        ));
        let (network_capture, network_capture_commands) = tokio::sync::mpsc::channel(4);
        supervisor.spawn(crate::network_capture::serve(
            crate::network_capture::NetworkCaptureTransport::new(
                provider.clone(),
                connection,
                adapter.clone(),
                handshake.clone(),
            ),
            network_capture_commands,
            views.network_capture.clone(),
            supervisor.reporter("network.capture"),
            supervisor.shutdown_receiver(),
        ));
        let (bluetooth_capture, bluetooth_capture_commands) = tokio::sync::mpsc::channel(4);
        supervisor.spawn(crate::bluetooth_capture::serve(
            adapter.clone(),
            handshake.clone(),
            bluetooth_capture_commands,
            views.bluetooth_capture.clone(),
            supervisor.reporter("bluetooth.capture"),
            supervisor.shutdown_receiver(),
        ));
        let (device_backup, device_backup_commands) = tokio::sync::mpsc::channel(4);
        supervisor.spawn(crate::device_backup::serve(
            crate::device_backup::DeviceBackupTransport::new(
                provider.clone(),
                connection,
                adapter.clone(),
                handshake.clone(),
                requested_udid,
            ),
            device_backup_commands,
            views.device_backup.clone(),
            supervisor.reporter("device.backup"),
            supervisor.shutdown_receiver(),
        ));
        let (sysdiagnose, sysdiagnose_commands) = tokio::sync::mpsc::channel(4);
        supervisor.spawn(crate::sysdiagnose::serve(
            adapter.clone(),
            handshake.clone(),
            sysdiagnose_commands,
            views.sysdiagnose.clone(),
            supervisor.reporter("device.sysdiagnose"),
            supervisor.shutdown_receiver(),
        ));
        let (log_archive, log_archive_commands) = tokio::sync::mpsc::channel(4);
        supervisor.spawn(crate::log_archive::serve(
            adapter.clone(),
            handshake.clone(),
            log_archive_commands,
            views.log_archive.clone(),
            supervisor.reporter("device.log_archive"),
            supervisor.shutdown_receiver(),
        ));
        let (developer_image, developer_image_commands) = tokio::sync::mpsc::channel(4);
        supervisor.spawn(crate::developer_image::serve(
            provider.clone(),
            developer_image_commands,
            views.developer_image.clone(),
            supervisor.reporter("device.developer_image"),
            supervisor.shutdown_receiver(),
        ));
        let (device_conditions, device_condition_commands) = tokio::sync::mpsc::channel(4);
        supervisor.spawn(crate::device_conditions::supervise(
            adapter.clone(),
            handshake.clone(),
            device_condition_commands,
            views.device_conditions.clone(),
            supervisor.reporter("device.conditions"),
            supervisor.shutdown_receiver(),
        ));
        let (provisioning, provisioning_commands) = tokio::sync::mpsc::channel(4);
        supervisor.spawn(crate::provisioning::supervise(
            adapter,
            handshake,
            provider,
            provisioning_commands,
            supervisor.reporter("device.provisioning"),
            supervisor.shutdown_receiver(),
        ));

        let management = DeviceManagementServices {
            icons,
            companions,
            home_screen,
            running_processes,
            app_lifecycle,
            wda,
            wda_runner,
            app_console,
            documents,
            device_files,
            screen_capture,
            network_capture,
            bluetooth_capture,
            device_backup,
            sysdiagnose,
            log_archive,
            developer_image,
            device_conditions,
            provisioning,
        };
        Self {
            supervisor,
            location,
            management: Some(management),
        }
    }

    pub(super) fn location(&self) -> &LocationBridge {
        &self.location
    }

    /// The input dispatcher is the sole command owner. Taking rather than
    /// cloning this bundle makes accidental duplicate dispatchers impossible.
    pub(super) fn take_management(&mut self) -> DeviceManagementServices {
        self.management
            .take()
            .expect("device management services already taken")
    }

    pub(super) async fn shutdown(self) {
        let Self {
            mut supervisor,
            location,
            management,
        } = self;
        // Closing command senders first lets idle workers exit naturally before
        // the supervisor broadcasts cancellation to any active operation.
        drop(location);
        drop(management);
        supervisor.shutdown().await;
    }
}
