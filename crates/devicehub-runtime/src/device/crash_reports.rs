use std::sync::Arc;
use std::time::Duration;

use devicehub_core::{
    DeviceCrashReport, DeviceCrashReportContent, DeviceCrashReportList, build_crash_report_content,
};
use idevice::IdeviceService;
use idevice::afc::opcode::AfcFopenMode;
use idevice::crashreportcopymobile::{CrashReportCopyMobileClient, flush_reports};
use idevice::provider::IdeviceProvider;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot, watch};

use crate::storage::HostFileIo;

use devicehub_core::validate_crash_report_path;

const MAX_REPORTS: usize = 2_000;
const MAX_ENTRIES: usize = 5_000;
const MAX_DEPTH: usize = 8;
const MAX_EXPORT_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_CRASH_REPORT_READ_BYTES: usize = 1024 * 1024;
const SERVICE_TIMEOUT: Duration = Duration::from_secs(5);

/// Host-backed crash report export request routed by the runtime session.
#[derive(Debug)]
pub enum CrashReportExportCommand<HostPath> {
    Export {
        device_path: String,
        destination: HostPath,
        reply: oneshot::Sender<Result<u64, String>>,
    },
}

impl<HostPath> CrashReportExportCommand<HostPath> {
    pub fn reject(self, reason: &str) {
        match self {
            Self::Export { reply, .. } => {
                let _ = reply.send(Err(reason.into()));
            }
        }
    }
}

/// Serves bounded device downloads while delegating destination validation and
/// persistence to the host filesystem port.
pub(crate) async fn serve_crash_report_exports<Files>(
    provider: Arc<dyn IdeviceProvider>,
    mut commands: mpsc::Receiver<CrashReportExportCommand<Files::Path>>,
    files: Files,
    mut shutdown: watch::Receiver<bool>,
) where
    Files: HostFileIo,
{
    loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => return,
            command = commands.recv() => {
                let Some(CrashReportExportCommand::Export {
                    device_path,
                    destination,
                    reply,
                }) = command else {
                    return;
                };
                tokio::select! {
                    _ = shutdown.changed() => {
                        let _ = reply.send(Err("crash report export was cancelled".into()));
                        return;
                    }
                    result = export_crash_report(
                        provider.clone(),
                        device_path,
                        destination,
                        &files,
                    ) => {
                        let _ = reply.send(result);
                    }
                }
            }
        }
    }
}

async fn export_crash_report<Files>(
    provider: Arc<dyn IdeviceProvider>,
    device_path: String,
    destination: Files::Path,
    files: &Files,
) -> Result<u64, String>
where
    Files: HostFileIo,
{
    validate_crash_report_path(&device_path)?;
    files.validate_export_file(&destination).await?;
    let data = download_crash_report(provider, device_path).await?;
    let mut writer = files.create_writer(&destination).await?;
    writer
        .write_all(&data)
        .await
        .map_err(|error| format!("unable to write crash report export: {error}"))?;
    writer.sync().await?;
    Ok(data.len() as u64)
}

