//! Shared performance observations and demand-controlled sampling lifecycle.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(test)]
use devicehub_core::ProcessPerformance;
use devicehub_core::{AppActivityEvent, PerformanceSnapshot, ProcessEnergy};
use idevice::dvt::energy_monitor::EnergySample;
use idevice::dvt::graphics::GraphicsSample;
use idevice::dvt::notifications::NotificationInfo;
use idevice::dvt::sysmontap::SysmontapSample;
use tokio::sync::watch;

use super::network::{NetworkRateSample, normalize_interfaces};
use super::system::{
    ProcessSchema, cpu_count, normalize_aggregate_cpu_percent, numeric_value, physical_cpu_count,
    physical_memory_bytes, top_processes,
};

const MAX_ENERGY_PROCESSES: usize = 16;
const MAX_ACTIVITY_EVENTS: usize = 100;
#[cfg(test)]
pub(super) const TEST_MAX_ENERGY_PROCESSES: usize = MAX_ENERGY_PROCESSES;
#[cfg(test)]
pub(super) const TEST_MAX_ACTIVITY_EVENTS: usize = MAX_ACTIVITY_EVENTS;
const MAX_ACTIVITY_TYPE_CHARS: usize = 96;
const MAX_ACTIVITY_NAME_CHARS: usize = 128;
const MAX_ACTIVITY_STATE_CHARS: usize = 160;

struct PerformanceSlotInner {
    sample: Mutex<PerformanceSnapshot>,
    activity_events: Mutex<VecDeque<AppActivityEvent>>,
    activity_sequence: AtomicU64,
}

#[derive(Clone)]
pub struct PerformanceSlot(Arc<PerformanceSlotInner>);

impl Default for PerformanceSlot {
    fn default() -> Self {
        Self(Arc::new(PerformanceSlotInner {
            sample: Mutex::new(PerformanceSnapshot::default()),
            activity_events: Mutex::new(VecDeque::with_capacity(MAX_ACTIVITY_EVENTS)),
            activity_sequence: AtomicU64::new(0),
        }))
    }
}

impl PerformanceSlot {
    pub fn get(&self) -> PerformanceSnapshot {
        self.0.sample.lock().unwrap().clone()
    }

    pub fn app_activity(&self) -> Vec<AppActivityEvent> {
        self.0
            .activity_events
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .collect()
    }

    pub fn reset(&self) {
        *self.0.sample.lock().unwrap() = PerformanceSnapshot::default();
        self.0.activity_events.lock().unwrap().clear();
        self.0.activity_sequence.store(0, Ordering::Relaxed);
    }

    pub(super) fn update_system(
        &self,
        sample: &SysmontapSample,
        cpu_count: u32,
        process_schema: &ProcessSchema,
    ) {
        let mut snapshot = self.0.sample.lock().unwrap();
        snapshot.captured_at_ms = unix_millis();
        snapshot.logical_cpu_count = Some(cpu_count);
        if let Some(processes) = sample.processes.as_ref() {
            snapshot.process_count = Some(processes.len() as u32);
            snapshot.top_processes = top_processes(processes, process_schema, cpu_count);
        }
        let raw_cpu_total_load = sample
            .system_cpu_usage
            .as_ref()
            .and_then(|cpu| cpu.get("CPU_TotalLoad"))
            .and_then(numeric_value);
        let normalized_cpu_load =
            raw_cpu_total_load.and_then(|value| normalize_aggregate_cpu_percent(value, cpu_count));
        if let Some(cpu) = sample.system_cpu_usage.as_ref() {
            tracing::debug!(
                raw_cpu_total_load,
                cpu_count,
                normalized_cpu_load,
                fields = ?cpu.keys().collect::<Vec<_>>(),
                "received DVT system CPU sample"
            );
            snapshot.system_cpu_percent = normalized_cpu_load;
        }
    }

    pub(super) fn update_hardware(&self, hardware: &plist::Dictionary) {
        let mut snapshot = self.0.sample.lock().unwrap();
        snapshot.logical_cpu_count = cpu_count(hardware);
        snapshot.physical_cpu_count = physical_cpu_count(hardware);
        snapshot.physical_memory_bytes = physical_memory_bytes(hardware);
    }

