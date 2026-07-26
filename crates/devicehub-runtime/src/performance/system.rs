//! Sysmontap sampling, hardware metadata, and bounded process normalization.

use std::collections::HashSet;

use devicehub_core::ProcessPerformance;
use idevice::dvt::device_info::DeviceInfoClient;
use idevice::dvt::sysmontap::{SysmontapClient, SysmontapConfig};
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use plist::Value;
use tokio::sync::watch;

use super::source::{SETUP_TIMEOUT, connect_remote, wait_until_enabled};
use super::{PerformanceSlot, network};
use crate::supervisor::{ServiceReporter, reconnect_backoff, wait_for_retry};

const SAMPLE_INTERVAL_MS: u32 = 1_000;
const TOP_PROCESSES_PER_METRIC: usize = 10;
#[cfg(test)]
pub(super) const TEST_SAMPLE_INTERVAL_MS: u32 = SAMPLE_INTERVAL_MS;
#[cfg(test)]
pub(super) const TEST_TOP_PROCESSES_PER_METRIC: usize = TOP_PROCESSES_PER_METRIC;

pub(crate) async fn supervise_performance_system(
    adapter: AdapterHandle,
    handshake: RsdHandshake,
    slot: PerformanceSlot,
    reporter: ServiceReporter,
    mut enabled: watch::Receiver<bool>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut attempt = 0;
    loop {
        if *shutdown.borrow() {
            break;
        }
        if !wait_until_enabled(&mut enabled, &mut shutdown, &reporter, attempt).await {
            break;
        }
        attempt += 1;
        reporter.connecting(attempt);
        let result = run_once(
            adapter.clone(),
            handshake.clone(),
            slot.clone(),
            &mut shutdown,
            &mut enabled,
            &reporter,
            attempt,
        )
        .await;
        if *shutdown.borrow() {
            break;
        }
        let Some(error) = result.err() else {
            continue;
        };
        reporter.retrying(attempt, error);
        if !wait_for_retry(&mut shutdown, reconnect_backoff(attempt - 1)).await {
            break;
        }
    }
    reporter.stopped(attempt);
}

async fn run_once(
    adapter: AdapterHandle,
    handshake: RsdHandshake,
    slot: PerformanceSlot,
    shutdown: &mut watch::Receiver<bool>,
    enabled: &mut watch::Receiver<bool>,
    reporter: &ServiceReporter,
    attempt: u32,
) -> Result<(), String> {
    let network_catalog = network::load_interface_catalog(adapter.clone(), handshake.clone());
    tokio::pin!(network_catalog);
    let mut network_catalog_pending = true;
    let mut remote = connect_remote(adapter, handshake).await?;
    let (process_attributes, system_attributes, hardware) =
        tokio::time::timeout(SETUP_TIMEOUT, async {
            let mut device_info = DeviceInfoClient::new(&mut remote).await?;
            let process = device_info.sysmon_process_attributes().await?;
            let system = device_info.sysmon_system_attributes().await?;
            let hardware = device_info.hardware_information().await?;
            Ok::<_, idevice::IdeviceError>((process, system, hardware))
        })
        .await
        .map_err(|_| "DVT sysmontap attribute query timed out".to_string())?
        .map_err(|error| format!("DVT sysmontap attribute query failed: {error:?}"))?;
    let cpu_count = cpu_count(&hardware).ok_or_else(|| {
        "DVT hardware information did not report a valid logical CPU count".to_string()
    })?;
    slot.update_hardware(&hardware);
    let process_schema = ProcessSchema::new(&process_attributes);
    let mut client = SysmontapClient::new(&mut remote)
        .await
        .map_err(|error| format!("DVT sysmontap channel failed: {error:?}"))?;
    let config = SysmontapConfig {
        interval_ms: SAMPLE_INTERVAL_MS,
        process_attributes,
        system_attributes,
    };
    tokio::time::timeout(SETUP_TIMEOUT, async {
        client.set_config(&config).await?;
        client.start().await
    })
    .await
    .map_err(|_| "DVT sysmontap setup timed out".to_string())?
    .map_err(|error| format!("DVT sysmontap setup failed: {error:?}"))?;
    reporter.ready(attempt);
    loop {
        tokio::select! {
            result = &mut network_catalog, if network_catalog_pending => {
                network_catalog_pending = false;
                match result {
                    Ok(network) => {
                        slot.update_network_interfaces(&network);
                        tracing::debug!(
                            count = slot.get().network_interfaces.len(),
                            "DVT network interface catalog updated"
                        );
                    }
                    Err(error) => tracing::debug!(%error, "DVT network interface catalog unavailable"),
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    let _ = client.stop().await;
                    return Ok(());
                }
            }
            changed = enabled.changed() => {
                if changed.is_err() || !*enabled.borrow() {
                    let _ = client.stop().await;
                    return Ok(());
                }
            }
            sample = client.next_sample() => match sample {
                Ok(sample) => slot.update_system(&sample, cpu_count, &process_schema),
                Err(error) => return Err(format!("DVT sysmontap stream failed: {error:?}")),
            }
        }
    }
}

pub(super) fn numeric_value(value: &Value) -> Option<f64> {
    match value {
        Value::Real(value) => Some(*value),
        Value::Integer(value) => value
            .as_signed()
            .map(|value| value as f64)
            .or_else(|| value.as_unsigned().map(|value| value as f64)),
        _ => None,
    }
}

