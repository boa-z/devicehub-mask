use std::sync::Arc;
use std::time::{Duration, Instant};

use idevice::core_device_proxy::CoreDeviceProxy;
use idevice::dvt::location_simulation::LocationSimulationClient;
use idevice::dvt::remote_server::RemoteServerClient;
use idevice::provider::IdeviceProvider;
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use idevice::{IdeviceService, ReadWrite};
use tokio::sync::{mpsc, oneshot};

use crate::protocol::{LocationStatus, LocationStatusSlot};

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

pub async fn run(
    provider: Arc<dyn IdeviceProvider>,
    commands: mpsc::Receiver<LocationCommand>,
    status: LocationStatusSlot,
) {
    let started = Instant::now();
    let stack = match connect_in_thread(provider).await {
        Ok(Ok(stack)) => {
            tracing::info!(
                component = "location",
                backend = "dvt",
                operation = "connect",
                elapsed_ms = started.elapsed().as_millis(),
                "location simulation backend connected"
            );
            stack
        }
        Ok(Err(error)) => {
            mark_failed(&status, error);
            return;
        }
        Err(_) => {
            mark_failed(&status, "DVT location connection worker stopped".into());
            return;
        }
    };

    let ConnectedStack {
        _adapter,
        _handshake,
        remote,
    } = stack;
    run_remote(remote, commands, status).await;
}

struct ConnectedStack {
    _adapter: AdapterHandle,
    _handshake: RsdHandshake,
    remote: RemoteServerClient<Box<dyn ReadWrite>>,
}

fn connect_in_thread(
    provider: Arc<dyn IdeviceProvider>,
) -> oneshot::Receiver<Result<ConnectedStack, String>> {
    let (sender, receiver) = oneshot::channel();
    let spawn = std::thread::Builder::new()
        .name("devicehub-location-connect".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            let result = match runtime {
                Ok(runtime) => runtime.block_on(async {
                    tokio::time::timeout(CONNECT_TIMEOUT, connect_stack(provider))
                        .await
                        .map_err(|_| "DVT location service connection timed out".to_string())?
                }),
                Err(error) => Err(format!("cannot start location connection worker: {error}")),
            };
            let _ = sender.send(result);
        });
    if let Err(error) = spawn {
        tracing::warn!(
            component = "location",
            backend = "dvt",
            operation = "spawn_connect",
            error = %error,
            "could not start location connection worker"
        );
    }
    receiver
}

async fn connect_stack(provider: Arc<dyn IdeviceProvider>) -> Result<ConnectedStack, String> {
    let proxy = CoreDeviceProxy::connect(&*provider)
        .await
        .map_err(|error| format!("CoreDevice proxy unavailable: {error:?}"))?;
    let rsd_port = proxy.tunnel_info().server_rsd_port;
    let adapter = proxy
        .create_software_tunnel()
        .map_err(|error| format!("software tunnel unavailable: {error:?}"))?;
    let mut adapter = adapter.to_async_handle();
    let stream = adapter
        .connect(rsd_port)
        .await
        .map_err(|error| format!("RSD connection failed: {error:?}"))?;
    let mut handshake = RsdHandshake::new(stream)
        .await
        .map_err(|error| format!("RSD handshake failed: {error:?}"))?;
    let mut remote = handshake
        .connect::<RemoteServerClient<Box<dyn ReadWrite>>>(&mut adapter)
        .await
        .map_err(|error| format!("DVT remote server unavailable: {error:?}"))?;
    remote
        .read_message(0)
        .await
        .map_err(|error| format!("DVT handshake failed: {error:?}"))?;
    Ok(ConnectedStack {
        _adapter: adapter,
        _handshake: handshake,
        remote,
    })
}

async fn run_remote(
    mut remote: RemoteServerClient<Box<dyn ReadWrite>>,
    mut commands: mpsc::Receiver<LocationCommand>,
    status: LocationStatusSlot,
) {
    let mut client =
        match tokio::time::timeout(CONNECT_TIMEOUT, LocationSimulationClient::new(&mut remote))
            .await
        {
            Ok(Ok(client)) => client,
            Ok(Err(error)) => {
                mark_failed(
                    &status,
                    format!("DVT location channel unavailable: {error:?}"),
                );
                return;
            }
            Err(_) => {
                mark_failed(&status, "DVT location channel timed out".into());
                return;
            }
        };

    status.set(LocationStatus {
        available: true,
        ..LocationStatus::default()
    });
    tracing::info!(
        component = "location",
        backend = "dvt",
        operation = "ready",
        "location simulation backend ready"
    );

    let mut current = None::<(f64, f64)>;
    let mut keepalive = tokio::time::interval_at(
        tokio::time::Instant::now() + KEEPALIVE_INTERVAL,
        KEEPALIVE_INTERVAL,
    );
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break };
                match command {
                    LocationCommand::Set { latitude, longitude, reply } => {
                        let started = Instant::now();
                        match timed_set(&mut client, latitude, longitude).await {
                            Ok(()) => {
                                current = Some((latitude, longitude));
                                keepalive.reset();
                                status.set(LocationStatus {
                                    available: true,
                                    active: true,
                                    latitude: Some(latitude),
                                    longitude: Some(longitude),
                                    error: None,
                                });
                                tracing::info!(
                                    component = "location",
                                    backend = "dvt",
                                    operation = "set",
                                    elapsed_ms = started.elapsed().as_millis(),
                                    "simulated location applied"
                                );
                                let _ = reply.send(Ok(()));
                            }
                            Err(error) => {
                                let _ = reply.send(Err(error.clone()));
                                mark_failed(&status, error);
                                return;
                            }
                        }
                    }
                    LocationCommand::Clear { reply } => {
                        let started = Instant::now();
                        match timed_clear(&mut client).await {
                            Ok(()) => {
                                current = None;
                                status.set(LocationStatus {
                                    available: true,
                                    ..LocationStatus::default()
                                });
                                tracing::info!(
                                    component = "location",
                                    backend = "dvt",
                                    operation = "clear",
                                    elapsed_ms = started.elapsed().as_millis(),
                                    "simulated location cleared"
                                );
                                let _ = reply.send(Ok(()));
                            }
                            Err(error) => {
                                let _ = reply.send(Err(error.clone()));
                                mark_failed(&status, error);
                                return;
                            }
                        }
                    }
                }
            }
            _ = keepalive.tick(), if current.is_some() => {
                let (latitude, longitude) = current.expect("guarded by select condition");
                if let Err(error) = timed_set(&mut client, latitude, longitude).await {
                    mark_failed(&status, error);
                    return;
                }
                tracing::debug!(
                    component = "location",
                    backend = "dvt",
                    operation = "keepalive",
                    "simulated location refreshed"
                );
            }
        }
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
    status.set(LocationStatus::default());
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
