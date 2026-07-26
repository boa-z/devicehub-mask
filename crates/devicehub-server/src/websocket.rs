//! Real-time status, WebCodecs media, and device-input transport.

mod input;
mod transport;

pub use transport::{WebSocketConfig, WebSocketState, upgrade};
