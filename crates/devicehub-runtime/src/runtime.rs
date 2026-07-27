//! Dedicated runtime ownership and state graph for CoreDevice sessions.

mod owner;
mod state;

pub(crate) use owner::CoreRuntimeFuture;
pub use owner::{CoreRuntime, OWNER_THREAD_STACK_BYTES};
pub(crate) use state::{CoreRuntimeState, DeviceSessionState};