pub(crate) async fn list_crash_reports(
    provider: Arc<dyn IdeviceProvider>,
) -> Result<DeviceCrashReportList, String> {
    match tokio::time::timeout(Duration::from_secs(3), flush_reports(provider.as_ref())).await {
        Ok(Ok(())) => tracing::debug!("device crash reports flushed"),
        Ok(Err(error)) => tracing::warn!("device crash report flush unavailable: {error:?}"),
        Err(_) => tracing::warn!("device crash report flush timed out"),
    }

    let mut client = connect(provider.as_ref()).await?;
    let mut reports = Vec::new();
    let mut directories = vec![(String::from("/"), 0_usize)];
    let mut visited = 0_usize;
    let mut truncated = false;

    while let Some((directory, depth)) = directories.pop() {
        let entries = client
            .ls(Some(&directory))
            .await
            .map_err(|error| format!("unable to list crash reports: {error:?}"))?;
        for name in entries {
            if name == "." || name == ".." {
                continue;
            }
            visited += 1;
            if visited > MAX_ENTRIES {
                truncated = true;
                break;
            }
            let path = child_path(&directory, &name);
            let info = match client.afc_client.get_file_info(&path).await {
                Ok(info) => info,
                Err(error) => {
                    tracing::debug!("unable to inspect crash report entry: {error:?}");
                    continue;
                }
            };
            match info.st_ifmt.as_str() {
                "S_IFDIR" if depth < MAX_DEPTH => directories.push((path, depth + 1)),
                "S_IFDIR" => truncated = true,
                "S_IFREG" if reports.len() < MAX_REPORTS => reports.push(DeviceCrashReport {
                    path,
                    name,
                    size_bytes: info.size as u64,
                    modified: info.modified.and_utc().to_rfc3339(),
                }),
                "S_IFREG" => truncated = true,
                _ => {}
            }
        }
        if visited > MAX_ENTRIES {
            break;
        }
    }

    reports.sort_by(|left, right| {
        right
            .modified
            .cmp(&left.modified)
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(DeviceCrashReportList { reports, truncated })
}

pub(crate) async fn download_crash_report(
    provider: Arc<dyn IdeviceProvider>,
    device_path: String,
) -> Result<Vec<u8>, String> {
    validate_crash_report_path(&device_path)?;
    let mut client = connect(provider.as_ref()).await?;
    let info = client
        .afc_client
        .get_file_info(&device_path)
        .await
        .map_err(|error| format!("unable to inspect crash report: {error:?}"))?;
    validate_regular_file(&info.st_ifmt)?;
    if info.size > MAX_EXPORT_BYTES {
        return Err(format!(
            "crash report exceeds the {} MiB export limit",
            MAX_EXPORT_BYTES / 1024 / 1024
        ));
    }
    read_exact_report(&mut client, device_path, info.size).await
}

pub(crate) async fn read_crash_report(
    provider: Arc<dyn IdeviceProvider>,
    device_path: String,
    max_bytes: usize,
) -> Result<DeviceCrashReportContent, String> {
    validate_crash_report_path(&device_path)?;
    if !(1..=MAX_CRASH_REPORT_READ_BYTES).contains(&max_bytes) {
        return Err(format!(
            "crash report read limit must be between 1 and {} bytes",
            MAX_CRASH_REPORT_READ_BYTES
        ));
    }
    let mut client = connect(provider.as_ref()).await?;
    let info = client
        .afc_client
        .get_file_info(&device_path)
        .await
        .map_err(|error| format!("unable to inspect crash report: {error:?}"))?;
    validate_regular_file(&info.st_ifmt)?;
    let data =
        read_exact_report(&mut client, device_path.clone(), info.size.min(max_bytes)).await?;
    Ok(build_crash_report_content(device_path, info.size, data))
}

pub(crate) async fn delete_crash_report(
    provider: Arc<dyn IdeviceProvider>,
    device_path: String,
) -> Result<(), String> {
    validate_crash_report_path(&device_path)?;
    let mut client = connect(provider.as_ref()).await?;
    let info = client
        .afc_client
        .get_file_info(&device_path)
        .await
        .map_err(|error| format!("unable to inspect crash report: {error:?}"))?;
    validate_regular_file(&info.st_ifmt)?;
    client
        .afc_client
        .remove(&device_path)
        .await
        .map_err(|error| format!("unable to delete crash report: {error:?}"))
}

async fn connect(provider: &dyn IdeviceProvider) -> Result<CrashReportCopyMobileClient, String> {
    tokio::time::timeout(
        SERVICE_TIMEOUT,
        CrashReportCopyMobileClient::connect(provider),
    )
    .await
    .map_err(|_| "crash report service connection timed out".to_string())?
    .map_err(|error| format!("unable to connect to crash report service: {error:?}"))
}

async fn read_exact_report(
    client: &mut CrashReportCopyMobileClient,
    device_path: String,
    expected: usize,
) -> Result<Vec<u8>, String> {
    let mut report = client
        .afc_client
        .open(device_path, AfcFopenMode::RdOnly)
        .await
        .map_err(|error| format!("unable to open crash report: {error:?}"))?;
    let data = report
        .read_n(expected)
        .await
        .map_err(|error| format!("unable to read crash report: {error:?}"))?;
    report
        .close()
        .await
        .map_err(|error| format!("unable to close crash report: {error:?}"))?;
    if data.len() != expected {
        return Err("crash report changed while it was being read".to_string());
    }
    Ok(data)
}

fn validate_regular_file(file_type: &str) -> Result<(), String> {
    if file_type != "S_IFREG" {
        return Err("selected crash report is not a regular file".to_string());
    }
    Ok(())
}

fn child_path(directory: &str, name: &str) -> String {
    if directory == "/" {
        format!("/{name}")
    } else {
        format!("{directory}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use idevice::usbmuxd::{UsbmuxdAddr, UsbmuxdConnection};

    #[test]
    fn joins_root_and_nested_report_paths() {
        assert_eq!(child_path("/", "Report.ips"), "/Report.ips");
        assert_eq!(child_path("/Retired", "Report.ips"), "/Retired/Report.ips");
    }

    #[test]
    fn accepts_only_regular_crash_report_files() {
        assert!(validate_regular_file("S_IFREG").is_ok());
        assert!(validate_regular_file("S_IFDIR").is_err());
        assert!(validate_regular_file("S_IFLNK").is_err());
    }

    #[tokio::test]
    #[ignore = "requires a connected physical device with crash reports"]
    async fn lists_and_downloads_a_report_from_hardware() {
        let mut usbmuxd = UsbmuxdConnection::default().await.unwrap();
        let device = usbmuxd
            .get_devices()
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("no connected device");
        let provider: Arc<dyn IdeviceProvider> = Arc::new(
            device.to_provider(UsbmuxdAddr::default(), "devicehub-mask-crash-report-test"),
        );
        let result = list_crash_reports(provider.clone()).await.unwrap();
        let report = result
            .reports
            .iter()
            .filter(|report| report.size_bytes <= MAX_EXPORT_BYTES as u64)
            .min_by_key(|report| report.size_bytes)
            .expect("device returned no exportable crash report");
        let content = read_crash_report(provider.clone(), report.path.clone(), 4 * 1024)
            .await
            .unwrap();
        assert!(content.bytes_read <= 4 * 1024);
        assert_eq!(content.device_path, report.path);
        let data = download_crash_report(provider, report.path.clone())
            .await
            .unwrap();
        assert_eq!(data.len() as u64, report.size_bytes);
    }
}
