//! Host filesystem adapter for exporting device crash reports.

use std::path::Path;
use std::sync::Arc;

use idevice::provider::IdeviceProvider;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, watch};

use devicehub_runtime::CrashReportExportCommand;

/// Serves host filesystem exports under the connected session supervisor.
/// Runtime code owns device access while this adapter owns path validation.
pub async fn serve(
    provider: Arc<dyn IdeviceProvider>,
    mut commands: mpsc::Receiver<CrashReportExportCommand<std::path::PathBuf>>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            command = commands.recv() => {
                let Some(CrashReportExportCommand::Export {
                    device_path,
                    destination,
                    reply,
                }) = command else {
                    break;
                };
                tokio::select! {
                    _ = shutdown.changed() => {
                        let _ = reply.send(Err("crash report export was cancelled".into()));
                        break;
                    }
                    result = export(provider.clone(), device_path, &destination) => {
                        let _ = reply.send(result);
                    }
                }
            }
        }
    }
}

pub async fn export(
    provider: Arc<dyn IdeviceProvider>,
    device_path: String,
    destination: &Path,
) -> Result<u64, String> {
    devicehub_runtime::validate_crash_report_path(&device_path)?;
    validate_destination(destination).await?;
    let data = devicehub_runtime::download_crash_report(provider, device_path).await?;
    let mut file = tokio::fs::File::create(destination)
        .await
        .map_err(|error| format!("unable to create export file: {error}"))?;
    file.write_all(&data)
        .await
        .map_err(|error| format!("unable to write export file: {error}"))?;
    file.flush()
        .await
        .map_err(|error| format!("unable to finish export file: {error}"))?;
    Ok(data.len() as u64)
}

async fn validate_destination(path: &Path) -> Result<(), String> {
    if path.file_name().is_none() {
        return Err("invalid crash report destination".to_string());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "crash report destination has no parent directory".to_string())?;
    let metadata = tokio::fs::metadata(parent)
        .await
        .map_err(|error| format!("unable to access export directory: {error}"))?;
    if !metadata.is_dir() {
        return Err("crash report export parent is not a directory".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn export_destination_requires_a_filename_and_existing_directory() {
        assert!(validate_destination(Path::new("")).await.is_err());
        let destination = std::env::temp_dir().join("devicehub-mask-crash-report-test.ips");
        assert!(validate_destination(&destination).await.is_ok());
    }
}