    pub(super) fn update_network_interfaces(&self, network: &plist::Dictionary) {
        let (interfaces, truncated) = normalize_interfaces(network);
        let mut snapshot = self.0.sample.lock().unwrap();
        snapshot.network_interfaces = interfaces;
        snapshot.network_interfaces_available = true;
        snapshot.network_interfaces_truncated = truncated;
    }

    pub(super) fn update_graphics(&self, sample: &GraphicsSample) {
        let mut snapshot = self.0.sample.lock().unwrap();
        snapshot.captured_at_ms = unix_millis();
        snapshot.graphics_fps = sample.fps.is_finite().then_some(sample.fps.max(0.0));
        snapshot.gpu_allocated_bytes = Some(sample.alloc_system_memory);
        snapshot.gpu_in_use_bytes = Some(sample.in_use_system_memory);
        snapshot.gpu_driver_bytes = Some(sample.in_use_system_memory_driver);
        snapshot.gpu_recovery_count = Some(sample.recovery_count);
    }

    pub(super) fn update_network(&self, sample: NetworkRateSample) {
        let mut snapshot = self.0.sample.lock().unwrap();
        snapshot.captured_at_ms = unix_millis();
        snapshot.network_rx_bytes_per_second = Some(sample.rx_bytes_per_second);
        snapshot.network_tx_bytes_per_second = Some(sample.tx_bytes_per_second);
        snapshot.network_recent_connections = Some(sample.recent_connections);
    }

    pub(super) fn energy_targets(&self) -> Vec<u32> {
        let snapshot = self.0.sample.lock().unwrap();
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

    pub(super) fn update_energy(&self, samples: Vec<EnergySample>) {
        let mut snapshot = self.0.sample.lock().unwrap();
        let names = snapshot
            .top_processes
            .iter()
            .map(|process| (process.pid, process.name.clone()))
            .collect::<HashMap<_, _>>();
        let mut processes = samples
            .into_iter()
            .filter(|sample| sample.pid > 0 && names.contains_key(&sample.pid))
            .map(|sample| ProcessEnergy {
                pid: sample.pid,
                name: names
                    .get(&sample.pid)
                    .cloned()
                    .unwrap_or_else(|| format!("pid {}", sample.pid)),
                total_score: energy_score(sample.total_energy),
                cpu_score: energy_score(sample.cpu_energy),
                gpu_score: energy_score(sample.gpu_energy),
                networking_score: energy_score(sample.networking_energy),
                display_score: energy_score(sample.display_energy),
                location_score: energy_score(sample.location_energy),
                app_state_score: energy_score(sample.appstate_energy),
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

    pub(super) fn publish_app_activity(&self, notification: NotificationInfo) {
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
                &notification.notification_type,
                MAX_ACTIVITY_TYPE_CHARS,
            )
            .unwrap_or_else(|| "unknown".into()),
            app_name: bounded_activity_text(&notification.app_name, MAX_ACTIVITY_NAME_CHARS),
            exec_name: bounded_activity_text(&notification.exec_name, MAX_ACTIVITY_NAME_CHARS),
            pid: (notification.pid > 0).then_some(notification.pid),
            state_description: bounded_activity_text(
                &notification.state_description,
                MAX_ACTIVITY_STATE_CHARS,
            ),
        };
        let mut events = self.0.activity_events.lock().unwrap();
        if events.len() == MAX_ACTIVITY_EVENTS {
            events.pop_front();
        }
        events.push_back(event);
    }

    #[cfg(test)]
    pub(super) fn replace_top_processes(&self, processes: Vec<ProcessPerformance>) {
        self.0.sample.lock().unwrap().top_processes = processes;
    }
}

#[derive(Clone, Default)]
pub struct PerformanceDemand(crate::demand::Demand);

impl PerformanceDemand {
    pub fn set(&self, enabled: bool) {
        self.0.set(enabled);
    }

    pub fn enabled(&self) -> bool {
        self.0.enabled()
    }

    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.0.subscribe()
    }

    pub fn acquire(&self) -> crate::demand::DemandLease {
        self.0.acquire()
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
