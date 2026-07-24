use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use idevice::IdeviceService;
use idevice::afc::opcode::AfcFopenMode;
use idevice::crashreportcopymobile::{CrashReportCopyMobileClient, flush_reports};
use idevice::provider::IdeviceProvider;
use tokio::io::AsyncWriteExt;

use crate::protocol::{
    CrashReportFormat, CrashReportKind, DeviceCrashReport, DeviceCrashReportContent,
    DeviceCrashReportList, DeviceCrashReportSummary,
};

const MAX_REPORTS: usize = 2_000;
const MAX_ENTRIES: usize = 5_000;
const MAX_DEPTH: usize = 8;
const MAX_EXPORT_BYTES: usize = 128 * 1024 * 1024;
pub(crate) const MAX_READ_BYTES: usize = 1024 * 1024;
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

pub async fn read(
    provider: Arc<dyn IdeviceProvider>,
    device_path: String,
    max_bytes: usize,
) -> Result<DeviceCrashReportContent, String> {
    validate_device_path(&device_path)?;
    if !(1..=MAX_READ_BYTES).contains(&max_bytes) {
        return Err(format!(
            "crash report read limit must be between 1 and {} bytes",
            MAX_READ_BYTES
        ));
    }
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
    let expected = info.size.min(max_bytes);
    let mut report = client
        .afc_client
        .open(device_path.clone(), AfcFopenMode::RdOnly)
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
    Ok(report_content(device_path, info.size, data))
}

pub async fn delete(provider: Arc<dyn IdeviceProvider>, device_path: String) -> Result<(), String> {
    validate_device_path(&device_path)?;
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
    client
        .afc_client
        .remove(&device_path)
        .await
        .map_err(|error| format!("unable to delete crash report: {error:?}"))
}

fn report_content(
    device_path: String,
    size_bytes: usize,
    data: Vec<u8>,
) -> DeviceCrashReportContent {
    let lossy_utf8 = std::str::from_utf8(&data).is_err();
    let content = String::from_utf8_lossy(&data).into_owned();
    let truncated = size_bytes > data.len();
    let summary = summarize_report(&device_path, &content, truncated);
    DeviceCrashReportContent {
        device_path,
        size_bytes: size_bytes as u64,
        bytes_read: data.len(),
        truncated,
        lossy_utf8,
        summary,
        content,
    }
}

fn summarize_report(
    device_path: &str,
    content: &str,
    source_truncated: bool,
) -> DeviceCrashReportSummary {
    let mut summary = empty_summary(source_truncated);
    let trimmed = content.trim_start_matches('\u{feff}').trim_start();
    let mut lines = trimmed.lines();
    let first_line = lines.next().unwrap_or_default().trim();
    if first_line.starts_with('{') {
        if let Ok(header) = serde_json::from_str::<serde_json::Value>(first_line) {
            summary.format = CrashReportFormat::IpsJson;
            apply_json_summary(&mut summary, &header);
            let remaining = trimmed
                .strip_prefix(first_line)
                .unwrap_or_default()
                .trim_start();
            if !remaining.is_empty() {
                if let Ok(details) = serde_json::from_str::<serde_json::Value>(remaining) {
                    apply_json_summary(&mut summary, &details);
                    summary.details_parsed = true;
                }
            } else {
                summary.details_parsed = !source_truncated;
            }
        } else if let Ok(details) = serde_json::from_str::<serde_json::Value>(trimmed) {
            summary.format = CrashReportFormat::IpsJson;
            apply_json_summary(&mut summary, &details);
            summary.details_parsed = true;
        }
    }
    if summary.format == CrashReportFormat::Unknown {
        apply_legacy_summary(&mut summary, trimmed);
    }
    summary.kind = classify_report(device_path, &summary);
    summary
}

