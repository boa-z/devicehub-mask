//! Desktop binding for runtime-owned unified-log archive export.

use std::path::{Path, PathBuf};

#[cfg(test)]
pub(crate) use devicehub_runtime::LogArchiveState;
pub(crate) use devicehub_runtime::{LogArchiveSlot, LogArchiveStatus};
pub(crate) type LogArchiveCommand = devicehub_runtime::LogArchiveCommand<PathBuf>;

pub(crate) fn validate_age_limit_hours(value: u16) -> Result<u16, String> {
    devicehub_runtime::validate_log_archive_age_limit_hours(value)
}

pub(crate) async fn prepare_destination(destination: &Path) -> Result<PathBuf, String> {
    crate::diagnostic_files::prepare_destination(destination, "log archive").await
}

pub(crate) async fn serve(
    adapter: idevice::tcp::handle::AdapterHandle,
    handshake: idevice::rsd::RsdHandshake,
    commands: tokio::sync::mpsc::Receiver<LogArchiveCommand>,
    status: LogArchiveSlot,
    reporter: crate::supervisor::ServiceReporter,
    shutdown: tokio::sync::watch::Receiver<bool>,
) {
    devicehub_runtime::serve_log_archive(
        adapter,
        handshake,
        commands,
        status,
        crate::host_files::TokioHostFileIo,
        reporter,
        shutdown,
    )
    .await;
}
