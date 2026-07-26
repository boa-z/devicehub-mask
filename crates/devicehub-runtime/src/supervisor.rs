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
        for mut task in self.tasks.drain(..) {
            if tokio::time::timeout(SHUTDOWN_GRACE, &mut task)
                .await
                .is_err()
            {
                task.abort();
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
    use super::*;

    #[test]
    fn reconnect_backoff_is_bounded() {
        assert_eq!(reconnect_backoff(0), Duration::from_millis(500));
        assert_eq!(reconnect_backoff(20), Duration::from_secs(8));
    }
}