fn empty_summary(source_truncated: bool) -> DeviceCrashReportSummary {
    DeviceCrashReportSummary {
        format: CrashReportFormat::Unknown,
        kind: CrashReportKind::Unknown,
        process_name: None,
        bundle_id: None,
        app_version: None,
        build_version: None,
        os_version: None,
        timestamp: None,
        bug_type: None,
        exception_type: None,
        exception_signal: None,
        termination_namespace: None,
        termination_code: None,
        faulting_thread: None,
        details_parsed: false,
        source_truncated,
    }
}

fn apply_json_summary(summary: &mut DeviceCrashReportSummary, value: &serde_json::Value) {
    summary.process_name = summary.process_name.take().or_else(|| {
        json_text(value, &["procName"], 128)
            .or_else(|| json_text(value, &["app_name"], 128))
            .or_else(|| json_text(value, &["name"], 128))
    });
    summary.bundle_id = summary.bundle_id.take().or_else(|| {
        json_bundle_id(value, &["bundleInfo", "CFBundleIdentifier"])
            .or_else(|| json_bundle_id(value, &["bundleID"]))
    });
    summary.app_version = summary.app_version.take().or_else(|| {
        json_text(value, &["bundleInfo", "CFBundleShortVersionString"], 64)
            .or_else(|| json_text(value, &["app_version"], 64))
    });
    summary.build_version = summary.build_version.take().or_else(|| {
        json_text(value, &["bundleInfo", "CFBundleVersion"], 64)
            .or_else(|| json_text(value, &["build_version"], 64))
    });
    summary.os_version = summary.os_version.take().or_else(|| {
        json_text(value, &["osVersion", "train"], 128)
            .or_else(|| json_text(value, &["os_version"], 128))
    });
    summary.timestamp = summary.timestamp.take().or_else(|| {
        json_text(value, &["captureTime"], 64).or_else(|| json_text(value, &["timestamp"], 64))
    });
    summary.bug_type = summary
        .bug_type
        .take()
        .or_else(|| json_scalar(value, &["bug_type"], 32));
    summary.exception_type = summary
        .exception_type
        .take()
        .or_else(|| json_text(value, &["exception", "type"], 128));
    summary.exception_signal = summary
        .exception_signal
        .take()
        .or_else(|| json_token(value, &["exception", "signal"], 64));
    summary.termination_namespace = summary
        .termination_namespace
        .take()
        .or_else(|| json_token(value, &["termination", "namespace"], 64));
    summary.termination_code = summary
        .termination_code
        .take()
        .or_else(|| json_scalar(value, &["termination", "code"], 64));
    summary.faulting_thread = summary.faulting_thread.or_else(|| {
        json_path(value, &["faultingThread"])
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
    });
}

fn apply_legacy_summary(summary: &mut DeviceCrashReportSummary, content: &str) {
    let mut recognized = false;
    for line in content.lines().take(2_048) {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "Process" => {
                summary.process_name =
                    normalize_text(value.split(" [").next().unwrap_or(value), 128);
                recognized |= summary.process_name.is_some();
            }
            "Identifier" => {
                summary.bundle_id = normalize_bundle_id(value);
                recognized |= summary.bundle_id.is_some();
            }
            "Version" => {
                let (version, build) = split_legacy_version(value);
                summary.app_version = version;
                summary.build_version = build;
                recognized |= summary.app_version.is_some() || summary.build_version.is_some();
            }
            "OS Version" => {
                summary.os_version = normalize_text(value, 128);
                recognized |= summary.os_version.is_some();
            }
            "Date/Time" => {
                summary.timestamp = normalize_text(value, 64);
                recognized |= summary.timestamp.is_some();
            }
            "Exception Type" => {
                summary.exception_type = normalize_text(value, 128);
                recognized |= summary.exception_type.is_some();
            }
            "Exception Signal" => {
                summary.exception_signal = normalize_text(value, 64);
                recognized |= summary.exception_signal.is_some();
            }
            "Termination Reason" => {
                let (namespace, code) = legacy_termination(value);
                summary.termination_namespace = namespace;
                summary.termination_code = code;
                recognized |=
                    summary.termination_namespace.is_some() || summary.termination_code.is_some();
            }
            "Triggered by Thread" | "Crashed Thread" => {
                summary.faulting_thread = value
                    .split_whitespace()
                    .next()
                    .and_then(|value| value.parse().ok());
                recognized |= summary.faulting_thread.is_some();
            }
            _ => {}
        }
    }
    if recognized {
        summary.format = CrashReportFormat::LegacyText;
        summary.details_parsed = true;
    }
}

