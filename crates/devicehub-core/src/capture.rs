//! Bounded capture state exposed to desktop, web, and headless clients.

use std::sync::{Arc, Mutex};

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

#[derive(Clone, Default)]
pub struct NetworkCaptureSlot(Arc<Mutex<NetworkCaptureStatus>>);

impl NetworkCaptureSlot {
    pub fn set(&self, status: NetworkCaptureStatus) {
        *self.0.lock().expect("network capture status lock poisoned") = status;
    }

    pub fn get(&self) -> NetworkCaptureStatus {
        self.0
            .lock()
            .expect("network capture status lock poisoned")
            .clone()
    }

    pub fn reset(&self) {
        self.set(NetworkCaptureStatus::default());
    }
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

#[derive(Clone, Default)]
pub struct BluetoothCaptureSlot(Arc<Mutex<BluetoothCaptureStatus>>);

impl BluetoothCaptureSlot {
    pub fn set(&self, status: BluetoothCaptureStatus) {
        *self
            .0
            .lock()
            .expect("Bluetooth capture status lock poisoned") = status;
    }

    pub fn get(&self) -> BluetoothCaptureStatus {
        self.0
            .lock()
            .expect("Bluetooth capture status lock poisoned")
            .clone()
    }

    pub fn reset(&self) {
        self.set(BluetoothCaptureStatus::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloned_capture_slots_share_normalized_status() {
        let network = NetworkCaptureSlot::default();
        let network_reader = network.clone();
        network.set(NetworkCaptureStatus {
            state: NetworkCaptureState::Capturing,
            packet_count: 7,
            ..NetworkCaptureStatus::default()
        });
        assert_eq!(network_reader.get().packet_count, 7);
        network.reset();
        assert_eq!(network_reader.get(), NetworkCaptureStatus::default());

        let bluetooth = BluetoothCaptureSlot::default();
        let bluetooth_reader = bluetooth.clone();
        bluetooth.set(BluetoothCaptureStatus {
            state: BluetoothCaptureState::Capturing,
            packet_count: 3,
            ..BluetoothCaptureStatus::default()
        });
        assert_eq!(bluetooth_reader.get().packet_count, 3);
    }
}
