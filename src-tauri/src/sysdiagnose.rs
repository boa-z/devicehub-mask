//! Desktop binding for runtime-owned sysdiagnose export.

use std::path::{Path, PathBuf};

#[cfg(test)]
pub(crate) use devicehub_runtime::SysdiagnoseState;
pub(crate) use devicehub_runtime::{SysdiagnoseSlot, SysdiagnoseStatus};
pub(crate) type SysdiagnoseCommand = devicehub_runtime::SysdiagnoseCommand<PathBuf>;

pub(crate) async fn prepare_destination(destination: &Path) -> Result<PathBuf, String> {
    crate::diagnostic_files::prepare_destination(destination, "sysdiagnose").await
}

pub(crate) async fn serve(
    adapter: idevice::tcp::handle::AdapterHandle,
    handshake: idevice::rsd::RsdHandshake,
    commands: tokio::sync::mpsc::Receiver<SysdiagnoseCommand>,
    status: SysdiagnoseSlot,
    reporter: crate::supervisor::ServiceReporter,
    shutdown: tokio::sync::watch::Receiver<bool>,
) {
    devicehub_runtime::serve_sysdiagnose(
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
