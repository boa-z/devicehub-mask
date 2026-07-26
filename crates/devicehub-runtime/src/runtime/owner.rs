//! Dedicated owner thread and executor lifecycle.
//!
//! CoreDevice sessions include non-`Send` DVT channels and deeply nested XPC
//! decoding. One deliberately sized thread and one `LocalSet` therefore own
//! the complete manager lifecycle across desktop and future headless hosts.

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::thread::JoinHandle;

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::SessionControlCommand;

const OWNER_THREAD_NAME: &str = "devicehub-coredevice";
pub const OWNER_THREAD_STACK_BYTES: usize = 16 * 1024 * 1024;

/// Non-`Send` session-manager future created after entering the owner thread.
pub(crate) type CoreRuntimeFuture = Pin<Box<dyn Future<Output = ()> + 'static>>;

/// Owns the session manager's control channel and dedicated executor thread.
pub struct CoreRuntime {
    control: UnboundedSender<SessionControlCommand>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl CoreRuntime {
    /// Create host-facing state and an owner-thread task around one shared
    /// control channel. `task` is invoked on the new thread so its future may
    /// retain non-`Send` CoreDevice clients safely inside the `LocalSet`.
    pub(crate) fn start<State, Build, Task>(build: Build) -> Result<(Self, State), String>
    where
        Build: FnOnce(
            UnboundedSender<SessionControlCommand>,
            UnboundedReceiver<SessionControlCommand>,
        ) -> (State, Task),
        Task: FnOnce() -> CoreRuntimeFuture + Send + 'static,
    {
        let (control, control_rx) = mpsc::unbounded_channel();
        let (state, task) = build(control.clone(), control_rx);
        let thread = std::thread::Builder::new()
            .name(OWNER_THREAD_NAME.into())
            .stack_size(OWNER_THREAD_STACK_BYTES)
            .spawn(move || run_owner(task))
            .map_err(|error| format!("cannot start CoreDevice thread: {error}"))?;
        Ok((
            Self {
                control,
                thread: Mutex::new(Some(thread)),
            },
            state,
        ))
    }

    pub fn request_shutdown(&self) {
        let _ = self.control.send(SessionControlCommand::Quit);
    }

    /// Request shutdown and join the owner exactly once.
    pub fn stop(&self) {
        self.request_shutdown();
        if let Some(thread) = self.thread.lock().unwrap().take() {
            let _ = thread.join();
        }
    }

    #[cfg(test)]
    fn is_stopped(&self) -> bool {
        self.thread.lock().unwrap().is_none()
    }
}

impl Drop for CoreRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_owner<Task>(task: Task)
where
    Task: FnOnce() -> CoreRuntimeFuture,
{
    tracing::info!(
        stack_bytes = OWNER_THREAD_STACK_BYTES,
        "CoreDevice owner thread started"
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build CoreDevice runtime");
    let local = tokio::task::LocalSet::new();
    runtime.block_on(local.run_until(task()));
    tracing::info!("CoreDevice owner thread stopped");
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{CoreRuntime, CoreRuntimeFuture};
    use crate::SessionControlCommand;

    #[test]
    fn shutdown_is_idempotent_and_joins_the_owner() {
        let stopped = Arc::new(AtomicBool::new(false));
        let owner_stopped = stopped.clone();
        let (runtime, ()) = CoreRuntime::start(|_control, mut receiver| {
            let task = move || -> CoreRuntimeFuture {
                Box::pin(async move {
                    while let Some(command) = receiver.recv().await {
                        if matches!(command, SessionControlCommand::Quit) {
                            break;
                        }
                    }
                    owner_stopped.store(true, Ordering::Release);
                })
            };
            ((), task)
        })
        .expect("start runtime");

        runtime.stop();
        runtime.stop();

        assert!(stopped.load(Ordering::Acquire));
        assert!(runtime.is_stopped());
    }
}
