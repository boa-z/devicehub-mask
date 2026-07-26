//! Device session lifecycle policy shared by host adapters.

mod commands;
mod heartbeat;
mod input;
mod lifecycle;
mod orientation;
mod router;
mod services;

pub use commands::{DeviceSessionCommand, SessionControlCommand};
pub use heartbeat::supervise_heartbeat;
pub use input::{
    ClipboardWriteFuture, DeviceClipboard, run_device_command_loop, run_management_command_loop,
};
pub use lifecycle::{SessionFailureAction, SessionRetry, SessionRetryPolicy};
pub use orientation::OrientationWatcher;
pub use router::DeviceSessionRouter;
pub use services::{
    DeviceServicePorts, LocationServicePort, RuntimeDeviceServicePorts, RuntimeServiceViews,
    RuntimeSessionServices,
};
