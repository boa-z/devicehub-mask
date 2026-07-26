//! Host-independent service health observations and transition policy.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

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

/// Shared observation port for the supervised services of one device session.
#[derive(Clone, Default)]
pub struct ServiceRegistry(Arc<Mutex<BTreeMap<String, ServiceHealth>>>);

impl ServiceRegistry {
    pub fn snapshot(&self) -> Vec<ServiceHealth> {
        self.0
            .lock()
            .expect("service health registry lock poisoned")
            .values()
            .cloned()
            .collect()
    }

    pub fn clear(&self) {
        self.0
            .lock()
            .expect("service health registry lock poisoned")
            .clear();
    }

    pub fn record(&self, name: &str, phase: ServicePhase, attempt: u32, error: Option<String>) {
        let mut services = self
            .0
            .lock()
            .expect("service health registry lock poisoned");
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
    fn recovery_transitions_increment_and_preserve_restart_count() {
        let registry = ServiceRegistry::default();
        registry.record("graphics", ServicePhase::Connecting, 1, None);
        registry.record("graphics", ServicePhase::Ready, 1, None);
        registry.record(
            "graphics",
            ServicePhase::Recovering,
            1,
            Some("closed".into()),
        );
        registry.record("graphics", ServicePhase::Connecting, 2, None);
        registry.record("graphics", ServicePhase::Ready, 2, None);
        let health = registry.snapshot().pop().unwrap();
        assert_eq!(health.phase, ServicePhase::Ready);
        assert_eq!(health.attempts, 2);
        assert_eq!(health.restarts, 1);
        assert_eq!(health.last_error, None);

        let reader = registry.clone();
        registry.clear();
        assert!(reader.snapshot().is_empty());
    }
}
