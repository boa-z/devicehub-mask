//! Device session lifecycle policy shared by host adapters.

mod commands;
mod diagnostics;
mod heartbeat;
mod input;
mod lifecycle;
mod manager;
mod orientation;
mod router;
mod runner;
mod services;
mod trust;

pub use commands::{DeviceSessionCommand, SessionCommandSlot, SessionControlCommand};
pub use diagnostics::{DiagnosticDumpSinkFactory, DiagnosticDumpSinkFuture, SessionDiagnostics};
pub use heartbeat::supervise_heartbeat;
pub use input::{
    ClipboardWriteFuture, DeviceClipboard, connect_device_input, run_device_command_loop,
    run_management_command_loop,
};
pub use lifecycle::{SessionFailureAction, SessionRetry, SessionRetryPolicy};
pub use manager::{SessionManagerHost, SessionManagerViews, run_session_manager};
pub use orientation::OrientationWatcher;
pub use router::{DeviceManagementBootstrap, DeviceManagementSession, DeviceSessionRouter};
pub use runner::{
    ConnectedSessionHost, ConnectedSessionMedia, ConnectedSessionViews, run_connected_session,
};
use services::RuntimeSessionServices;
pub use services::{
    DeviceServicePorts, LocationServicePort, RuntimeHostServiceViews, RuntimeServiceViews,
    RuntimeSessionHostAdapters,
};
pub use trust::{PairingCredentialStore, forget_device, pair_device};
