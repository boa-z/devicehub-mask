//! Desktop path binding for the runtime-owned public AFC service.

use std::path::PathBuf;

pub(crate) use devicehub_runtime::{
    DeviceFileActivitySlot, DeviceFileActivityView, DeviceFileEntry, DeviceFileList,
    DeviceFileTransport, is_transfer_cancelled, serve_device_files as serve,
};
#[cfg(test)]
pub(crate) use devicehub_runtime::{DeviceFileActivityState, DeviceFileKind, DeviceFileTransfer};

pub(crate) type DeviceFileCommand = devicehub_runtime::DeviceFileCommand<PathBuf>;
