use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

pub const MAX_ENERGY_PROCESSES: usize = 16;
pub const MAX_APP_ACTIVITY_EVENTS: usize = 100;
const MAX_ACTIVITY_TYPE_CHARS: usize = 96;
const MAX_ACTIVITY_NAME_CHARS: usize = 128;
const MAX_ACTIVITY_STATE_CHARS: usize = 160;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProcessPerformance {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: Option<f64>,
    pub memory_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProcessEnergy {
    pub pid: u32,
    pub name: String,
    pub total_score: f64,
    pub cpu_score: f64,
    pub gpu_score: f64,
    pub networking_score: f64,
    pub display_score: f64,
    pub location_score: f64,
    pub app_state_score: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceNetworkInterfaceKind {
    Wifi,
    Cellular,
    Ethernet,
    Loopback,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceNetworkInterface {
    pub name: String,
    pub kind: DeviceNetworkInterfaceKind,
    pub description: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PerformanceSnapshot {
    pub captured_at_ms: u64,
    pub system_cpu_percent: Option<f64>,
    pub process_count: Option<u32>,
    pub logical_cpu_count: Option<u32>,
    pub physical_cpu_count: Option<u32>,
    pub physical_memory_bytes: Option<u64>,
    pub top_processes: Vec<ProcessPerformance>,
    pub energy_processes: Vec<ProcessEnergy>,
    pub graphics_fps: Option<f64>,
    pub gpu_allocated_bytes: Option<u64>,
    pub gpu_in_use_bytes: Option<u64>,
    pub gpu_driver_bytes: Option<u64>,
    pub gpu_recovery_count: Option<u64>,
    pub network_rx_bytes_per_second: Option<f64>,
    pub network_tx_bytes_per_second: Option<f64>,
    pub network_recent_connections: Option<u32>,
    pub network_interfaces: Vec<DeviceNetworkInterface>,
    pub network_interfaces_available: bool,
    pub network_interfaces_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppActivityEvent {
    pub sequence: u64,
    pub received_at_ms: u64,
    pub notification_type: String,
    pub app_name: Option<String>,
    pub exec_name: Option<String>,
    pub pid: Option<u32>,
    pub state_description: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessPerformanceObservation {
    pub process_count: u32,
    pub top_processes: Vec<ProcessPerformance>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnergyMeasurement {
    pub pid: u32,
    pub total_score: f64,
    pub cpu_score: f64,
    pub gpu_score: f64,
    pub networking_score: f64,
    pub display_score: f64,
    pub location_score: f64,
    pub app_state_score: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppActivityObservation {
    pub notification_type: String,
    pub app_name: String,
    pub exec_name: String,
    pub pid: u32,
    pub state_description: String,
}

/// Normalized updates produced by runtime-specific performance sources.
#[derive(Debug, Clone, PartialEq)]
pub enum PerformanceObservation {
    System {
        logical_cpu_count: u32,
        processes: Option<ProcessPerformanceObservation>,
        /// `None` retains the last sample; `Some(None)` records an invalid or
        /// unavailable value from a present CPU sample.
        system_cpu_percent: Option<Option<f64>>,
    },
    Hardware {
        logical_cpu_count: Option<u32>,
        physical_cpu_count: Option<u32>,
        physical_memory_bytes: Option<u64>,
    },
    NetworkInterfaces {
        interfaces: Vec<DeviceNetworkInterface>,
        truncated: bool,
    },
    Graphics {
        fps: Option<f64>,
        allocated_bytes: u64,
        in_use_bytes: u64,
        driver_bytes: u64,
        recovery_count: u64,
    },
    Network {
        rx_bytes_per_second: f64,
        tx_bytes_per_second: f64,
        recent_connections: u32,
    },
    Energy(Vec<EnergyMeasurement>),
}

struct PerformanceSlotInner {
    sample: Mutex<PerformanceSnapshot>,
    activity_events: Mutex<VecDeque<AppActivityEvent>>,
    activity_sequence: AtomicU64,
}

/// Shared observation port for one device session's performance sources.
#[derive(Clone)]
pub struct PerformanceSlot(Arc<PerformanceSlotInner>);

impl Default for PerformanceSlot {
    fn default() -> Self {
        Self(Arc::new(PerformanceSlotInner {
            sample: Mutex::new(PerformanceSnapshot::default()),
            activity_events: Mutex::new(VecDeque::with_capacity(MAX_APP_ACTIVITY_EVENTS)),
            activity_sequence: AtomicU64::new(0),
        }))
    }
}

impl PerformanceSlot {
    pub fn get(&self) -> PerformanceSnapshot {
        self.0
            .sample
            .lock()
            .expect("performance snapshot lock poisoned")
            .clone()
    }

    pub fn app_activity(&self) -> Vec<AppActivityEvent> {
        self.0
            .activity_events
            .lock()
            .expect("performance activity lock poisoned")
            .iter()
            .cloned()
            .collect()
    }

    pub fn reset(&self) {
        *self
            .0
            .sample
            .lock()
            .expect("performance snapshot lock poisoned") = PerformanceSnapshot::default();
        self.0
            .activity_events
            .lock()
            .expect("performance activity lock poisoned")
            .clear();
        self.0.activity_sequence.store(0, Ordering::Relaxed);
    }

    pub fn observe(&self, observation: PerformanceObservation) {
        let mut snapshot = self
            .0
            .sample
            .lock()
            .expect("performance snapshot lock poisoned");
        match observation {
            PerformanceObservation::System {
                logical_cpu_count,
                processes,
                system_cpu_percent,
            } => {
                snapshot.captured_at_ms = unix_millis();
                snapshot.logical_cpu_count = Some(logical_cpu_count);
                if let Some(processes) = processes {
                    snapshot.process_count = Some(processes.process_count);
                    snapshot.top_processes = processes.top_processes;
                }
                if let Some(cpu_percent) = system_cpu_percent {
                    snapshot.system_cpu_percent = cpu_percent;
                }
            }
            PerformanceObservation::Hardware {
                logical_cpu_count,
                physical_cpu_count,
                physical_memory_bytes,
            } => {
                snapshot.logical_cpu_count = logical_cpu_count;
                snapshot.physical_cpu_count = physical_cpu_count;
                snapshot.physical_memory_bytes = physical_memory_bytes;
            }
            PerformanceObservation::NetworkInterfaces {
                interfaces,
                truncated,
            } => {
                snapshot.network_interfaces = interfaces;
                snapshot.network_interfaces_available = true;
                snapshot.network_interfaces_truncated = truncated;
            }
            PerformanceObservation::Graphics {
                fps,
                allocated_bytes,
                in_use_bytes,
                driver_bytes,
                recovery_count,
            } => {
                snapshot.captured_at_ms = unix_millis();
                snapshot.graphics_fps = fps;
                snapshot.gpu_allocated_bytes = Some(allocated_bytes);
                snapshot.gpu_in_use_bytes = Some(in_use_bytes);
                snapshot.gpu_driver_bytes = Some(driver_bytes);
                snapshot.gpu_recovery_count = Some(recovery_count);
            }
            PerformanceObservation::Network {
                rx_bytes_per_second,
                tx_bytes_per_second,
                recent_connections,
            } => {
                snapshot.captured_at_ms = unix_millis();
                snapshot.network_rx_bytes_per_second = Some(rx_bytes_per_second);
                snapshot.network_tx_bytes_per_second = Some(tx_bytes_per_second);
                snapshot.network_recent_connections = Some(recent_connections);
            }
            PerformanceObservation::Energy(measurements) => {
                let names = snapshot
                    .top_processes
                    .iter()
                    .map(|process| (process.pid, process.name.clone()))
                    .collect::<HashMap<_, _>>();
                let mut processes = measurements
                    .into_iter()
                    .filter(|measurement| {
                        measurement.pid > 0 && names.contains_key(&measurement.pid)
                    })
                    .map(|measurement| ProcessEnergy {
                        pid: measurement.pid,
                        name: names
                            .get(&measurement.pid)
                            .cloned()
                            .unwrap_or_else(|| format!("pid {}", measurement.pid)),
                        total_score: energy_score(measurement.total_score),
                        cpu_score: energy_score(measurement.cpu_score),
                        gpu_score: energy_score(measurement.gpu_score),
                        networking_score: energy_score(measurement.networking_score),
                        display_score: energy_score(measurement.display_score),
                        location_score: energy_score(measurement.location_score),
                        app_state_score: energy_score(measurement.app_state_score),
                    })
                    .collect::<Vec<_>>();
                processes.sort_by(|left, right| {
                    right
                        .total_score
                        .total_cmp(&left.total_score)
                        .then_with(|| left.pid.cmp(&right.pid))
                });
                processes.truncate(MAX_ENERGY_PROCESSES);
                snapshot.captured_at_ms = unix_millis();
                snapshot.energy_processes = processes;
            }
        }
    }

    pub fn energy_targets(&self) -> Vec<u32> {
        let snapshot = self
            .0
            .sample
            .lock()
            .expect("performance snapshot lock poisoned");
        let mut seen = HashSet::with_capacity(MAX_ENERGY_PROCESSES);
        let mut pids = snapshot
            .top_processes
            .iter()
            .map(|process| process.pid)
            .filter(|pid| *pid > 0 && seen.insert(*pid))
            .take(MAX_ENERGY_PROCESSES)
            .collect::<Vec<_>>();
        pids.sort_unstable();
        pids
    }

    pub fn publish_app_activity(&self, observation: AppActivityObservation) {
        let sequence = self
            .0
            .activity_sequence
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
            .max(1);
        let event = AppActivityEvent {
            sequence,
            received_at_ms: unix_millis(),
            notification_type: bounded_activity_text(
                &observation.notification_type,
                MAX_ACTIVITY_TYPE_CHARS,
            )
            .unwrap_or_else(|| "unknown".into()),
            app_name: bounded_activity_text(&observation.app_name, MAX_ACTIVITY_NAME_CHARS),
            exec_name: bounded_activity_text(&observation.exec_name, MAX_ACTIVITY_NAME_CHARS),
            pid: (observation.pid > 0).then_some(observation.pid),
            state_description: bounded_activity_text(
                &observation.state_description,
                MAX_ACTIVITY_STATE_CHARS,
            ),
        };
        let mut events = self
            .0
            .activity_events
            .lock()
            .expect("performance activity lock poisoned");
        if events.len() == MAX_APP_ACTIVITY_EVENTS {
            events.pop_front();
        }
        events.push_back(event);
    }
}

fn bounded_activity_text(value: &str, max_chars: usize) -> Option<String> {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        None
    } else {
        Some(normalized.chars().take(max_chars).collect())
    }
}

fn energy_score(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
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

    fn process(pid: u32, name: &str) -> ProcessPerformance {
        ProcessPerformance {
            pid,
            name: name.into(),
            cpu_percent: Some(f64::from(pid)),
            memory_bytes: None,
        }
    }

    #[test]
    fn partial_system_observations_preserve_previous_fields() {
        let slot = PerformanceSlot::default();
        slot.observe(PerformanceObservation::System {
            logical_cpu_count: 6,
            processes: Some(ProcessPerformanceObservation {
                process_count: 1,
                top_processes: vec![process(1, "app")],
            }),
            system_cpu_percent: Some(Some(40.0)),
        });
        slot.observe(PerformanceObservation::System {
            logical_cpu_count: 6,
            processes: None,
            system_cpu_percent: None,
        });
        let snapshot = slot.get();
        assert_eq!(snapshot.logical_cpu_count, Some(6));
        assert_eq!(snapshot.process_count, Some(1));
        assert_eq!(snapshot.top_processes, vec![process(1, "app")]);
        assert_eq!(snapshot.system_cpu_percent, Some(40.0));

        slot.observe(PerformanceObservation::System {
            logical_cpu_count: 6,
            processes: None,
            system_cpu_percent: Some(None),
        });
        assert_eq!(slot.get().system_cpu_percent, None);
    }

    #[test]
    fn independent_observations_merge_without_erasing_hardware() {
        let slot = PerformanceSlot::default();
        slot.observe(PerformanceObservation::Hardware {
            logical_cpu_count: Some(8),
            physical_cpu_count: Some(6),
            physical_memory_bytes: Some(6_442_450_944),
        });
        slot.observe(PerformanceObservation::Graphics {
            fps: Some(60.0),
            allocated_bytes: 10,
            in_use_bytes: 8,
            driver_bytes: 3,
            recovery_count: 0,
        });
        let snapshot = slot.get();
        assert_eq!(snapshot.logical_cpu_count, Some(8));
        assert_eq!(snapshot.physical_cpu_count, Some(6));
        assert_eq!(snapshot.physical_memory_bytes, Some(6_442_450_944));
        assert_eq!(snapshot.graphics_fps, Some(60.0));
        assert!(snapshot.captured_at_ms > 0);
    }

    #[test]
    fn activity_observations_are_sanitized_bounded_and_reset() {
        let slot = PerformanceSlot::default();
        for index in 0..=MAX_APP_ACTIVITY_EVENTS {
            slot.publish_app_activity(AppActivityObservation {
                notification_type: " application\nstate ".into(),
                app_name: " Example  Game ".into(),
                exec_name: " Example\tGame ".into(),
                pid: (index + 1) as u32,
                state_description: " foreground\nactive ".into(),
            });
        }
        let events = slot.app_activity();
        assert_eq!(events.len(), MAX_APP_ACTIVITY_EVENTS);
        assert_eq!(events.first().unwrap().sequence, 2);
        assert_eq!(events.last().unwrap().sequence, 101);
        assert_eq!(
            events.last().unwrap().notification_type,
            "application state"
        );
        assert_eq!(
            events.last().unwrap().exec_name.as_deref(),
            Some("Example Game")
        );
        assert_eq!(
            events.last().unwrap().state_description.as_deref(),
            Some("foreground active")
        );
        slot.reset();
        assert!(slot.app_activity().is_empty());
    }

    #[test]
    fn energy_observations_use_ranked_known_processes_and_sanitize_scores() {
        let slot = PerformanceSlot::default();
        slot.observe(PerformanceObservation::System {
            logical_cpu_count: 1,
            processes: Some(ProcessPerformanceObservation {
                process_count: 20,
                top_processes: (0..20)
                    .map(|index| process(100 - index, &format!("rank-{index}")))
                    .collect(),
            }),
            system_cpu_percent: None,
        });
        let targets = slot.energy_targets();
        assert_eq!(targets.len(), MAX_ENERGY_PROCESSES);
        assert!(targets.contains(&100));
        assert!(targets.contains(&85));
        assert!(!targets.contains(&84));

        slot.observe(PerformanceObservation::Energy(vec![
            EnergyMeasurement {
                pid: 100,
                total_score: 5.0,
                cpu_score: f64::NAN,
                gpu_score: -2.0,
                networking_score: 1.5,
                display_score: 0.5,
                location_score: 0.0,
                app_state_score: f64::INFINITY,
            },
            EnergyMeasurement {
                pid: 99,
                total_score: 8.0,
                cpu_score: 3.0,
                gpu_score: 2.0,
                networking_score: 1.0,
                display_score: 1.0,
                location_score: 0.5,
                app_state_score: 0.5,
            },
            EnergyMeasurement {
                pid: 777,
                total_score: 99.0,
                cpu_score: 99.0,
                gpu_score: 0.0,
                networking_score: 0.0,
                display_score: 0.0,
                location_score: 0.0,
                app_state_score: 0.0,
            },
        ]));
        let snapshot = slot.get();
        assert_eq!(snapshot.energy_processes.len(), 2);
        assert_eq!(snapshot.energy_processes[0].pid, 99);
        assert_eq!(snapshot.energy_processes[0].name, "rank-1");
        assert_eq!(snapshot.energy_processes[1].cpu_score, 0.0);
        assert_eq!(snapshot.energy_processes[1].gpu_score, 0.0);
        assert_eq!(snapshot.energy_processes[1].app_state_score, 0.0);
    }
}