pub(super) fn cpu_count(hardware: &plist::Dictionary) -> Option<u32> {
    ["numberOfCpus", "numberOfPhysicalCpus"]
        .into_iter()
        .filter_map(|key| hardware.get(key))
        .filter_map(numeric_u32)
        .find(|count| (1..=256).contains(count))
}

pub(super) fn physical_cpu_count(hardware: &plist::Dictionary) -> Option<u32> {
    hardware
        .get("numberOfPhysicalCpus")
        .and_then(numeric_u32)
        .filter(|count| (1..=256).contains(count))
}

pub(super) fn physical_memory_bytes(hardware: &plist::Dictionary) -> Option<u64> {
    hardware
        .get("physicalMemory")
        .and_then(numeric_u64)
        .filter(|bytes| (16 * 1024 * 1024..=1024 * 1024 * 1024 * 1024).contains(bytes))
}

fn numeric_u32(value: &Value) -> Option<u32> {
    match value {
        Value::Integer(value) => value
            .as_unsigned()
            .and_then(|value| u32::try_from(value).ok())
            .or_else(|| {
                value
                    .as_signed()
                    .and_then(|value| u32::try_from(value).ok())
            }),
        _ => None,
    }
}

fn numeric_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Integer(value) => value.as_unsigned().or_else(|| {
            value
                .as_signed()
                .and_then(|value| u64::try_from(value).ok())
        }),
        _ => None,
    }
}

#[derive(Debug, Default)]
pub(super) struct ProcessSchema {
    name: Option<usize>,
    pid: Option<usize>,
    cpu_usage: Option<usize>,
    physical_footprint: Option<usize>,
}

impl ProcessSchema {
    pub(super) fn new(attributes: &[String]) -> Self {
        let index = |name: &str| attributes.iter().position(|attribute| attribute == name);
        Self {
            name: index("name"),
            pid: index("pid"),
            cpu_usage: index("cpuUsage"),
            physical_footprint: index("physFootprint"),
        }
    }

    #[cfg(test)]
    pub(super) fn has_expected_fields(&self) -> bool {
        self.name.is_some()
            && self.pid.is_some()
            && self.cpu_usage.is_some()
            && self.physical_footprint.is_some()
    }
}

pub(super) fn top_processes(
    processes: &plist::Dictionary,
    schema: &ProcessSchema,
    cpu_count: u32,
) -> Vec<ProcessPerformance> {
    let mut normalized = processes
        .iter()
        .filter_map(|(key, value)| normalize_process(key, value, schema, cpu_count))
        .collect::<Vec<_>>();
    let mut by_cpu = normalized.clone();
    by_cpu.sort_by(compare_process_cpu);
    normalized.sort_by(compare_process_memory);

    let mut selected = Vec::with_capacity(TOP_PROCESSES_PER_METRIC * 2);
    let mut selected_pids = HashSet::with_capacity(TOP_PROCESSES_PER_METRIC * 2);
    for process in by_cpu
        .into_iter()
        .take(TOP_PROCESSES_PER_METRIC)
        .chain(normalized.into_iter().take(TOP_PROCESSES_PER_METRIC))
    {
        if selected_pids.insert(process.pid) {
            selected.push(process);
        }
    }
    selected.sort_by(compare_process_cpu);
    selected
}

fn normalize_process(
    key: &str,
    value: &Value,
    schema: &ProcessSchema,
    cpu_count: u32,
) -> Option<ProcessPerformance> {
    let row = value.as_array()?;
    let pid = schema
        .pid
        .and_then(|index| row.get(index))
        .and_then(numeric_u32)
        .or_else(|| key.parse().ok())?;
    let name = schema
        .name
        .and_then(|index| row.get(index))
        .and_then(Value::as_string)
        .map(sanitize_process_name)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("pid {pid}"));
    let cpu_percent = schema
        .cpu_usage
        .and_then(|index| row.get(index))
        .and_then(numeric_value)
        .and_then(|value| normalize_aggregate_cpu_percent(value, cpu_count));
    let memory_bytes = schema
        .physical_footprint
        .and_then(|index| row.get(index))
        .and_then(numeric_u64);
    Some(ProcessPerformance {
        pid,
        name,
        cpu_percent,
        memory_bytes,
    })
}

#[cfg(test)]
pub(super) fn normalize_process_for_test(
    key: &str,
    value: &Value,
    schema: &ProcessSchema,
    cpu_count: u32,
) -> Option<ProcessPerformance> {
    normalize_process(key, value, schema, cpu_count)
}

fn sanitize_process_name(name: &str) -> String {
    name.chars()
        .filter(|character| !character.is_control())
        .take(256)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn compare_process_cpu(
    left: &ProcessPerformance,
    right: &ProcessPerformance,
) -> std::cmp::Ordering {
    right
        .cpu_percent
        .unwrap_or(-1.0)
        .total_cmp(&left.cpu_percent.unwrap_or(-1.0))
        .then_with(|| compare_process_memory(left, right))
        .then_with(|| left.pid.cmp(&right.pid))
}

fn compare_process_memory(
    left: &ProcessPerformance,
    right: &ProcessPerformance,
) -> std::cmp::Ordering {
    right
        .memory_bytes
        .unwrap_or(0)
        .cmp(&left.memory_bytes.unwrap_or(0))
        .then_with(|| left.pid.cmp(&right.pid))
}

pub(super) fn normalize_aggregate_cpu_percent(value: f64, cpu_count: u32) -> Option<f64> {
    let normalized = value / f64::from(cpu_count);
    (value.is_finite() && cpu_count > 0 && (0.0..=100.0).contains(&normalized))
        .then_some(normalized)
}
