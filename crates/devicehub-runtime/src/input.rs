//! Device input transports and report construction.

mod dispatcher;
mod hid;

pub use dispatcher::{DeviceInputCommand, DeviceInputDispatcher};
pub use hid::{TouchContact, UniversalHidClient};
