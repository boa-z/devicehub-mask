//! Device session lifecycle policy shared by host adapters.

mod commands;
mod diagnostics;
mod heartbeat;
mod input;
mod lifecycle;
mod orientation;
mod router;
mod services;

pub use commands::{DeviceSessionCommand, SessionControlCommand};
pub use diagnostics::{DiagnosticDumpSinkFactory, DiagnosticDumpSinkFuture, SessionDiagnostics};
pub use heartbeat::supervise_heartbeat;
pub use input::{
    ClipboardWriteFuture, DeviceClipboard, connect_device_input, run_device_command_loop,
    run_management_command_loop,
};
pub use lifecycle::{SessionFailureAction, SessionRetry, SessionRetryPolicy};
pub use orientation::OrientationWatcher;
pub use router::{DeviceManagementBootstrap, DeviceManagementSession, DeviceSessionRouter};
pub use services::{
    DeviceServicePorts, LocationServicePort, RuntimeConnectedSessionServices,
    RuntimeHostServiceViews, RuntimeServiceViews, RuntimeSessionHostAdapters,
    RuntimeSessionServices,
};
