//! Desktop binding for runtime-owned network capture.

use std::path::{Path, PathBuf};

use devicehub_runtime::{CaptureFileIo, CaptureFileKind};

pub(crate) use devicehub_runtime::{NetworkCaptureSlot, NetworkCaptureStatus};
pub(crate) type NetworkCaptureCommand = devicehub_runtime::NetworkCaptureCommand<PathBuf>;

pub(crate) async fn validate_request(path: &Path, duration_seconds: u64) -> Result<(), String> {
    devicehub_runtime::validate_network_capture_duration(duration_seconds)?;
    crate::capture_files::TokioCaptureFileIo
        .validate(&path.to_path_buf(), CaptureFileKind::Network)
        .await
}
