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
pub(crate) use heartbeat::supervise_heartbeat;
pub(crate) use input::{
    ClipboardWriteFuture, DeviceClipboard, connect_device_input, run_device_command_loop,
    run_management_command_loop,
};
pub(crate) use lifecycle::{SessionFailureAction, SessionRetryPolicy};
pub(crate) use manager::SessionManagerViews;
pub use manager::{RuntimeHostAdapters, StartedRuntime, start_runtime};
pub(crate) use orientation::OrientationWatcher;
pub(crate) use router::{DeviceManagementBootstrap, DeviceSessionRouter};
pub(crate) use runner::run_connected_session;
pub(crate) use runner::{ConnectedSessionHost, ConnectedSessionMedia, ConnectedSessionViews};
pub use services::RuntimeSessionHostAdapters;
use services::RuntimeSessionServices;
pub(crate) use services::{RuntimeHostServiceViews, RuntimeServiceViews};
pub(crate) use trust::PairingCredentialStore;
pub(crate) use trust::{forget_device, pair_device};
