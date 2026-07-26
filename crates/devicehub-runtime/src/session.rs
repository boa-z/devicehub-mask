//! Device session lifecycle policy shared by host adapters.

mod heartbeat;
mod lifecycle;
mod orientation;

pub use heartbeat::supervise_heartbeat;
pub use lifecycle::{SessionFailureAction, SessionRetry, SessionRetryPolicy};
pub use orientation::OrientationWatcher;
