//! Shared lifecycle and health reporting for optional device services.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use devicehub_core::{ServicePhase, ServiceRegistry};
use tokio::sync::watch;

const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);
const MAX_BACKOFF: Duration = Duration::from_secs(8);

#[derive(Clone)]
pub(crate) struct ServiceReporter {
    name: Arc<str>,
    registry: ServiceRegistry,
}

impl ServiceReporter {
    pub(crate) fn connecting(&self, attempt: u32) {
        self.registry
            .record(&self.name, ServicePhase::Connecting, attempt, None);
    }

    pub(crate) fn ready(&self, attempt: u32) {
        self.registry
            .record(&self.name, ServicePhase::Ready, attempt, None);
    }

    pub(crate) fn recovering(&self, attempt: u32, error: impl Into<String>) {
        let error = error.into();
        tracing::warn!(
            component = "service_supervisor",
            service = %self.name,
            attempt,
            error = %error,
            "device service will reconnect"
        );
        self.registry
            .record(&self.name, ServicePhase::Recovering, attempt, Some(error));
    }

    pub(crate) fn unavailable(&self, attempt: u32, error: impl Into<String>) {
        self.registry.record(
            &self.name,
            ServicePhase::Unavailable,
            attempt,
            Some(error.into()),
        );
    }

    pub(crate) fn retrying(&self, attempt: u32, error: impl Into<String>) {
        let error = error.into();
        if attempt >= 3 {
            self.unavailable(attempt, error);
        } else {
            self.recovering(attempt, error);
        }
    }

    pub(crate) fn stopped(&self, attempt: u32) {
        self.registry
            .record(&self.name, ServicePhase::Stopped, attempt, None);
    }
}

pub(crate) struct ServiceSupervisor {
    registry: ServiceRegistry,
    shutdown: watch::Sender<bool>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl ServiceSupervisor {
    pub(crate) fn new(registry: ServiceRegistry) -> Self {
        registry.clear();
        let (shutdown, _) = watch::channel(false);
        Self {
            registry,
            shutdown,
            tasks: Vec::new(),
        }
    }

    pub(crate) fn reporter(&self, name: &'static str) -> ServiceReporter {
        ServiceReporter {
            name: Arc::from(name),
            registry: self.registry.clone(),
        }
    }

    pub(crate) fn shutdown_receiver(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    pub(crate) fn spawn(&mut self, task: impl Future<Output = ()> + 'static) {
        self.tasks.push(tokio::task::spawn_local(task));
    }

    pub(crate) async fn shutdown(&mut self) {
        let _ = self.shutdown.send(true);
        let deadline = tokio::time::Instant::now() + SHUTDOWN_GRACE;
        for mut task in self.tasks.drain(..) {
            if tokio::time::timeout_at(deadline, &mut task).await.is_err() {
                tracing::warn!(
                    component = "service_supervisor",
                    task_id = ?task.id(),
                    grace_ms = SHUTDOWN_GRACE.as_millis(),
                    "device service exceeded shutdown grace; aborting task"
                );
                task.abort();
                let _ = task.await;
            }
        }
    }
}

impl Drop for ServiceSupervisor {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        for task in &self.tasks {
            task.abort();
        }
    }
}

pub(crate) fn reconnect_backoff(attempt: u32) -> Duration {
    Duration::from_millis(500_u64.saturating_mul(1_u64 << attempt.min(4))).min(MAX_BACKOFF)
}

pub(crate) async fn wait_for_retry(shutdown: &mut watch::Receiver<bool>, delay: Duration) -> bool {
    if *shutdown.borrow() {
        return false;
    }
    tokio::select! {
        _ = tokio::time::sleep(delay) => true,
        changed = shutdown.changed() => changed.is_ok() && !*shutdown.borrow(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    #[test]
    fn reconnect_backoff_is_bounded() {
        assert_eq!(reconnect_backoff(0), Duration::from_millis(500));
        assert_eq!(reconnect_backoff(20), Duration::from_secs(8));
    }

    #[test]
    fn shutdown_waits_for_service_resources_to_drop() {
        struct Resource(Arc<AtomicBool>);

        impl Drop for Resource {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }

        let local = tokio::task::LocalSet::new();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let dropped = Arc::new(AtomicBool::new(false));
        let task_dropped = dropped.clone();

        runtime.block_on(local.run_until(async move {
            let mut supervisor = ServiceSupervisor::new(ServiceRegistry::default());
            supervisor.spawn(async move {
                let _resource = Resource(task_dropped);
                std::future::pending::<()>().await;
            });
            tokio::task::yield_now().await;
            supervisor.shutdown().await;
        }));

        assert!(dropped.load(Ordering::Acquire));
    }
}