fn json_path<'a>(value: &'a serde_json::Value, path: &[&str]) -> Option<&'a serde_json::Value> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))
}

fn json_text(value: &serde_json::Value, path: &[&str], max_chars: usize) -> Option<String> {
    json_path(value, path)
        .and_then(serde_json::Value::as_str)
        .and_then(|value| normalize_text(value, max_chars))
}

fn json_scalar(value: &serde_json::Value, path: &[&str], max_chars: usize) -> Option<String> {
    let value = json_path(value, path)?;
    if let Some(value) = value.as_str() {
        normalize_token(value, max_chars)
    } else if value.is_number() {
        normalize_token(&value.to_string(), max_chars)
    } else {
        None
    }
}

fn json_token(value: &serde_json::Value, path: &[&str], max_chars: usize) -> Option<String> {
    json_path(value, path)
        .and_then(serde_json::Value::as_str)
        .and_then(|value| normalize_token(value, max_chars))
}

fn json_bundle_id(value: &serde_json::Value, path: &[&str]) -> Option<String> {
    json_path(value, path)
        .and_then(serde_json::Value::as_str)
        .and_then(normalize_bundle_id)
}

fn normalize_text(value: &str, max_chars: usize) -> Option<String> {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    (!normalized.is_empty()
        && normalized.chars().count() <= max_chars
        && normalized
            .chars()
            .all(|character| !character.is_control() && !matches!(character, '/' | '\\')))
    .then_some(normalized)
}

fn normalize_token(value: &str, max_chars: usize) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.chars().count() <= max_chars
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        }))
    .then(|| value.to_string())
}

fn normalize_bundle_id(value: &str) -> Option<String> {
    let value = normalize_token(value, 255)?;
    (value.contains('.') && !value.starts_with('.') && !value.ends_with('.')).then_some(value)
}

fn split_legacy_version(value: &str) -> (Option<String>, Option<String>) {
    let Some((version, build)) = value.rsplit_once(" (") else {
        return (normalize_text(value, 64), None);
    };
    (
        normalize_text(version, 64),
        normalize_text(build.trim_end_matches(')'), 64),
    )
}

fn legacy_termination(value: &str) -> (Option<String>, Option<String>) {
    let mut namespace = None;
    let mut code = None;
    for part in value.split(',') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix("Namespace ") {
            namespace = normalize_token(value, 64);
        } else if let Some(value) = part.strip_prefix("Code ") {
            code = normalize_token(value.split_whitespace().next().unwrap_or_default(), 64);
        }
    }
    (namespace, code)
}

fn classify_report(device_path: &str, summary: &DeviceCrashReportSummary) -> CrashReportKind {
    let path = device_path.to_ascii_lowercase();
    let termination = summary
        .termination_namespace
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if path.contains("panic")
        || summary
            .process_name
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case("panic"))
    {
        CrashReportKind::Panic
    } else if summary.bug_type.as_deref() == Some("298") || path.contains("jetsam") {
        CrashReportKind::Jetsam
    } else if termination.contains("watchdog") || termination.contains("frontboard") {
        CrashReportKind::Watchdog
    } else if summary.exception_type.is_some()
        || summary.exception_signal.is_some()
        || matches!(summary.bug_type.as_deref(), Some("109" | "309"))
    {
        CrashReportKind::AppCrash
    } else if summary.details_parsed {
        CrashReportKind::Other
    } else {
        CrashReportKind::Unknown
    }
}

