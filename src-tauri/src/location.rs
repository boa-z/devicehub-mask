use std::time::{Duration, Instant};

use idevice::dvt::location_simulation::LocationSimulationClient;
use idevice::dvt::remote_server::RemoteServerClient;
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use idevice::{ReadWrite, RsdService};
use tokio::sync::{mpsc, oneshot, watch};

use crate::protocol::{LocationStatus, LocationStatusSlot};
use crate::supervisor::{ServiceReporter, reconnect_backoff, wait_for_retry};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const OPERATION_TIMEOUT: Duration = Duration::from_secs(5);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);
const SHUTDOWN_CLEAR_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub enum LocationCommand {
    Set {
        latitude: f64,
        longitude: f64,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Clear {
        reply: oneshot::Sender<Result<(), String>>,
    },
}

async fn connect(
    mut adapter: AdapterHandle,
    mut handshake: RsdHandshake,
) -> Result<RemoteServerClient<Box<dyn ReadWrite>>, String> {
    let started = Instant::now();
    let result = tokio::time::timeout(CONNECT_TIMEOUT, async {
        let mut remote = RemoteServerClient::connect_rsd(&mut adapter, &mut handshake).await?;
        remote.read_message(0).await?;
        Ok::<_, idevice::IdeviceError>(remote)
    })
    .await;

    match result {
        Ok(Ok(remote)) => {
            tracing::info!(
                component = "location",
                backend = "dvt",
                operation = "connect",
                elapsed_ms = started.elapsed().as_millis(),
                "location simulation backend connected"
            );
            Ok(remote)
        }
        Ok(Err(error)) => Err(format!("DVT location service unavailable: {error:?}")),
        Err(_) => Err("DVT location service connection timed out".into()),
    }
}

pub async fn supervise(
    adapter: AdapterHandle,
    handshake: RsdHandshake,
    mut commands: mpsc::Receiver<LocationCommand>,
    status: LocationStatusSlot,
    reporter: ServiceReporter,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut current = None::<(f64, f64)>;
    let mut attempt = 0;
    loop {
        if *shutdown.borrow() {
            break;
        }
        attempt += 1;
        reporter.connecting(attempt);
        let mut remote = match connect(adapter.clone(), handshake.clone()).await {
            Ok(remote) => remote,
            Err(error) => {
                mark_failed(&status, error.clone());
                reporter.retrying(attempt, error);
                if !wait_for_retry(&mut shutdown, reconnect_backoff(attempt - 1)).await {
                    break;
                }
                continue;
            }
        };
        let mut client =
            match tokio::time::timeout(CONNECT_TIMEOUT, LocationSimulationClient::new(&mut remote))
                .await
            {
                Ok(Ok(client)) => client,
                Ok(Err(error)) => {
                    let error = format!("DVT location channel unavailable: {error:?}");
                    mark_failed(&status, error.clone());
                    reporter.retrying(attempt, error);
                    if !wait_for_retry(&mut shutdown, reconnect_backoff(attempt - 1)).await {
                        break;
                    }
                    continue;
                }
                Err(_) => {
                    let error = "DVT location channel timed out".to_string();
                    mark_failed(&status, error.clone());
                    reporter.retrying(attempt, error);
                    if !wait_for_retry(&mut shutdown, reconnect_backoff(attempt - 1)).await {
                        break;
                    }
                    continue;
                }
            };

        if let Some((latitude, longitude)) = current
            && let Err(error) = timed_set(&mut client, latitude, longitude).await
        {
            mark_failed(&status, error.clone());
            reporter.retrying(attempt, error);
            if !wait_for_retry(&mut shutdown, reconnect_backoff(attempt - 1)).await {
                break;
            }
            continue;
        }
        status.set(LocationStatus {
            available: true,
            active: current.is_some(),
            latitude: current.map(|value| value.0),
            longitude: current.map(|value| value.1),
            error: None,
        });
        reporter.ready(attempt);
        tracing::info!(
            component = "location",
            backend = "dvt",
            operation = "ready",
            attempt,
            "location simulation backend ready"
        );

        let mut keepalive = tokio::time::interval_at(
            tokio::time::Instant::now() + KEEPALIVE_INTERVAL,
            KEEPALIVE_INTERVAL,
        );
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let failure = loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break None;
                    }
                }
                command = commands.recv() => {
                    let Some(command) = command else { break None };
                    match command {
                        LocationCommand::Set { latitude, longitude, reply } => {
                            let started = Instant::now();
                            match timed_set(&mut client, latitude, longitude).await {
                                Ok(()) => {
                                    current = Some((latitude, longitude));
                                    keepalive.reset();
                                    status.set(LocationStatus { available: true, active: true, latitude: Some(latitude), longitude: Some(longitude), error: None });
                                    tracing::info!(component = "location", backend = "dvt", operation = "set", elapsed_ms = started.elapsed().as_millis(), "simulated location applied");
                                    let _ = reply.send(Ok(()));
                                }
                                Err(error) => {
                                    let _ = reply.send(Err(error.clone()));
                                    break Some(error);
                                }
                            }
                        }
                        LocationCommand::Clear { reply } => match timed_clear(&mut client).await {
                            Ok(()) => {
                                current = None;
                                status.set(LocationStatus { available: true, ..LocationStatus::default() });
                                let _ = reply.send(Ok(()));
                            }
                            Err(error) => {
                                let _ = reply.send(Err(error.clone()));
                                break Some(error);
                            }
                        }
                    }
                }
                _ = keepalive.tick(), if current.is_some() => {
                    let (latitude, longitude) = current.expect("guarded by select condition");
                    if let Err(error) = timed_set(&mut client, latitude, longitude).await {
                        break Some(error);
                    }
                }
            }
        };

        if let Some(error) = failure {
            mark_failed(&status, error.clone());
            reporter.retrying(attempt, error);
            if !wait_for_retry(&mut shutdown, reconnect_backoff(attempt - 1)).await {
                break;
            }
            continue;
        }
        if current.is_some() {
            let started = Instant::now();
            let cleared = tokio::time::timeout(SHUTDOWN_CLEAR_TIMEOUT, client.clear()).await;
            tracing::info!(
                component = "location",
                backend = "dvt",
                operation = "shutdown_clear",
                success = matches!(cleared, Ok(Ok(()))),
                elapsed_ms = started.elapsed().as_millis(),
                "location simulation shutdown cleanup finished"
            );
        }
        break;
    }
    status.set(LocationStatus::default());
    reporter.stopped(attempt);
}

