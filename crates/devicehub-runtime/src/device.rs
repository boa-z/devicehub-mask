//! Device-wide runtime capabilities.

mod companions;
mod conditions;
mod crash_reports;
mod details;
mod developer_image;
mod developer_mode;
mod events;
mod home_screen;
mod location;
mod logs;
mod power;
mod provisioning;
mod screenshot;

pub use companions::{CompanionDeviceCommand, serve_companion_devices};
pub use conditions::{
    DeviceConditionCommand, DeviceConditionSlot, supervise_device_conditions,
    validate_identifiers as validate_device_condition_identifiers,
};
pub use crash_reports::{
    MAX_CRASH_REPORT_READ_BYTES, delete_crash_report, download_crash_report, list_crash_reports,
    read_crash_report, validate_crash_report_path,
};
pub use details::{
    read_activation_state, read_device_battery, read_device_details,
    read_device_developer_mode_status, rename_device,
};
pub use developer_image::{
    DeveloperImageAssetFuture, DeveloperImageAssetLoader, DeveloperImageMountCommand,
    DeveloperImageMountRequest, DeveloperImageMountSlot, DeveloperImageMountState,
    DeveloperImageMountStatus, developer_image_type_for_version, is_developer_image_mounted,
    is_developer_image_mounted_for_device, read_device_product_version,
    serve_developer_image_mount,
};
pub use developer_mode::{
    DeveloperModeCommand, DeveloperModePreparation, execute_developer_mode,
    read_developer_mode_status,
};
pub use events::{DeviceEventSlot, supervise_device_events};
pub use home_screen::{HomeScreenCommand, serve_home_screen};
pub use location::{LocationCommand, supervise_location};
pub use logs::{
    DeviceLogBatch, DeviceLogDemand, DeviceLogEntry, DeviceLogLevel, DeviceLogSlot,
    DeviceLogSource, MAX_BATCH_ENTRIES, supervise_device_logs,
};
pub use power::{DevicePowerAction, DevicePowerController};
pub use provisioning::{
    MAX_PROVISIONING_PROFILE_BYTES, ProvisioningCommand, ProvisioningFailure, ProvisioningInstall,
    parse_provisioning_profile, prepare_provisioning_install, profiles_from_raw,
    supervise_provisioning, unreadable_profile,
};
pub use screenshot::{ScreenCaptureCommand, ScreenCaptureTransport, serve_screen_capture};
