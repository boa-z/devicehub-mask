//! Desktop adapters for runtime-owned connected-device services.
//!
//! A connected device exposes many independent, best-effort services. This
//! module injects desktop host capabilities once, while the runtime creates
//! bounded command channels, registers every task with one supervisor, and
//! guarantees that shutdown reaches the whole service tree.

use devicehub_runtime::RuntimeSessionHostAdapters;

/// Construct filesystem-backed desktop capabilities without exposing runtime
/// service clients or lifecycle controls to the host.
pub(super) fn adapters() -> RuntimeSessionHostAdapters<
    crate::host_files::TokioHostFileIo,
    crate::capture_files::TokioCaptureFileIo,
    crate::device_backup::TokioDeviceBackupExecutor,
    crate::developer_image::TokioDeveloperImageAssets,
    crate::provisioning::TokioProvisioningProfiles,
> {
    RuntimeSessionHostAdapters {
        files: crate::host_files::TokioHostFileIo,
        capture_files: crate::capture_files::TokioCaptureFileIo,
        backup: crate::device_backup::TokioDeviceBackupExecutor,
        developer_images: crate::developer_image::TokioDeveloperImageAssets,
        provisioning_profiles: crate::provisioning::TokioProvisioningProfiles,
    }
}
