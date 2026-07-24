use std::sync::Arc;
use std::time::{Duration, Instant};

use idevice::dvt::location_simulation::LocationSimulationClient;
use idevice::dvt::remote_server::RemoteServerClient;
use idevice::provider::IdeviceProvider;
use idevice::rsd::RsdHandshake;
use idevice::services::simulate_location::LocationSimulationService;
use idevice::tcp::handle::AdapterHandle;
use idevice::{IdeviceService, ReadWrite, RsdService};
use tokio::sync::{mpsc, oneshot, watch};

use crate::protocol::{LocationBackend, LocationStatus, LocationStatusSlot};
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

enum BackendExit {
    Stopped,
    Failed(String),
}

trait LocationOperations {
    fn backend(&self) -> LocationBackend;

    fn requires_keepalive(&self) -> bool {
        false
    }

    async fn set_location(&mut self, latitude: f64, longitude: f64) -> Result<(), String>;

    async fn clear_location(&mut self) -> Result<(), String>;
}

impl<R: ReadWrite> LocationOperations for LocationSimulationClient<'_, R> {
    fn backend(&self) -> LocationBackend {
        LocationBackend::Dvt
    }

    fn requires_keepalive(&self) -> bool {
        true
    }

    async fn set_location(&mut self, latitude: f64, longitude: f64) -> Result<(), String> {
        self.set(latitude, longitude)
            .await
            .map_err(|error| format!("DVT set location failed: {error:?}"))
    }

    async fn clear_location(&mut self) -> Result<(), String> {
        self.clear()
            .await
            .map_err(|error| format!("DVT clear location failed: {error:?}"))
    }
}

impl LocationOperations for LocationSimulationService {
    fn backend(&self) -> LocationBackend {
        LocationBackend::Legacy
    }

    async fn set_location(&mut self, latitude: f64, longitude: f64) -> Result<(), String> {
        let latitude = format_coordinate(latitude)?;
        let longitude = format_coordinate(longitude)?;
        self.set(&latitude, &longitude)
            .await
            .map_err(|error| format!("legacy set location failed: {error:?}"))
    }

    async fn clear_location(&mut self) -> Result<(), String> {
        self.clear()
            .await
            .map_err(|error| format!("legacy clear location failed: {error:?}"))
    }
}

async fn connect_dvt(
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

async fn run_dvt(
    transport: (AdapterHandle, RsdHandshake),
    commands: &mut mpsc::Receiver<LocationCommand>,
    status: &LocationStatusSlot,
    reporter: &ServiceReporter,
    shutdown: &mut watch::Receiver<bool>,
    current: &mut Option<(f64, f64)>,
    attempt: u32,
) -> BackendExit {
    let (adapter, handshake) = transport;
    let mut remote = match connect_dvt(adapter, handshake).await {
        Ok(remote) => remote,
        Err(error) => return BackendExit::Failed(error),
    };
    let mut client =
        match tokio::time::timeout(CONNECT_TIMEOUT, LocationSimulationClient::new(&mut remote))
            .await
        {
            Ok(Ok(client)) => client,
            Ok(Err(error)) => {
                return BackendExit::Failed(format!("DVT location channel unavailable: {error:?}"));
            }
            Err(_) => return BackendExit::Failed("DVT location channel timed out".into()),
        };
    run_backend(
        &mut client,
        commands,
        status,
        reporter,
        shutdown,
        current,
        attempt,
    )
    .await
}

async fn run_legacy(
    provider: &Arc<dyn IdeviceProvider>,
    commands: &mut mpsc::Receiver<LocationCommand>,
    status: &LocationStatusSlot,
    reporter: &ServiceReporter,
    shutdown: &mut watch::Receiver<bool>,
    current: &mut Option<(f64, f64)>,
    attempt: u32,
) -> BackendExit {
    let started = Instant::now();
    let mut client = match tokio::time::timeout(
        CONNECT_TIMEOUT,
        LocationSimulationService::connect(provider.as_ref()),
    )
    .await
    {
        Ok(Ok(client)) => client,
        Ok(Err(error)) => {
            return BackendExit::Failed(format!(
                "legacy location service unavailable: {error:?}; mount a compatible Developer Disk Image"
            ));
        }
        Err(_) => {
            return BackendExit::Failed("legacy location service connection timed out".into());
        }
    };
    tracing::info!(
        component = "location",
        backend = "legacy",
        operation = "connect",
        elapsed_ms = started.elapsed().as_millis(),
        "location simulation fallback connected"
    );
    run_backend(
        &mut client,
        commands,
        status,
        reporter,
        shutdown,
        current,
        attempt,
    )
    .await
}

async fn run_backend<B: LocationOperations>(
    client: &mut B,
    commands: &mut mpsc::Receiver<LocationCommand>,
    status: &LocationStatusSlot,
    reporter: &ServiceReporter,
    shutdown: &mut watch::Receiver<bool>,
    current: &mut Option<(f64, f64)>,
    attempt: u32,
) -> BackendExit {
    let backend = client.backend();
    let requires_keepalive = client.requires_keepalive();
    if let Some((latitude, longitude)) = *current
        && let Err(error) = timed_set(client, latitude, longitude).await
    {
        return BackendExit::Failed(error);
    }
    set_ready_status(status, backend, *current);
    reporter.ready(attempt);
    tracing::info!(
        component = "location",
        backend = backend_name(backend),
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
                        match timed_set(client, latitude, longitude).await {
                            Ok(()) => {
                                *current = Some((latitude, longitude));
                                keepalive.reset();
                                set_ready_status(status, backend, *current);
                                tracing::info!(component = "location", backend = backend_name(backend), operation = "set", elapsed_ms = started.elapsed().as_millis(), "simulated location applied");
                                let _ = reply.send(Ok(()));
                            }
                            Err(error) => {
                                let _ = reply.send(Err(error.clone()));
                                break Some(error);
                            }
                        }
                    }
                    LocationCommand::Clear { reply } => match timed_clear(client).await {
                        Ok(()) => {
                            *current = None;
                            set_ready_status(status, backend, None);
                            let _ = reply.send(Ok(()));
                        }
                        Err(error) => {
                            let _ = reply.send(Err(error.clone()));
                            break Some(error);
                        }
                    }
                }
            }
            _ = keepalive.tick(), if requires_keepalive && current.is_some() => {
                let (latitude, longitude) = current.expect("guarded by select condition");
                if let Err(error) = timed_set(client, latitude, longitude).await {
                    break Some(error);
                }
            }
        }
    };

    if let Some(error) = failure {
        return BackendExit::Failed(error);
    }
    if current.is_some() {
        let started = Instant::now();
        let cleared = tokio::time::timeout(SHUTDOWN_CLEAR_TIMEOUT, client.clear_location()).await;
        tracing::info!(
            component = "location",
            backend = backend_name(backend),
            operation = "shutdown_clear",
            success = matches!(cleared, Ok(Ok(()))),
            elapsed_ms = started.elapsed().as_millis(),
            "location simulation shutdown cleanup finished"
        );
    }
    BackendExit::Stopped
}

