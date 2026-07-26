//! Device input transports and report construction.

mod dispatcher;
mod hid;

pub use dispatcher::{DeviceInputCommand, DeviceInputDispatcher};
pub use hid::TouchContact;
pub(crate) use hid::{UniversalHidClient, capture_connected_services};
