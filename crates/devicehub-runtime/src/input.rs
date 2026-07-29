//! Device input transports and report construction.

mod dispatcher;
mod hid;

pub(crate) use dispatcher::DeviceInputDispatcher;
pub(crate) use hid::capture_connected_services;