pub async fn supervise(
    adapter: AdapterHandle,
    handshake: RsdHandshake,
    provider: Arc<dyn IdeviceProvider>,
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
        let dvt_error = match run_dvt(
            (adapter.clone(), handshake.clone()),
            &mut commands,
            &status,
            &reporter,
            &mut shutdown,
            &mut current,
            attempt,
        )
        .await
        {
            BackendExit::Stopped => break,
            BackendExit::Failed(error) => error,
        };
        tracing::warn!(
            component = "location",
            backend = "dvt",
            operation = "fallback",
            error = %dvt_error,
            "DVT location backend failed; trying legacy service"
        );
        if *shutdown.borrow() {
            break;
        }

        let legacy_error = match run_legacy(
            &provider,
            &mut commands,
            &status,
            &reporter,
            &mut shutdown,
            &mut current,
            attempt,
        )
        .await
        {
            BackendExit::Stopped => break,
            BackendExit::Failed(error) => error,
        };
        let error = format!("{dvt_error}; {legacy_error}");
        mark_failed(&status, error.clone());
        reporter.retrying(attempt, error);
        if !wait_for_retry(&mut shutdown, reconnect_backoff(attempt - 1)).await {
            break;
        }
    }
    status.set(LocationStatus::default());
    reporter.stopped(attempt);
}

async fn timed_set<B: LocationOperations>(
    client: &mut B,
    latitude: f64,
    longitude: f64,
) -> Result<(), String> {
    let backend = backend_name(client.backend());
    tokio::time::timeout(OPERATION_TIMEOUT, client.set_location(latitude, longitude))
        .await
        .map_err(|_| format!("{backend} set location timed out"))?
}

async fn timed_clear<B: LocationOperations>(client: &mut B) -> Result<(), String> {
    let backend = backend_name(client.backend());
    tokio::time::timeout(OPERATION_TIMEOUT, client.clear_location())
        .await
        .map_err(|_| format!("{backend} clear location timed out"))?
}

fn set_ready_status(
    status: &LocationStatusSlot,
    backend: LocationBackend,
    current: Option<(f64, f64)>,
) {
    status.set(LocationStatus {
        available: true,
        active: current.is_some(),
        backend: Some(backend),
        latitude: current.map(|value| value.0),
        longitude: current.map(|value| value.1),
        error: None,
    });
}

fn backend_name(backend: LocationBackend) -> &'static str {
    match backend {
        LocationBackend::Dvt => "dvt",
        LocationBackend::Legacy => "legacy",
    }
}

fn format_coordinate(value: f64) -> Result<String, String> {
    if !value.is_finite() {
        return Err("location coordinate must be finite".into());
    }
    if value == 0.0 {
        return Ok("0".into());
    }
    let formatted = format!("{value:.8}");
    Ok(formatted
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string())
}

fn mark_failed(status: &LocationStatusSlot, error: String) {
    tracing::warn!(
        component = "location",
        operation = "failed",
        error = %error,
        "all location simulation backends stopped"
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
    fn formats_legacy_coordinates_as_bounded_locale_independent_decimals() {
        assert_eq!(format_coordinate(25.033).unwrap(), "25.033");
        assert_eq!(format_coordinate(-122.03118).unwrap(), "-122.03118");
        assert_eq!(format_coordinate(-0.0).unwrap(), "0");
        assert_eq!(format_coordinate(1.234567891).unwrap(), "1.23456789");
        assert!(format_coordinate(f64::NAN).is_err());
    }

    #[test]
    fn ready_status_identifies_the_selected_backend() {
        let status = LocationStatusSlot::default();
        set_ready_status(&status, LocationBackend::Legacy, Some((25.033, 121.5654)));
        assert_eq!(
            status.get(),
            LocationStatus {
                available: true,
                active: true,
                backend: Some(LocationBackend::Legacy),
                latitude: Some(25.033),
                longitude: Some(121.5654),
                error: None,
            }
        );
    }

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
