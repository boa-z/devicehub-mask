//! Supervised DVT performance sampling over the active CoreDevice tunnel.

#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::time::Duration;
#[cfg(test)]
use std::time::Instant;

#[cfg(test)]
use devicehub_core::{DeviceNetworkInterfaceKind, ProcessPerformance};
#[cfg(test)]
use idevice::RsdService;
#[cfg(test)]
use idevice::dvt::device_info::DeviceInfoClient;
#[cfg(test)]
use idevice::dvt::energy_monitor::{EnergyMonitorClient, EnergySample};
#[cfg(test)]
use idevice::dvt::network_monitor::{NetworkEvent, NetworkMonitorClient};
#[cfg(test)]
use idevice::dvt::notifications::NotificationInfo;
#[cfg(test)]
use idevice::dvt::remote_server::RemoteServerClient;
#[cfg(test)]
use idevice::dvt::sysmontap::SysmontapSample;
#[cfg(test)]
use idevice::dvt::sysmontap::{SysmontapClient, SysmontapConfig};
#[cfg(test)]
use idevice::rsd::RsdHandshake;
#[cfg(test)]
use plist::Value;

mod activity;
mod energy;
mod graphics;
mod network;
mod slot;
mod source;
mod system;

pub(crate) use activity::supervise_performance_app_activity;
pub(crate) use energy::supervise_performance_energy;
pub(crate) use graphics::supervise_performance_graphics;
pub(crate) use network::supervise_performance_network;
#[cfg(test)]
use network::{
    NetworkAccumulator, TEST_CONNECTION_TTL as NETWORK_CONNECTION_TTL,
    TEST_MAX_INTERFACES as MAX_NETWORK_INTERFACES,
    normalize_interfaces as normalize_network_interfaces,
};
pub use slot::{PerformanceDemand, PerformanceSlot};
#[cfg(test)]
use slot::{
    TEST_MAX_ACTIVITY_EVENTS as MAX_ACTIVITY_EVENTS,
    TEST_MAX_ENERGY_PROCESSES as MAX_ENERGY_PROCESSES,
};
pub(crate) use system::supervise_performance_system;
#[cfg(test)]
use system::{
    ProcessSchema, TEST_SAMPLE_INTERVAL_MS as SAMPLE_INTERVAL_MS,
    TEST_TOP_PROCESSES_PER_METRIC as TOP_PROCESSES_PER_METRIC, cpu_count,
    normalize_aggregate_cpu_percent, normalize_process_for_test as normalize_process,
    physical_cpu_count, physical_memory_bytes, top_processes,
};

#[cfg(test)]
mod tests;
