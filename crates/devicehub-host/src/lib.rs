//! Shared operating-system adapters used by desktop and headless hosts.
//!
//! This crate resolves files and child processes, but does not own device
//! policy, listeners, authentication, or application lifecycle.

pub mod browser_transfers;
pub mod capture_files;
pub mod decode;
pub mod developer_image;
pub mod device_backup;
pub mod diagnostic_files;
pub mod diagnostic_sinks;
pub mod host_files;
pub mod keymap_catalog;
pub mod netmuxd;
pub mod private_api;
pub mod profile_files;
pub mod provisioning;
pub mod wifi_devices;

use devicehub_runtime::RuntimeSessionHostAdapters;

/// Construct the local-filesystem capabilities shared by all native hosts.
pub fn session_adapters() -> RuntimeSessionHostAdapters<
    host_files::TokioHostFileIo,
    capture_files::TokioCaptureFileIo,
    device_backup::TokioDeviceBackupDestination,
    developer_image::TokioDeveloperImageAssets,
    provisioning::TokioProvisioningProfiles,
> {
    RuntimeSessionHostAdapters {
        files: host_files::TokioHostFileIo,
        capture_files: capture_files::TokioCaptureFileIo,
        backup: device_backup::TokioDeviceBackupDestination,
        developer_images: developer_image::TokioDeveloperImageAssets,
        provisioning_profiles: provisioning::TokioProvisioningProfiles,
    }
}
