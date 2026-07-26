//! Per-session device service composition and lifecycle ownership.
//!
//! A connected device exposes many independent, best-effort services. This
//! module creates their bounded command channels, registers every background
//! task with one supervisor, and guarantees that shutdown reaches the whole
//! service tree. The session orchestrator only retains the bridges it needs for
//! input dispatch and no longer knows how each optional service is constructed.

use std::path::PathBuf;
use std::sync::Arc;

use idevice::{provider::IdeviceProvider, rsd::RsdHandshake, tcp::handle::AdapterHandle};

use super::manager::SessionViews;
use crate::protocol::ConnKind;
use devicehub_runtime::{DeviceServicePorts, RuntimeSessionServices};

pub(super) type DeviceManagementServices = DeviceServicePorts<PathBuf>;

/// Owns all optional background services for exactly one connected device.
///
/// `management` is handed once to the input dispatcher. Runtime-owned and
/// host-backed services share the deterministic shutdown tree owned by
/// [`RuntimeSessionServices`].
pub(super) struct SessionServices {
    runtime: RuntimeSessionServices,
    management: Option<DeviceManagementServices>,
}

impl SessionServices {
    pub(super) fn start(
        provider: Arc<dyn IdeviceProvider>,
        connection: ConnKind,
        adapter: AdapterHandle,
        handshake: RsdHandshake,
        requested_udid: String,
        views: &SessionViews,
    ) -> Self {
        let mut runtime = RuntimeSessionServices::start(
            provider.clone(),
            connection,
            adapter.clone(),
            handshake.clone(),
            views.runtime_services.clone(),
        );
        let runtime_ports = runtime.take_device_ports();
        let (documents, document_commands) = tokio::sync::mpsc::channel(8);
        runtime.spawn_host_task(crate::app_documents::serve(
            crate::app_documents::AppStorageTransport::new(
                provider.clone(),
                connection,
                adapter.clone(),
                handshake.clone(),
            ),
            document_commands,
            views.app_document_activity.clone(),
            crate::host_files::TokioHostFileIo,
            runtime.shutdown_receiver(),
        ));
        let (device_files, device_file_commands) = tokio::sync::mpsc::channel(8);
        runtime.spawn_host_task(crate::device_files::serve(
            crate::device_files::DeviceFileTransport::new(
                provider.clone(),
                connection,
                adapter.clone(),
                handshake.clone(),
            ),
            device_file_commands,
            views.device_file_activity.clone(),
            crate::host_files::TokioHostFileIo,
            runtime.reporter("device.files"),
            runtime.shutdown_receiver(),
        ));
        let (network_capture, network_capture_commands) = tokio::sync::mpsc::channel(4);
        runtime.spawn_host_task(crate::network_capture::serve(
            crate::network_capture::NetworkCaptureTransport::new(
                provider.clone(),
                connection,
                adapter.clone(),
                handshake.clone(),
            ),
            network_capture_commands,
            views.network_capture.clone(),
            runtime.reporter("network.capture"),
            runtime.shutdown_receiver(),
        ));
        let (bluetooth_capture, bluetooth_capture_commands) = tokio::sync::mpsc::channel(4);
        runtime.spawn_host_task(crate::bluetooth_capture::serve(
            devicehub_runtime::BluetoothCaptureTransport::new(adapter.clone(), handshake.clone()),
            bluetooth_capture_commands,
            views.bluetooth_capture.clone(),
            runtime.reporter("bluetooth.capture"),
            runtime.shutdown_receiver(),
        ));
        let (device_backup, device_backup_commands) = tokio::sync::mpsc::channel(4);
        runtime.spawn_host_task(crate::device_backup::serve(
            crate::device_backup::DeviceBackupTransport::new(
                provider.clone(),
                connection,
                adapter.clone(),
                handshake.clone(),
                requested_udid,
            ),
            device_backup_commands,
            views.device_backup.clone(),
            runtime.reporter("device.backup"),
            runtime.shutdown_receiver(),
        ));
        let (sysdiagnose, sysdiagnose_commands) = tokio::sync::mpsc::channel(4);
        runtime.spawn_host_task(crate::sysdiagnose::serve(
            adapter.clone(),
            handshake.clone(),
            sysdiagnose_commands,
            views.sysdiagnose.clone(),
            runtime.reporter("device.sysdiagnose"),
            runtime.shutdown_receiver(),
        ));
        let (log_archive, log_archive_commands) = tokio::sync::mpsc::channel(4);
        runtime.spawn_host_task(crate::log_archive::serve(
            adapter.clone(),
            handshake.clone(),
            log_archive_commands,
            views.log_archive.clone(),
            runtime.reporter("device.log_archive"),
            runtime.shutdown_receiver(),
        ));
        let (developer_image, developer_image_commands) = tokio::sync::mpsc::channel(4);
        runtime.spawn_host_task(crate::developer_image::serve(
            provider.clone(),
            developer_image_commands,
            views.developer_image.clone(),
            crate::developer_image::TokioDeveloperImageAssets,
            runtime.reporter("device.developer_image"),
            runtime.shutdown_receiver(),
        ));
        let (provisioning, provisioning_commands) = tokio::sync::mpsc::channel(4);
        runtime.spawn_host_task(crate::provisioning::supervise(
            adapter,
            handshake,
            provider.clone(),
            provisioning_commands,
            runtime.reporter("device.provisioning"),
            runtime.shutdown_receiver(),
        ));
        let (crash_report_exports, crash_report_export_commands) = tokio::sync::mpsc::channel(2);
        runtime.spawn_host_task(crate::crash_reports::serve(
            provider,
            crash_report_export_commands,
            runtime.shutdown_receiver(),
        ));

        let management = DeviceServicePorts {
            location: runtime_ports.location,
            icons: runtime_ports.icons,
            companions: runtime_ports.companions,
            home_screen: runtime_ports.home_screen,
            running_processes: runtime_ports.running_processes,
            app_lifecycle: runtime_ports.app_lifecycle,
            wda: runtime_ports.wda,
            wda_runner: runtime_ports.wda_runner,
            app_console: runtime_ports.app_console,
            documents,
            device_files,
            screen_capture: runtime_ports.screen_capture,
            network_capture,
            bluetooth_capture,
            device_backup,
            sysdiagnose,
            log_archive,
            developer_image,
            device_conditions: runtime_ports.device_conditions,
            provisioning,
            crash_report_exports,
        };
        Self {
            runtime,
            management: Some(management),
        }
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
            runtime,
            management,
        } = self;
        // Closing command senders first lets idle workers exit naturally before
        // the supervisor broadcasts cancellation to any active operation.
        drop(management);
        runtime.shutdown().await;
    }
}
