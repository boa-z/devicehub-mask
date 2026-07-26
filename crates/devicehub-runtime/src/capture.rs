//! Bounded, user-initiated device capture services.
//!
//! Runtime modules own device protocols, capture policy, PCAP encoding, and
//! lifecycle supervision. Hosts inject destination validation and durable
//! output through [`CaptureFileIo`].

mod bluetooth;
mod network;
mod output;

pub use bluetooth::{
    BluetoothCaptureCommand, BluetoothCaptureSlot, MAX_BLUETOOTH_CAPTURE_DURATION_SECONDS,
    MIN_BLUETOOTH_CAPTURE_DURATION_SECONDS,
    validate_duration as validate_bluetooth_capture_duration,
};
pub(crate) use bluetooth::{BluetoothCaptureTransport, serve as serve_bluetooth_capture};
pub use devicehub_core::{
    BluetoothCaptureState, BluetoothCaptureStatus, BluetoothCaptureStopReason, NetworkCaptureState,
    NetworkCaptureStatus, NetworkCaptureStopReason,
};
pub use network::{
    MAX_NETWORK_CAPTURE_DURATION_SECONDS, MIN_NETWORK_CAPTURE_DURATION_SECONDS,
    NetworkCaptureCommand, NetworkCaptureSlot,
    validate_duration as validate_network_capture_duration,
};
pub(crate) use network::{NetworkCaptureTransport, serve as serve_network_capture};
pub use output::{CaptureFileFuture, CaptureFileIo, CaptureFileKind, CaptureFileWriter};
