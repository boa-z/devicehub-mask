//! Supervised device diagnostic exports with host-injected persistence.

mod device_backup;
mod log_archive;
mod sysdiagnose;

pub use device_backup::{
    DeviceBackupCommand, DeviceBackupExecutor, DeviceBackupFuture, DeviceBackupPrepareFuture,
    DeviceBackupSlot, DeviceBackupTransport, serve as serve_device_backup,
};
pub use devicehub_core::{
    DeviceBackupState, DeviceBackupStatus, LogArchiveState, LogArchiveStatus, SysdiagnoseState,
    SysdiagnoseStatus,
};
pub use log_archive::{
    ALLOWED_LOG_ARCHIVE_AGE_LIMIT_HOURS, LogArchiveCommand, LogArchiveSlot,
    serve as serve_log_archive, validate_age_limit_hours as validate_log_archive_age_limit_hours,
};
pub use sysdiagnose::{SysdiagnoseCommand, SysdiagnoseSlot, serve as serve_sysdiagnose};
