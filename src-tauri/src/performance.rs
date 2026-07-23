//! Supervised DVT performance sampling over the active CoreDevice tunnel.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use idevice::dvt::device_info::DeviceInfoClient;
use idevice::dvt::graphics::GraphicsClient;
use idevice::dvt::remote_server::RemoteServerClient;
use idevice::dvt::sysmontap::{SysmontapClient, SysmontapConfig, SysmontapSample};
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use idevice::{ReadWrite, RsdService};
use plist::Value;
use serde::Serialize;
use tokio::sync::watch;

use crate::supervisor::{ServiceReporter, reconnect_backoff, wait_for_retry};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
const SETUP_TIMEOUT: Duration = Duration::from_secs(8);
const SAMPLE_INTERVAL_MS: u32 = 1_000;

#[derive(Debug, Clone, Default, Serialize)]
pub struct PerformanceSnapshot {
    pub captured_at_ms: u64,
    pub system_cpu_percent: Option<f64>,
    pub process_count: Option<u32>,
    pub graphics_fps: Option<f64>,
    pub gpu_allocated_bytes: Option<u64>,
    pub gpu_in_use_bytes: Option<u64>,
    pub gpu_driver_bytes: Option<u64>,
    pub gpu_recovery_count: Option<u64>,
}

#[derive(Clone, Default)]
pub struct PerformanceSlot(Arc<Mutex<PerformanceSnapshot>>);

impl PerformanceSlot {
    pub fn get(&self) -> PerformanceSnapshot {
        self.0.lock().unwrap().clone()
    }

    pub fn reset(&self) {
        *self.0.lock().unwrap() = PerformanceSnapshot::default();
    }

    fn update_system(&self, sample: &SysmontapSample, cpu_count: u32) {
        let mut snapshot = self.0.lock().unwrap();
        snapshot.captured_at_ms = unix_millis();
        if let Some(processes) = sample.processes.as_ref() {
            snapshot.process_count = Some(processes.len() as u32);
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

    fn update_graphics(&self, sample: &idevice::dvt::graphics::GraphicsSample) {
        let mut snapshot = self.0.lock().unwrap();
        snapshot.captured_at_ms = unix_millis();
        snapshot.graphics_fps = sample.fps.is_finite().then_some(sample.fps.max(0.0));
        snapshot.gpu_allocated_bytes = Some(sample.alloc_system_memory);
        snapshot.gpu_in_use_bytes = Some(sample.in_use_system_memory);
        snapshot.gpu_driver_bytes = Some(sample.in_use_system_memory_driver);
        snapshot.gpu_recovery_count = Some(sample.recovery_count);
    }
}

#[derive(Clone)]
pub struct PerformanceDemand(watch::Sender<bool>);

impl Default for PerformanceDemand {
    fn default() -> Self {
        let (sender, _) = watch::channel(false);
        Self(sender)
    }
}

impl PerformanceDemand {
    pub fn set(&self, enabled: bool) {
        self.0.send_replace(enabled);
    }

    pub fn enabled(&self) -> bool {
        *self.0.borrow()
    }

    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.0.subscribe()
    }
}

pub async fn supervise_system(
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
        let result = run_system_once(
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
        let Some(error) = result.err() else { continue };
        reporter.retrying(attempt, error);
        if !wait_for_retry(&mut shutdown, reconnect_backoff(attempt - 1)).await {
            break;
        }
    }
    reporter.stopped(attempt);
}

pub async fn supervise_graphics(
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
        let result = run_graphics_once(
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
        let Some(error) = result.err() else { continue };
        reporter.retrying(attempt, error);
        if !wait_for_retry(&mut shutdown, reconnect_backoff(attempt - 1)).await {
            break;
        }
    }
    reporter.stopped(attempt);
}

async fn run_system_once(
    adapter: AdapterHandle,
    handshake: RsdHandshake,
    slot: PerformanceSlot,
    shutdown: &mut watch::Receiver<bool>,
    enabled: &mut watch::Receiver<bool>,
    reporter: &ServiceReporter,
    attempt: u32,
) -> Result<(), String> {
    let mut remote = connect_remote(adapter, handshake).await?;
    let (process_attributes, system_attributes, cpu_count) =
        tokio::time::timeout(SETUP_TIMEOUT, async {
            let mut device_info = DeviceInfoClient::new(&mut remote).await?;
            let process = device_info.sysmon_process_attributes().await?;
            let system = device_info.sysmon_system_attributes().await?;
            let hardware = device_info.hardware_information().await?;
            Ok::<_, idevice::IdeviceError>((process, system, cpu_count(&hardware)))
        })
        .await
        .map_err(|_| "DVT sysmontap attribute query timed out".to_string())?
        .map_err(|error| format!("DVT sysmontap attribute query failed: {error:?}"))?;
    let cpu_count = cpu_count.ok_or_else(|| {
        "DVT hardware information did not report a valid logical CPU count".to_string()
    })?;
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
                Ok(sample) => slot.update_system(&sample, cpu_count),
                Err(error) => return Err(format!("DVT sysmontap stream failed: {error:?}")),
            }
        }
    }
}

