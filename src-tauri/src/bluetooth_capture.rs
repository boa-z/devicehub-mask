//! Desktop binding for runtime-owned Bluetooth capture.

use std::path::{Path, PathBuf};

use devicehub_runtime::{CaptureFileIo, CaptureFileKind};

pub(crate) use devicehub_runtime::{BluetoothCaptureSlot, BluetoothCaptureStatus};
pub(crate) type BluetoothCaptureCommand = devicehub_runtime::BluetoothCaptureCommand<PathBuf>;

pub(crate) async fn validate_request(path: &Path, duration_seconds: u64) -> Result<(), String> {
    devicehub_runtime::validate_bluetooth_capture_duration(duration_seconds)?;
    crate::capture_files::TokioCaptureFileIo
        .validate(&path.to_path_buf(), CaptureFileKind::Bluetooth)
        .await
}

pub(crate) async fn serve(
    transport: devicehub_runtime::BluetoothCaptureTransport,
    commands: tokio::sync::mpsc::Receiver<BluetoothCaptureCommand>,
    status: BluetoothCaptureSlot,
    reporter: crate::supervisor::ServiceReporter,
    shutdown: tokio::sync::watch::Receiver<bool>,
) {
    devicehub_runtime::serve_bluetooth_capture(
        transport,
        commands,
        status,
        crate::capture_files::TokioCaptureFileIo,
        reporter,
        shutdown,
    )
    .await;
}
