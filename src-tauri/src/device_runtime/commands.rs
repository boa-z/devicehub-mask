//! Host path bindings for commands owned by `devicehub-runtime`.

use std::path::PathBuf;

pub(crate) type InputCmd = devicehub_runtime::DeviceSessionCommand<PathBuf>;
pub(crate) type ControlCmd = devicehub_runtime::SessionControlCommand;