fn child_path(directory: &str, name: &str) -> String {
    if directory == "/" {
        format!("/{name}")
    } else {
        format!("{directory}/{name}")
    }
}

pub(crate) fn validate_device_path(path: &str) -> Result<(), String> {
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

    #[test]
    fn bounded_report_content_marks_truncation_and_invalid_utf8() {
        let report = report_content("/Report.ips".into(), 64, vec![b'a', 0xff, b'b']);
        assert_eq!(report.size_bytes, 64);
        assert_eq!(report.bytes_read, 3);
        assert!(report.truncated);
        assert!(report.lossy_utf8);
        assert_eq!(report.content, "a\u{fffd}b");
        assert_eq!(report.summary.format, CrashReportFormat::Unknown);
        assert!(report.summary.source_truncated);
        assert!(!report.summary.details_parsed);
    }

    #[test]
    fn summarizes_modern_ips_without_exposing_private_payloads() {
        let content = concat!(
            r#"{"app_name":"Game","timestamp":"2026-07-25 01:02:03.00 +0800","app_version":"1.2","build_version":"34","bundleID":"com.example.game","bug_type":"309","incident_id":"PRIVATE-INCIDENT"}"#,
            "\n",
            r#"{"procName":"Game","procPath":"/private/var/containers/Game","bundleInfo":{"CFBundleIdentifier":"com.example.game","CFBundleShortVersionString":"1.2","CFBundleVersion":"34"},"osVersion":{"train":"iPhone OS 27.0"},"exception":{"type":"EXC_BAD_ACCESS","signal":"SIGSEGV"},"termination":{"namespace":"SIGNAL","code":11,"indicator":"PRIVATE PATH"},"faultingThread":3,"crashReporterKey":"PRIVATE-KEY","threads":[{"frames":[{"symbol":"PRIVATE-SYMBOL"}]}],"usedImages":[{"path":"/private/image"}]}"#,
        );
        let report = report_content(
            "/Game-2026-07-25.ips".into(),
            content.len(),
            content.as_bytes().to_vec(),
        );

        assert_eq!(report.summary.format, CrashReportFormat::IpsJson);
        assert_eq!(report.summary.kind, CrashReportKind::AppCrash);
        assert_eq!(report.summary.process_name.as_deref(), Some("Game"));
        assert_eq!(
            report.summary.bundle_id.as_deref(),
            Some("com.example.game")
        );
        assert_eq!(report.summary.app_version.as_deref(), Some("1.2"));
        assert_eq!(report.summary.build_version.as_deref(), Some("34"));
        assert_eq!(report.summary.os_version.as_deref(), Some("iPhone OS 27.0"));
        assert_eq!(report.summary.bug_type.as_deref(), Some("309"));
        assert_eq!(
            report.summary.exception_type.as_deref(),
            Some("EXC_BAD_ACCESS")
        );
        assert_eq!(report.summary.exception_signal.as_deref(), Some("SIGSEGV"));
        assert_eq!(
            report.summary.termination_namespace.as_deref(),
            Some("SIGNAL")
        );
        assert_eq!(report.summary.termination_code.as_deref(), Some("11"));
        assert_eq!(report.summary.faulting_thread, Some(3));
        assert!(report.summary.details_parsed);
        let serialized = serde_json::to_string(&report.summary).unwrap();
        for private in [
            "/private/var/containers/Game",
            "PRIVATE-INCIDENT",
            "PRIVATE PATH",
            "PRIVATE-KEY",
            "PRIVATE-SYMBOL",
            "/private/image",
        ] {
            assert!(!serialized.contains(private), "summary exposed {private:?}");
        }
    }

    #[test]
    fn summarizes_legacy_crash_headers_only() {
        let content = concat!(
            "Process: Game [123]\n",
            "Path: /private/var/containers/Game\n",
            "Identifier: com.example.game\n",
            "Version: 1.2 (34)\n",
            "OS Version: iPhone OS 27.0 (24A123)\n",
            "Date/Time: 2026-07-25 01:02:03.000 +0800\n",
            "Exception Type: EXC_CRASH (SIGABRT)\n",
            "Exception Signal: SIGABRT\n",
            "Termination Reason: Namespace SIGNAL, Code 0x6, private reason\n",
            "Triggered by Thread: 7\n",
            "Thread 7 Crashed:\n0 private stack frame\n",
        );
        let summary = summarize_report("/Game.crash", content, false);
        assert_eq!(summary.format, CrashReportFormat::LegacyText);
        assert_eq!(summary.kind, CrashReportKind::AppCrash);
        assert_eq!(summary.process_name.as_deref(), Some("Game"));
        assert_eq!(summary.bundle_id.as_deref(), Some("com.example.game"));
        assert_eq!(summary.app_version.as_deref(), Some("1.2"));
        assert_eq!(summary.build_version.as_deref(), Some("34"));
        assert_eq!(summary.termination_namespace.as_deref(), Some("SIGNAL"));
        assert_eq!(summary.termination_code.as_deref(), Some("0x6"));
        assert_eq!(summary.faulting_thread, Some(7));
        let serialized = serde_json::to_string(&summary).unwrap();
        assert!(!serialized.contains("/private/"));
        assert!(!serialized.contains("private reason"));
        assert!(!serialized.contains("private stack frame"));
    }

    #[test]
    fn classifies_bounded_report_kinds_and_incomplete_headers() {
        let jetsam = summarize_report(
            "/JetsamEvent-2026-07-25.ips",
            r#"{"bug_type":"298","procName":"Game"}"#,
            false,
        );
        assert_eq!(jetsam.kind, CrashReportKind::Jetsam);

        let watchdog = summarize_report(
            "/Game.ips",
            r#"{"procName":"Game","termination":{"namespace":"FRONTBOARD"}}"#,
            false,
        );
        assert_eq!(watchdog.kind, CrashReportKind::Watchdog);

        let panic = summarize_report("/panic-full-2026-07-25.ips", r#"{"bug_type":"210"}"#, false);
        assert_eq!(panic.kind, CrashReportKind::Panic);

        let truncated = summarize_report(
            "/Game.ips",
            r#"{"app_name":"Game","bundleID":"com.example.game","bug_type":"309"}"#,
            true,
        );
        assert_eq!(truncated.format, CrashReportFormat::IpsJson);
        assert_eq!(truncated.kind, CrashReportKind::AppCrash);
        assert!(truncated.source_truncated);
        assert!(!truncated.details_parsed);

        let unknown = summarize_report("/Unknown.txt", "not a crash report", false);
        assert_eq!(unknown.format, CrashReportFormat::Unknown);
        assert_eq!(unknown.kind, CrashReportKind::Unknown);
    }

    #[test]
    fn rejects_oversized_or_unsafe_summary_fields() {
        let oversized_name = "x".repeat(129);
        let content = serde_json::json!({
            "procName": oversized_name,
            "bundleID": "invalid bundle/id",
            "exception": { "signal": "SIG SEGV" },
            "termination": { "namespace": "SIGNAL/PRIVATE", "code": "0x6 private" },
        })
        .to_string();
        let summary = summarize_report("/Unknown.ips", &content, false);
        assert!(summary.process_name.is_none());
        assert!(summary.bundle_id.is_none());
        assert!(summary.exception_signal.is_none());
        assert!(summary.termination_namespace.is_none());
        assert!(summary.termination_code.is_none());
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
        let content = read(provider.clone(), report.path.clone(), 4 * 1024)
            .await
            .unwrap();
        assert!(content.bytes_read <= 4 * 1024);
        assert_eq!(content.device_path, report.path);
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
