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

pub use companions::CompanionDeviceCommand;
pub(crate) use companions::serve_companion_devices;
pub(crate) use conditions::supervise_device_conditions;
pub use conditions::{
    DeviceConditionCommand, validate_identifiers as validate_device_condition_identifiers,
};
pub use crash_reports::{CrashReportExportCommand, MAX_CRASH_REPORT_READ_BYTES};
pub(crate) use crash_reports::{
    delete_crash_report, list_crash_reports, read_crash_report, serve_crash_report_exports,
};
pub(crate) use details::{
    read_activation_state, read_device_battery, read_device_details,
    read_device_developer_mode_status, rename_device,
};
pub(crate) use developer_image::serve_developer_image_mount;
pub use developer_image::{
    DeveloperImageAssetFuture, DeveloperImageAssetLoader, DeveloperImageMountCommand,
    DeveloperImageMountRequest,
};
pub(crate) use developer_image::{
    is_developer_image_mounted, is_developer_image_mounted_for_device,
};
pub(crate) use developer_mode::execute_developer_mode;
pub use developer_mode::{DeveloperModeCommand, DeveloperModePreparation};
pub use events::DeviceEventSlot;
pub(crate) use events::supervise_device_events;
pub use home_screen::HomeScreenCommand;
pub(crate) use home_screen::serve_home_screen;
pub use location::LocationCommand;
pub(crate) use location::supervise_location;
pub use logs::DeviceLogDemand;
pub(crate) use logs::supervise_device_logs;
pub(crate) use power::{DevicePowerAction, DevicePowerController};
pub(crate) use provisioning::supervise_provisioning;
pub use provisioning::{
    MAX_PROVISIONING_PROFILE_BYTES, ProvisioningCommand, ProvisioningFailure, ProvisioningInstall,
    ProvisioningProfileFuture, ProvisioningProfileLoader, parse_provisioning_profile,
    prepare_provisioning_install, profiles_from_raw, unreadable_profile,
};
pub use screenshot::ScreenCaptureCommand;
pub(crate) use screenshot::{ScreenCaptureTransport, serve_screen_capture};
