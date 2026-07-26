//! Runtime-specific performance observation conversion and demand lifecycle.

use devicehub_core::{
    AppActivityObservation, EnergyMeasurement, PerformanceObservation, PerformanceSlot,
    ProcessPerformanceObservation,
};
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

#[cfg(test)]
pub(super) const TEST_MAX_ENERGY_PROCESSES: usize = devicehub_core::MAX_ENERGY_PROCESSES;
#[cfg(test)]
pub(super) const TEST_MAX_ACTIVITY_EVENTS: usize = devicehub_core::MAX_APP_ACTIVITY_EVENTS;

pub(super) fn update_system(
    slot: &PerformanceSlot,
    sample: &SysmontapSample,
    logical_cpu_count: u32,
    process_schema: &ProcessSchema,
) {
    let processes = sample
        .processes
        .as_ref()
        .map(|processes| ProcessPerformanceObservation {
            process_count: processes.len() as u32,
            top_processes: top_processes(processes, process_schema, logical_cpu_count),
        });
    let raw_cpu_total_load = sample
        .system_cpu_usage
        .as_ref()
        .and_then(|cpu| cpu.get("CPU_TotalLoad"))
        .and_then(numeric_value);
    let normalized_cpu_load = raw_cpu_total_load
        .and_then(|value| normalize_aggregate_cpu_percent(value, logical_cpu_count));
    if let Some(cpu) = sample.system_cpu_usage.as_ref() {
        tracing::debug!(
            raw_cpu_total_load,
            cpu_count = logical_cpu_count,
            normalized_cpu_load,
            fields = ?cpu.keys().collect::<Vec<_>>(),
            "received DVT system CPU sample"
        );
    }
    slot.observe(PerformanceObservation::System {
        logical_cpu_count,
        processes,
        system_cpu_percent: sample
            .system_cpu_usage
            .as_ref()
            .map(|_| normalized_cpu_load),
    });
}

pub(super) fn update_hardware(slot: &PerformanceSlot, hardware: &plist::Dictionary) {
    slot.observe(PerformanceObservation::Hardware {
        logical_cpu_count: cpu_count(hardware),
        physical_cpu_count: physical_cpu_count(hardware),
        physical_memory_bytes: physical_memory_bytes(hardware),
    });
}

pub(super) fn update_network_interfaces(slot: &PerformanceSlot, network: &plist::Dictionary) {
    let (interfaces, truncated) = normalize_interfaces(network);
    slot.observe(PerformanceObservation::NetworkInterfaces {
        interfaces,
        truncated,
    });
}

pub(super) fn update_graphics(slot: &PerformanceSlot, sample: &GraphicsSample) {
    slot.observe(PerformanceObservation::Graphics {
        fps: sample.fps.is_finite().then_some(sample.fps.max(0.0)),
        allocated_bytes: sample.alloc_system_memory,
        in_use_bytes: sample.in_use_system_memory,
        driver_bytes: sample.in_use_system_memory_driver,
        recovery_count: sample.recovery_count,
    });
}

pub(super) fn update_network(slot: &PerformanceSlot, sample: NetworkRateSample) {
    slot.observe(PerformanceObservation::Network {
        rx_bytes_per_second: sample.rx_bytes_per_second,
        tx_bytes_per_second: sample.tx_bytes_per_second,
        recent_connections: sample.recent_connections,
    });
}

pub(super) fn update_energy(slot: &PerformanceSlot, samples: Vec<EnergySample>) {
    slot.observe(PerformanceObservation::Energy(
        samples
            .into_iter()
            .map(|sample| EnergyMeasurement {
                pid: sample.pid,
                total_score: sample.total_energy,
                cpu_score: sample.cpu_energy,
                gpu_score: sample.gpu_energy,
                networking_score: sample.networking_energy,
                display_score: sample.display_energy,
                location_score: sample.location_energy,
                app_state_score: sample.appstate_energy,
            })
            .collect(),
    ));
}

pub(super) fn publish_app_activity(slot: &PerformanceSlot, notification: NotificationInfo) {
    slot.publish_app_activity(AppActivityObservation {
        notification_type: notification.notification_type,
        app_name: notification.app_name,
        exec_name: notification.exec_name,
        pid: notification.pid,
        state_description: notification.state_description,
    });
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
