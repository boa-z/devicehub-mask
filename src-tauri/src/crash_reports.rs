use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use idevice::IdeviceService;
use idevice::afc::opcode::AfcFopenMode;
use idevice::crashreportcopymobile::{CrashReportCopyMobileClient, flush_reports};
use idevice::provider::IdeviceProvider;
use tokio::io::AsyncWriteExt;

use crate::protocol::{DeviceCrashReport, DeviceCrashReportList};

const MAX_REPORTS: usize = 2_000;
const MAX_ENTRIES: usize = 5_000;
const MAX_DEPTH: usize = 8;
const MAX_EXPORT_BYTES: usize = 128 * 1024 * 1024;
const SERVICE_TIMEOUT: Duration = Duration::from_secs(5);

pub async fn list(provider: Arc<dyn IdeviceProvider>) -> Result<DeviceCrashReportList, String> {
    match tokio::time::timeout(Duration::from_secs(3), flush_reports(provider.as_ref())).await {
        Ok(Ok(())) => tracing::debug!("device crash reports flushed"),
        Ok(Err(error)) => tracing::warn!("device crash report flush unavailable: {error:?}"),
        Err(_) => tracing::warn!("device crash report flush timed out"),
    }

    let mut client = tokio::time::timeout(
        SERVICE_TIMEOUT,
        CrashReportCopyMobileClient::connect(provider.as_ref()),
    )
    .await
    .map_err(|_| "crash report service connection timed out".to_string())?
    .map_err(|error| format!("unable to connect to crash report service: {error:?}"))?;

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

pub async fn export(
    provider: Arc<dyn IdeviceProvider>,
    device_path: String,
    destination: &Path,
) -> Result<u64, String> {
    validate_device_path(&device_path)?;
    validate_destination(destination).await?;
    let mut client = tokio::time::timeout(
        SERVICE_TIMEOUT,
        CrashReportCopyMobileClient::connect(provider.as_ref()),
    )
    .await
    .map_err(|_| "crash report service connection timed out".to_string())?
    .map_err(|error| format!("unable to connect to crash report service: {error:?}"))?;
    let info = client
        .afc_client
        .get_file_info(&device_path)
        .await
        .map_err(|error| format!("unable to inspect crash report: {error:?}"))?;
    if info.st_ifmt != "S_IFREG" {
        return Err("selected crash report is not a regular file".to_string());
    }
    if info.size > MAX_EXPORT_BYTES {
        return Err(format!(
            "crash report exceeds the {} MiB export limit",
            MAX_EXPORT_BYTES / 1024 / 1024
        ));
    }
    let mut report = client
        .afc_client
        .open(device_path, AfcFopenMode::RdOnly)
        .await
        .map_err(|error| format!("unable to open crash report: {error:?}"))?;
    let data = report
        .read_n(info.size)
        .await
        .map_err(|error| format!("unable to read crash report: {error:?}"))?;
    report
        .close()
        .await
        .map_err(|error| format!("unable to close crash report: {error:?}"))?;
    if data.len() != info.size {
        return Err("crash report changed while it was being read".to_string());
    }

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

fn child_path(directory: &str, name: &str) -> String {
    if directory == "/" {
        format!("/{name}")
    } else {
        format!("{directory}/{name}")
    }
}

fn validate_device_path(path: &str) -> Result<(), String> {
    if path.len() > 1_024
        || !path.starts_with('/')
        || path.ends_with('/')
        || path.contains(['\\', '\0'])
        || path.split('/').skip(1).any(|part| {
            part.is_empty() || part == "." || part == ".." || part.chars().any(char::is_control)
        })
    {
        return Err("invalid crash report path".to_string());
    }
    Ok(())
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
    use idevice::usbmuxd::{UsbmuxdAddr, UsbmuxdConnection};

    #[test]
    fn crash_report_paths_are_absolute_and_cannot_traverse() {
        assert!(validate_device_path("/JetsamEvent-2026-07-24.ips").is_ok());
        assert!(validate_device_path("/Retired/App-2026-07-24.ips").is_ok());
        for invalid in [
            "relative.ips",
            "/../private/file",
            "/Retired/../../file",
            "/Retired/",
            "/Retired//file",
            "/Retired\\file",
            "/Retired/\0file",
        ] {
            assert!(
                validate_device_path(invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
    }

    #[test]
    fn joins_root_and_nested_report_paths() {
        assert_eq!(child_path("/", "Report.ips"), "/Report.ips");
        assert_eq!(child_path("/Retired", "Report.ips"), "/Retired/Report.ips");
    }

    #[tokio::test]
    #[ignore = "requires a connected physical device with crash reports"]
    async fn lists_and_exports_a_report_from_hardware() {
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
        let result = list(provider.clone()).await.unwrap();
        let report = result
            .reports
            .iter()
            .filter(|report| report.size_bytes <= MAX_EXPORT_BYTES as u64)
            .min_by_key(|report| report.size_bytes)
            .expect("device returned no exportable crash report");
        let destination = std::env::temp_dir().join(format!(
            "devicehub-mask-crash-report-{}.tmp",
            uuid::Uuid::new_v4()
        ));
        let written = export(provider, report.path.clone(), &destination)
            .await
            .unwrap();
        assert_eq!(
            written,
            tokio::fs::metadata(&destination).await.unwrap().len()
        );
        tokio::fs::remove_file(destination).await.unwrap();
    }
}
