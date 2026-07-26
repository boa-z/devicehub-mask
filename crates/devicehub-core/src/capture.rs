//! Bounded capture state exposed to desktop, web, and headless clients.

use serde::Serialize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkCaptureState {
    #[default]
    Idle,
    Starting,
    Capturing,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkCaptureStopReason {
    UserRequested,
    DurationLimit,
    SizeLimit,
    SessionEnded,
    StreamEnded,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct NetworkCaptureStatus {
    pub state: NetworkCaptureState,
    pub process_id: Option<u32>,
    pub packet_count: u64,
    pub filtered_packet_count: u64,
    pub bytes_written: u64,
    pub elapsed_ms: u64,
    pub duration_seconds: Option<u64>,
    pub stop_reason: Option<NetworkCaptureStopReason>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BluetoothCaptureState {
    #[default]
    Idle,
    Starting,
    Capturing,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BluetoothCaptureStopReason {
    UserRequested,
    DurationLimit,
    SizeLimit,
    SessionEnded,
    StreamEnded,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct BluetoothCaptureStatus {
    pub state: BluetoothCaptureState,
    pub packet_count: u64,
    pub bytes_written: u64,
    pub elapsed_ms: u64,
    pub duration_seconds: Option<u64>,
    pub stop_reason: Option<BluetoothCaptureStopReason>,
    pub error: Option<String>,
}