async fn run_graphics_once(
    adapter: AdapterHandle,
    handshake: RsdHandshake,
    slot: PerformanceSlot,
    shutdown: &mut watch::Receiver<bool>,
    enabled: &mut watch::Receiver<bool>,
    reporter: &ServiceReporter,
    attempt: u32,
) -> Result<(), String> {
    let mut remote = connect_remote(adapter, handshake).await?;
    let mut client = GraphicsClient::new(&mut remote)
        .await
        .map_err(|error| format!("DVT graphics channel failed: {error:?}"))?;
    tokio::time::timeout(SETUP_TIMEOUT, client.start_sampling(1.0))
        .await
        .map_err(|_| "DVT graphics setup timed out".to_string())?
        .map_err(|error| format!("DVT graphics setup failed: {error:?}"))?;
    reporter.ready(attempt);
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    let _ = client.stop_sampling().await;
                    return Ok(());
                }
            }
            changed = enabled.changed() => {
                if changed.is_err() || !*enabled.borrow() {
                    let _ = client.stop_sampling().await;
                    return Ok(());
                }
            }
            sample = client.sample() => match sample {
                Ok(sample) => slot.update_graphics(&sample),
                Err(error) => return Err(format!("DVT graphics stream failed: {error:?}")),
            }
        }
    }
}

async fn connect_remote(
    mut adapter: AdapterHandle,
    mut handshake: RsdHandshake,
) -> Result<RemoteServerClient<Box<dyn ReadWrite + 'static>>, String> {
    tokio::time::timeout(
        CONNECT_TIMEOUT,
        RemoteServerClient::connect_rsd(&mut adapter, &mut handshake),
    )
    .await
    .map_err(|_| "DVT performance connection timed out".to_string())?
    .map_err(|error| format!("DVT performance connection failed: {error:?}"))
}

fn numeric_value(value: &Value) -> Option<f64> {
    match value {
        Value::Real(value) => Some(*value),
        Value::Integer(value) => value
            .as_signed()
            .map(|value| value as f64)
            .or_else(|| value.as_unsigned().map(|value| value as f64)),
        _ => None,
    }
}

fn cpu_count(hardware: &plist::Dictionary) -> Option<u32> {
    ["numberOfCpus", "numberOfPhysicalCpus"]
        .into_iter()
        .filter_map(|key| hardware.get(key))
        .filter_map(numeric_u32)
        .find(|count| (1..=256).contains(count))
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

fn normalize_aggregate_cpu_percent(value: f64, cpu_count: u32) -> Option<f64> {
    let normalized = value / f64::from(cpu_count);
    (value.is_finite() && cpu_count > 0 && (0.0..=100.0).contains(&normalized))
        .then_some(normalized)
}

async fn wait_until_enabled(
    enabled: &mut watch::Receiver<bool>,
    shutdown: &mut watch::Receiver<bool>,
    reporter: &ServiceReporter,
    attempt: u32,
) -> bool {
    while !*enabled.borrow() {
        reporter.stopped(attempt);
        tokio::select! {
            changed = enabled.changed() => {
                if changed.is_err() {
                    return false;
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return false;
                }
            }
        }
    }
    true
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
    fn aggregate_cpu_load_is_normalized_by_device_cpu_count() {
        assert_eq!(normalize_aggregate_cpu_percent(240.0, 6), Some(40.0));
        assert_eq!(normalize_aggregate_cpu_percent(600.0, 6), Some(100.0));
        assert_eq!(normalize_aggregate_cpu_percent(601.0, 6), None);
        assert_eq!(normalize_aggregate_cpu_percent(42.0, 0), None);
        assert_eq!(normalize_aggregate_cpu_percent(f64::NAN, 6), None);
    }

    #[test]
    fn logical_cpu_count_falls_back_to_physical_count() {
        let mut hardware = plist::Dictionary::new();
        hardware.insert("numberOfPhysicalCpus".into(), Value::Integer(6.into()));
        assert_eq!(cpu_count(&hardware), Some(6));

        hardware.insert("numberOfCpus".into(), Value::Integer(8.into()));
        assert_eq!(cpu_count(&hardware), Some(8));

        hardware.insert("numberOfCpus".into(), Value::Integer(0.into()));
        assert_eq!(cpu_count(&hardware), Some(6));
    }

    #[test]
    fn partial_system_samples_preserve_the_latest_metrics() {
        let slot = PerformanceSlot::default();
        let mut cpu = plist::Dictionary::new();
        cpu.insert("CPU_TotalLoad".into(), Value::Real(240.0));
        let mut processes = plist::Dictionary::new();
        processes.insert("1".into(), Value::Array(Vec::new()));
        slot.update_system(
            &SysmontapSample {
                processes: Some(processes),
                system: None,
                system_cpu_usage: Some(cpu),
            },
            6,
        );
        slot.update_system(
            &SysmontapSample {
                processes: None,
                system: None,
                system_cpu_usage: None,
            },
            6,
        );

        let snapshot = slot.get();
        assert_eq!(snapshot.system_cpu_percent, Some(40.0));
        assert_eq!(snapshot.process_count, Some(1));
    }

    #[test]
    fn performance_slot_merges_independent_sources() {
        let slot = PerformanceSlot::default();
        slot.update_graphics(&idevice::dvt::graphics::GraphicsSample {
            timestamp: 1,
            fps: 59.5,
            alloc_system_memory: 10,
            in_use_system_memory: 8,
            in_use_system_memory_driver: 3,
            gpu_bundle_name: "Built-In".into(),
            recovery_count: 0,
        });
        let snapshot = slot.get();
        assert_eq!(snapshot.graphics_fps, Some(59.5));
        assert_eq!(snapshot.gpu_in_use_bytes, Some(8));
        assert!(snapshot.captured_at_ms > 0);
    }
}