async fn timed_set<R: ReadWrite>(
    client: &mut LocationSimulationClient<'_, R>,
    latitude: f64,
    longitude: f64,
) -> Result<(), String> {
    tokio::time::timeout(OPERATION_TIMEOUT, client.set(latitude, longitude))
        .await
        .map_err(|_| "DVT set location timed out".to_string())?
        .map_err(|error| format!("DVT set location failed: {error:?}"))
}

async fn timed_clear<R: ReadWrite>(
    client: &mut LocationSimulationClient<'_, R>,
) -> Result<(), String> {
    tokio::time::timeout(OPERATION_TIMEOUT, client.clear())
        .await
        .map_err(|_| "DVT clear location timed out".to_string())?
        .map_err(|error| format!("DVT clear location failed: {error:?}"))
}

fn mark_failed(status: &LocationStatusSlot, error: String) {
    tracing::warn!(
        component = "location",
        backend = "dvt",
        operation = "failed",
        error = %error,
        "location simulation backend stopped"
    );
    status.set(LocationStatus {
        error: Some(error),
        ..LocationStatus::default()
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_status_is_unavailable_and_inactive() {
        let status = LocationStatusSlot::default();
        mark_failed(&status, "failed".into());
        assert_eq!(
            status.get(),
            LocationStatus {
                error: Some("failed".into()),
                ..LocationStatus::default()
            }
        );
    }
}
