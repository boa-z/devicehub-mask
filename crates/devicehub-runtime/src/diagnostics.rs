//! Supervised device diagnostic exports with host-injected persistence.

mod device_backup;
mod log_archive;
mod sysdiagnose;

pub use device_backup::{DeviceBackupCommand, DeviceBackupDestination, DeviceBackupPrepareFuture};
pub(crate) use device_backup::{DeviceBackupTransport, serve as serve_device_backup};
pub(crate) use log_archive::serve as serve_log_archive;
pub use log_archive::{
    ALLOWED_LOG_ARCHIVE_AGE_LIMIT_HOURS, LogArchiveCommand,
    validate_age_limit_hours as validate_log_archive_age_limit_hours,
};
pub use sysdiagnose::SysdiagnoseCommand;
pub(crate) use sysdiagnose::serve as serve_sysdiagnose;
