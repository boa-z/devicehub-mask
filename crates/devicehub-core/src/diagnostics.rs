//! Bounded crash-report metadata exposed to diagnostics adapters.

use serde::Serialize;

mod crash_reports;

pub use crash_reports::{build_crash_report_content, validate_crash_report_path};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceBackupState {
    #[default]
    Idle,
    Starting,
    BackingUp,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DeviceBackupStatus {
    pub state: DeviceBackupState,
    pub files_received: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub progress_percent: Option<f64>,
    pub elapsed_ms: u64,
    pub full: bool,
    pub destination_name: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SysdiagnoseState {
    #[default]
    Idle,
    Starting,
    Collecting,
    Downloading,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SysdiagnoseStatus {
    pub state: SysdiagnoseState,
    pub bytes_written: u64,
    pub bytes_total: u64,
    pub progress_percent: Option<f64>,
    pub elapsed_ms: u64,
    pub destination_name: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogArchiveState {
    #[default]
    Idle,
    Starting,
    Exporting,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LogArchiveStatus {
    pub state: LogArchiveState,
    pub bytes_written: u64,
    pub elapsed_ms: u64,
    pub destination_name: Option<String>,
    pub age_limit_hours: Option<u16>,
    pub error: Option<String>,
}

/// Stable, non-reversible identifier used to correlate device logs without
/// exposing the device UDID.
pub fn device_id_fingerprint(udid: &str) -> String {
    let hash = udid.as_bytes().iter().fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(*byte)).wrapping_mul(0x0100_0193)
    });
    format!("{hash:08x}")
}

#[cfg(test)]
mod tests {
    use super::device_id_fingerprint;

    #[test]
    fn device_fingerprint_is_stable_and_does_not_expose_the_udid() {
        let udid = "00008110-0011223344556677";
        let fingerprint = device_id_fingerprint(udid);
        assert_eq!(fingerprint, device_id_fingerprint(udid));
        assert_eq!(fingerprint.len(), 8);
        assert!(!fingerprint.contains(udid));
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceCrashReport {
    pub path: String,
    pub name: String,
    pub size_bytes: u64,
    pub modified: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceCrashReportList {
    pub reports: Vec<DeviceCrashReport>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceCrashReportContent {
    pub device_path: String,
    pub size_bytes: u64,
    pub bytes_read: usize,
    pub truncated: bool,
    pub lossy_utf8: bool,
    pub summary: DeviceCrashReportSummary,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CrashReportFormat {
    IpsJson,
    LegacyText,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CrashReportKind {
    AppCrash,
    Jetsam,
    Watchdog,
    Panic,
    Other,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceCrashReportSummary {
    pub format: CrashReportFormat,
    pub kind: CrashReportKind,
    pub process_name: Option<String>,
    pub bundle_id: Option<String>,
    pub app_version: Option<String>,
    pub build_version: Option<String>,
    pub os_version: Option<String>,
    pub timestamp: Option<String>,
    pub bug_type: Option<String>,
    pub exception_type: Option<String>,
    pub exception_signal: Option<String>,
    pub termination_namespace: Option<String>,
    pub termination_code: Option<String>,
    pub faulting_thread: Option<u32>,
    pub details_parsed: bool,
    pub source_truncated: bool,
}
