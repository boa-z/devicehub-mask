//! Per-session device service composition and lifecycle ownership.
//!
//! A connected device exposes many independent, best-effort services. This
//! module injects desktop host capabilities once, while the runtime creates
//! bounded command channels, registers every task with one supervisor, and
//! guarantees that shutdown reaches the whole service tree.

use std::path::PathBuf;
use std::sync::Arc;

use idevice::{provider::IdeviceProvider, rsd::RsdHandshake, tcp::handle::AdapterHandle};

use super::manager::SessionViews;
use crate::protocol::ConnKind;
use devicehub_runtime::{
    RuntimeConnectedSessionServices, RuntimeHostServiceViews, RuntimeSessionHostAdapters,
    RuntimeSessionServices,
};

/// Inject desktop capabilities into the runtime-owned connected service tree.
pub(super) fn start(
    provider: Arc<dyn IdeviceProvider>,
    connection: ConnKind,
    adapter: AdapterHandle,
    handshake: RsdHandshake,
    requested_udid: String,
    views: &SessionViews,
) -> RuntimeConnectedSessionServices<PathBuf> {
    RuntimeSessionServices::start(
        provider.clone(),
        connection,
        adapter.clone(),
        handshake.clone(),
        views.runtime_services.clone(),
    )
    .attach_host_services(
        provider,
        connection,
        adapter,
        handshake,
        requested_udid,
        RuntimeHostServiceViews {
            app_documents: views.app_document_activity.clone(),
            device_files: views.device_file_activity.clone(),
            network_capture: views.network_capture.clone(),
            bluetooth_capture: views.bluetooth_capture.clone(),
            device_backup: views.device_backup.clone(),
            sysdiagnose: views.sysdiagnose.clone(),
            log_archive: views.log_archive.clone(),
            developer_image: views.developer_image.clone(),
        },
        RuntimeSessionHostAdapters {
            files: crate::host_files::TokioHostFileIo,
            capture_files: crate::capture_files::TokioCaptureFileIo,
            backup: crate::device_backup::TokioDeviceBackupExecutor,
            developer_images: crate::developer_image::TokioDeveloperImageAssets,
            provisioning_profiles: crate::provisioning::TokioProvisioningProfiles,
        },
    )
}
