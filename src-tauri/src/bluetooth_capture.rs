//! Desktop binding for runtime-owned Bluetooth capture.

use std::path::{Path, PathBuf};

use devicehub_runtime::{CaptureFileIo, CaptureFileKind};

pub(crate) use devicehub_core::BluetoothCaptureStatus;
pub(crate) use devicehub_runtime::BluetoothCaptureSlot;
pub(crate) type BluetoothCaptureCommand = devicehub_runtime::BluetoothCaptureCommand<PathBuf>;

pub(crate) async fn validate_request(path: &Path, duration_seconds: u64) -> Result<(), String> {
    devicehub_runtime::validate_bluetooth_capture_duration(duration_seconds)?;
    crate::capture_files::TokioCaptureFileIo
        .validate(&path.to_path_buf(), CaptureFileKind::Bluetooth)
        .await
}
