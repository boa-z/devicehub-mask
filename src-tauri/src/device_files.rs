//! Desktop path binding for the runtime-owned public AFC service.

use std::path::PathBuf;

pub(crate) use devicehub_core::{
    DeviceFileActivitySlot, DeviceFileActivityView, DeviceFileEntry, DeviceFileList,
    is_device_file_transfer_cancelled as is_transfer_cancelled,
};
#[cfg(test)]
pub(crate) use devicehub_core::{DeviceFileActivityState, DeviceFileKind, DeviceFileTransfer};

pub(crate) type DeviceFileCommand = devicehub_runtime::DeviceFileCommand<PathBuf>;
