//! Shared lifecycle and health reporting for optional device services.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::sync::watch;

const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);
const MAX_BACKOFF: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServicePhase {
    Connecting,
    Ready,
    Recovering,
    Unavailable,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServiceHealth {
    pub name: String,
    pub phase: ServicePhase,
    pub attempts: u32,
    pub restarts: u32,
    pub last_error: Option<String>,
    pub updated_at_ms: u64,
}

#[derive(Clone, Default)]
pub struct ServiceRegistry(Arc<Mutex<BTreeMap<String, ServiceHealth>>>);

impl ServiceRegistry {
    pub fn snapshot(&self) -> Vec<ServiceHealth> {
        self.0.lock().unwrap().values().cloned().collect()
    }

    pub fn clear(&self) {
        self.0.lock().unwrap().clear();
    }

    fn update(&self, name: &str, phase: ServicePhase, attempt: u32, error: Option<String>) {
        let mut services = self.0.lock().unwrap();
        let previous_restarts = services.get(name).map_or(0, |service| service.restarts);
        let restarts = if matches!(phase, ServicePhase::Recovering | ServicePhase::Unavailable) {
            previous_restarts.saturating_add(1)
        } else {
            previous_restarts
        };
        services.insert(
            name.into(),
            ServiceHealth {
                name: name.into(),
                phase,
                attempts: attempt,
                restarts,
                last_error: error,
                updated_at_ms: unix_millis(),
            },
        );
    }
}

#[derive(Clone)]
pub struct ServiceReporter {
    name: Arc<str>,
    registry: ServiceRegistry,
}

impl ServiceReporter {
    pub fn connecting(&self, attempt: u32) {
        self.registry
            .update(&self.name, ServicePhase::Connecting, attempt, None);
    }

    pub fn ready(&self, attempt: u32) {
        self.registry
            .update(&self.name, ServicePhase::Ready, attempt, None);
    }

    pub fn recovering(&self, attempt: u32, error: impl Into<String>) {
        let error = error.into();
        tracing::warn!(
            component = "service_supervisor",
            service = %self.name,
            attempt,
            error = %error,
            "device service will reconnect"
        );
        self.registry
            .update(&self.name, ServicePhase::Recovering, attempt, Some(error));
    }

    pub fn unavailable(&self, attempt: u32, error: impl Into<String>) {
        self.registry.update(
            &self.name,
            ServicePhase::Unavailable,
            attempt,
            Some(error.into()),
        );
    }

    pub fn retrying(&self, attempt: u32, error: impl Into<String>) {
        let error = error.into();
        if attempt >= 3 {
            self.unavailable(attempt, error);
        } else {
            self.recovering(attempt, error);
        }
    }

    pub fn stopped(&self, attempt: u32) {
        self.registry
            .update(&self.name, ServicePhase::Stopped, attempt, None);
    }
}

pub struct ServiceSupervisor {
    registry: ServiceRegistry,
    shutdown: watch::Sender<bool>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl ServiceSupervisor {
    pub fn new(registry: ServiceRegistry) -> Self {
        registry.clear();
        let (shutdown, _) = watch::channel(false);
        Self {
            registry,
            shutdown,
            tasks: Vec::new(),
        }
    }

    pub fn reporter(&self, name: &'static str) -> ServiceReporter {
        ServiceReporter {
            name: Arc::from(name),
            registry: self.registry.clone(),
        }
    }

    pub fn shutdown_receiver(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    pub fn spawn(&mut self, task: impl Future<Output = ()> + 'static) {
        self.tasks.push(tokio::task::spawn_local(task));
    }

    pub async fn shutdown(&mut self) {
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

pub fn reconnect_backoff(attempt: u32) -> Duration {
    Duration::from_millis(500_u64.saturating_mul(1_u64 << attempt.min(4))).min(MAX_BACKOFF)
}

pub async fn wait_for_retry(shutdown: &mut watch::Receiver<bool>, delay: Duration) -> bool {
    if *shutdown.borrow() {
        return false;
    }
    tokio::select! {
        _ = tokio::time::sleep(delay) => true,
        changed = shutdown.changed() => changed.is_ok() && !*shutdown.borrow(),
    }
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_health_tracks_recovery_without_losing_restart_count() {
        let registry = ServiceRegistry::default();
        let reporter = ServiceReporter {
            name: Arc::from("graphics"),
            registry: registry.clone(),
        };
        reporter.connecting(1);
        reporter.ready(1);
        reporter.recovering(1, "closed");
        reporter.connecting(2);
        reporter.ready(2);
        let health = registry.snapshot().pop().unwrap();
        assert_eq!(health.phase, ServicePhase::Ready);
        assert_eq!(health.attempts, 2);
        assert_eq!(health.restarts, 1);
        assert_eq!(health.last_error, None);
    }

    #[test]
    fn reconnect_backoff_is_bounded() {
        assert_eq!(reconnect_backoff(0), Duration::from_millis(500));
        assert_eq!(reconnect_backoff(20), Duration::from_secs(8));
    }
}
