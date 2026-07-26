//! Desktop path binding for the runtime-owned public AFC service.

use std::path::PathBuf;

pub(crate) use devicehub_runtime::{
    DeviceFileActivitySlot, DeviceFileActivityView, DeviceFileEntry, DeviceFileList,
    is_transfer_cancelled,
};
#[cfg(test)]
pub(crate) use devicehub_runtime::{DeviceFileActivityState, DeviceFileKind, DeviceFileTransfer};

pub(crate) type DeviceFileCommand = devicehub_runtime::DeviceFileCommand<PathBuf>;
