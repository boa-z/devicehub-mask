use super::{
    CrashReportFormat, CrashReportKind, DeviceCrashReportContent, DeviceCrashReportSummary,
};

/// Builds the bounded public representation of crash report bytes read from a device.
pub fn build_crash_report_content(
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

/// Validates an absolute path inside the crash report service root.
pub fn validate_crash_report_path(path: &str) -> Result<(), String> {
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

fn summarize_report(
    device_path: &str,
    content: &str,
    source_truncated: bool,
) -> DeviceCrashReportSummary {
    let mut summary = empty_summary(source_truncated);
    let trimmed = content.trim_start_matches('\u{feff}').trim_start();
    let first_line = trimmed.lines().next().unwrap_or_default().trim();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crash_report_paths_are_absolute_and_cannot_traverse() {
        assert!(validate_crash_report_path("/JetsamEvent-2026-07-24.ips").is_ok());
        assert!(validate_crash_report_path("/Retired/App-2026-07-24.ips").is_ok());
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
                validate_crash_report_path(invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
    }

    #[test]
    fn bounded_report_content_marks_truncation_and_invalid_utf8() {
        let report = build_crash_report_content("/Report.ips".into(), 64, vec![b'a', 0xff, b'b']);
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
        let report = build_crash_report_content(
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
        let summary = build_crash_report_content(
            "/Game.crash".into(),
            content.len(),
            content.as_bytes().to_vec(),
        )
        .summary;
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
        let summary = |path: &str, content: &str, size: usize| {
            build_crash_report_content(path.into(), size, content.as_bytes().to_vec()).summary
        };
        assert_eq!(
            summary(
                "/JetsamEvent-2026-07-25.ips",
                r#"{"bug_type":"298","procName":"Game"}"#,
                40,
            )
            .kind,
            CrashReportKind::Jetsam
        );
        assert_eq!(
            summary(
                "/Game.ips",
                r#"{"procName":"Game","termination":{"namespace":"FRONTBOARD"}}"#,
                65,
            )
            .kind,
            CrashReportKind::Watchdog
        );
        assert_eq!(
            summary("/panic-full.ips", r#"{"bug_type":"210"}"#, 18).kind,
            CrashReportKind::Panic
        );
        let truncated = summary(
            "/Game.ips",
            r#"{"app_name":"Game","bundleID":"com.example.game","bug_type":"309"}"#,
            1_024,
        );
        assert_eq!(truncated.format, CrashReportFormat::IpsJson);
        assert_eq!(truncated.kind, CrashReportKind::AppCrash);
        assert!(truncated.source_truncated);
        assert!(!truncated.details_parsed);
        let unknown = summary("/Unknown.txt", "not a crash report", 18);
        assert_eq!(unknown.format, CrashReportFormat::Unknown);
        assert_eq!(unknown.kind, CrashReportKind::Unknown);
    }

    #[test]
    fn rejects_oversized_or_unsafe_summary_fields() {
        let content = serde_json::json!({
            "procName": "x".repeat(129),
            "bundleID": "invalid bundle/id",
            "exception": { "signal": "SIG SEGV" },
            "termination": { "namespace": "SIGNAL/PRIVATE", "code": "0x6 private" },
        })
        .to_string();
        let summary =
            build_crash_report_content("/Unknown.ips".into(), content.len(), content.into_bytes())
                .summary;
        assert!(summary.process_name.is_none());
        assert!(summary.bundle_id.is_none());
        assert!(summary.exception_signal.is_none());
        assert!(summary.termination_namespace.is_none());
        assert!(summary.termination_code.is_none());
    }
}
