//! Device input transports and report construction.

mod dispatcher;
mod hid;

pub(crate) use dispatcher::DeviceInputDispatcher;
pub(crate) use hid::{UniversalHidClient, capture_connected_services};
