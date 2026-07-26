//! Per-process energy sampling and device-side subscription reconciliation.

use std::time::Duration;

use idevice::ReadWrite;
use idevice::dvt::energy_monitor::{EnergyMonitorClient, EnergySample};
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use tokio::sync::watch;

use super::source::{connect_remote, wait_until_enabled};
use super::{PerformanceSlot, update_energy};
use crate::supervisor::{ServiceReporter, reconnect_backoff, wait_for_retry};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const OPERATION_TIMEOUT: Duration = Duration::from_secs(4);

pub(crate) async fn supervise_performance_energy(
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
    let mut remote = connect_remote(adapter, handshake).await?;
    let mut client = EnergyMonitorClient::new(&mut remote)
        .await
        .map_err(|error| format!("DVT energy monitor channel failed: {error:?}"))?;
    reporter.ready(attempt);
    let mut sampled_pids = Vec::new();
    let mut tick = tokio::time::interval(SAMPLE_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    tick.tick().await;
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    stop_sampling(&mut client, &sampled_pids).await;
                    return Ok(());
                }
            }
            changed = enabled.changed() => {
                if changed.is_err() || !*enabled.borrow() {
                    stop_sampling(&mut client, &sampled_pids).await;
                    update_energy(&slot, Vec::new());
                    return Ok(());
                }
            }
            _ = tick.tick() => {
                let targets = slot.energy_targets();
                if targets != sampled_pids {
                    let removed = sampled_pids
                        .iter()
                        .copied()
                        .filter(|pid| targets.binary_search(pid).is_err())
                        .collect::<Vec<_>>();
                    let added = targets
                        .iter()
                        .copied()
                        .filter(|pid| sampled_pids.binary_search(pid).is_err())
                        .collect::<Vec<_>>();
                    stop_sampling(&mut client, &removed).await;
                    if !added.is_empty() {
                        // Clear device-side state left by an interrupted prior session.
                        stop_sampling(&mut client, &added).await;
                        tokio::time::timeout(OPERATION_TIMEOUT, client.start_sampling(&added))
                            .await
                            .map_err(|_| "DVT energy sampling setup timed out".to_string())?
                            .map_err(|error| {
                                format!("DVT energy sampling setup failed: {error:?}")
                            })?;
                    }
                    sampled_pids = targets;
                    if sampled_pids.is_empty() {
                        update_energy(&slot, Vec::new());
                    }
                }
                if !sampled_pids.is_empty() {
                    let bytes = tokio::time::timeout(
                        OPERATION_TIMEOUT,
                        client.sample_attributes(&sampled_pids),
                    )
                    .await
                    .map_err(|_| "DVT energy sample timed out".to_string())?
                    .map_err(|error| format!("DVT energy sample failed: {error:?}"))?;
                    let samples = EnergySample::from_bytes(&bytes)
                        .map_err(|error| format!("DVT energy sample decode failed: {error:?}"))?;
                    update_energy(&slot, samples);
                }
            }
        }
    }
}

async fn stop_sampling<R: ReadWrite>(client: &mut EnergyMonitorClient<'_, R>, pids: &[u32]) {
    if !pids.is_empty() {
        let _ = tokio::time::timeout(OPERATION_TIMEOUT, client.stop_sampling(pids)).await;
    }
}
