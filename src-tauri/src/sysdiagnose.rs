//! Desktop binding for runtime-owned sysdiagnose export.

use std::path::{Path, PathBuf};

#[cfg(test)]
pub(crate) use devicehub_core::SysdiagnoseState;
pub(crate) use devicehub_core::SysdiagnoseStatus;
pub(crate) use devicehub_runtime::SysdiagnoseSlot;
pub(crate) type SysdiagnoseCommand = devicehub_runtime::SysdiagnoseCommand<PathBuf>;

pub(crate) async fn prepare_destination(destination: &Path) -> Result<PathBuf, String> {
    crate::diagnostic_files::prepare_destination(destination, "sysdiagnose").await
}
