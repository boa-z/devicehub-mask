//! Desktop binding for runtime-owned unified-log archive export.

use std::path::{Path, PathBuf};

#[cfg(test)]
pub(crate) use devicehub_core::LogArchiveState;
pub(crate) use devicehub_core::LogArchiveStatus;
pub(crate) use devicehub_runtime::LogArchiveSlot;
pub(crate) type LogArchiveCommand = devicehub_runtime::LogArchiveCommand<PathBuf>;

pub(crate) fn validate_age_limit_hours(value: u16) -> Result<u16, String> {
    devicehub_runtime::validate_log_archive_age_limit_hours(value)
}

pub(crate) async fn prepare_destination(destination: &Path) -> Result<PathBuf, String> {
    crate::diagnostic_files::prepare_destination(destination, "log archive").await
}
