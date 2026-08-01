//! Real-time status, WebCodecs media, and device-input transport.

mod control_lease;
mod input;
mod keymap;
mod transport;

pub use control_lease::BrowserControlLeases;
pub use transport::{BrowserAudioSlot, WebSocketConfig, WebSocketState, upgrade};
