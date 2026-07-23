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

    fn update_system(&self, sample: &SysmontapSample) {
        let mut snapshot = self.0.lock().unwrap();
        snapshot.captured_at_ms = unix_millis();
        snapshot.process_count = sample.processes.as_ref().map(|value| value.len() as u32);
        snapshot.system_cpu_percent = sample
            .system_cpu_usage
            .as_ref()
            .and_then(|cpu| cpu.get("CPU_TotalLoad"))
            .and_then(numeric_value)
            .map(normalize_percent);
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
    let (process_attributes, system_attributes) = tokio::time::timeout(SETUP_TIMEOUT, async {
        let mut device_info = DeviceInfoClient::new(&mut remote).await?;
        let process = device_info.sysmon_process_attributes().await?;
        let system = device_info.sysmon_system_attributes().await?;
        Ok::<_, idevice::IdeviceError>((process, system))
    })
    .await
    .map_err(|_| "DVT sysmontap attribute query timed out".to_string())?
    .map_err(|error| format!("DVT sysmontap attribute query failed: {error:?}"))?;
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
                Ok(sample) => slot.update_system(&sample),
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

fn normalize_percent(value: f64) -> f64 {
    if value <= 1.0 { value * 100.0 } else { value }.clamp(0.0, 100.0)
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
    fn cpu_load_accepts_fractional_and_percentage_values() {
        assert_eq!(normalize_percent(0.42), 42.0);
        assert_eq!(normalize_percent(42.0), 42.0);
        assert_eq!(normalize_percent(140.0), 100.0);
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
