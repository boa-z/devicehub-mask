//! Normalized, bounded device log observations shared by every host adapter.

use serde::Serialize;

/// Maximum number of entries returned by one device-log observation.
pub const MAX_DEVICE_LOG_BATCH_ENTRIES: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceLogSource {
    Unified,
    Syslog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceLogLevel {
    Notice,
    Info,
    Debug,
    Error,
    Fault,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceLogEntry {
    pub sequence: u64,
    pub received_at_ms: u64,
    pub message: String,
    pub level: Option<DeviceLogLevel>,
    pub process: Option<String>,
    pub pid: Option<u32>,
    pub subsystem: Option<String>,
    pub category: Option<String>,
    pub filename: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceLogBatch {
    pub entries: Vec<DeviceLogEntry>,
    pub oldest_sequence: Option<u64>,
    pub latest_sequence: Option<u64>,
    pub cursor_lagged: bool,
    pub has_more: bool,
    pub streaming: bool,
    pub source: Option<DeviceLogSource>,
}
