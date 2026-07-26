//! Bounded, user-initiated device capture services.
//!
//! Runtime modules own device protocols, capture policy, PCAP encoding, and
//! lifecycle supervision. Hosts inject destination validation and durable
//! output through [`CaptureFileIo`].

mod bluetooth;
mod network;
mod output;

pub use bluetooth::{
    BluetoothCaptureCommand, BluetoothCaptureSlot, BluetoothCaptureTransport,
    MAX_BLUETOOTH_CAPTURE_DURATION_SECONDS, MIN_BLUETOOTH_CAPTURE_DURATION_SECONDS,
    serve as serve_bluetooth_capture, validate_duration as validate_bluetooth_capture_duration,
};
pub use devicehub_core::{
    BluetoothCaptureState, BluetoothCaptureStatus, BluetoothCaptureStopReason, NetworkCaptureState,
    NetworkCaptureStatus, NetworkCaptureStopReason,
};
pub use network::{
    MAX_NETWORK_CAPTURE_DURATION_SECONDS, MIN_NETWORK_CAPTURE_DURATION_SECONDS,
    NetworkCaptureCommand, NetworkCaptureSlot, NetworkCaptureTransport,
    serve as serve_network_capture, validate_duration as validate_network_capture_duration,
};
pub use output::{CaptureFileFuture, CaptureFileIo, CaptureFileKind, CaptureFileWriter};
