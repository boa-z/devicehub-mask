//! Supervised MobileBackup2 orchestration with host-injected persistence.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devicehub_core::{ConnKind, DeviceBackupState, DeviceBackupStatus};
use idevice::mobilebackup2::MobileBackup2Client;
use idevice::provider::IdeviceProvider;
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use idevice::{IdeviceError, IdeviceService, RsdService};
use tokio::sync::{mpsc, oneshot, watch};

use crate::supervisor::ServiceReporter;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const STATUS_INTERVAL: Duration = Duration::from_millis(250);
const MAX_ERROR_BYTES: usize = 1_024;

#[derive(Clone, Default)]
pub struct DeviceBackupSlot(Arc<Mutex<DeviceBackupStatus>>);

impl DeviceBackupSlot {
    pub fn set(&self, status: DeviceBackupStatus) {
        *self.0.lock().expect("device backup status lock poisoned") = status;
    }

    pub fn update(&self, update: impl FnOnce(&mut DeviceBackupStatus)) {
        update(&mut self.0.lock().expect("device backup status lock poisoned"));
    }

    pub fn get(&self) -> DeviceBackupStatus {
        self.0
            .lock()
            .expect("device backup status lock poisoned")
            .clone()
    }

    pub fn reset(&self) {
        self.set(DeviceBackupStatus::default());
    }
}

#[derive(Debug)]
pub enum DeviceBackupCommand<Destination> {
    Start {
        destination: Destination,
        full: bool,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Stop {
        reply: oneshot::Sender<Result<(), String>>,
    },
}

pub(crate) struct DeviceBackupTransport {
    provider: Arc<dyn IdeviceProvider>,
    connection: ConnKind,
    adapter: AdapterHandle,
    handshake: RsdHandshake,
    source_identifier: String,
}

impl DeviceBackupTransport {
    pub(crate) fn new(
        provider: Arc<dyn IdeviceProvider>,
        connection: ConnKind,
        adapter: AdapterHandle,
        handshake: RsdHandshake,
        source_identifier: String,
    ) -> Self {
        Self {
            provider,
            connection,
            adapter,
            handshake,
            source_identifier,
        }
    }
}

pub type DeviceBackupPrepareFuture<'a, Prepared> =
    Pin<Box<dyn Future<Output = Result<Prepared, String>> + Send + 'a>>;
pub type DeviceBackupFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<plist::Dictionary>, IdeviceError>> + Send + 'a>>;

/// Host boundary required because idevice's MobileBackup2 API currently uses
/// a concrete filesystem path even when file operations are delegate-backed.
pub trait DeviceBackupExecutor: Clone + Send + Sync + 'static {
    type Destination: Send + 'static;
    type Prepared: Send + 'static;

    fn destination_name(&self, destination: &Self::Destination) -> Option<String>;

    fn prepare<'a>(
        &'a self,
        destination: Self::Destination,
        source_identifier: &'a str,
    ) -> DeviceBackupPrepareFuture<'a, Self::Prepared>;

    fn execute<'a>(
        &'a self,
        client: MobileBackup2Client,
        prepared: Self::Prepared,
        source_identifier: String,
        full: bool,
        status: DeviceBackupSlot,
        started: Instant,
    ) -> DeviceBackupFuture<'a>;
}

pub(crate) async fn serve<Executor>(
    mut transport: DeviceBackupTransport,
    mut commands: mpsc::Receiver<DeviceBackupCommand<Executor::Destination>>,
    status: DeviceBackupSlot,
    executor: Executor,
    reporter: ServiceReporter,
    mut shutdown: watch::Receiver<bool>,
) where
    Executor: DeviceBackupExecutor,
{
    let mut attempt = 0;
    status.reset();
    reporter.stopped(attempt);
    loop {
        let command = tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
                continue;
            }
            command = commands.recv() => command,
        };
        let Some(command) = command else { return };
        match command {
            DeviceBackupCommand::Stop { reply } => {
                let _ = reply.send(Err("no device backup is running".into()));
            }
            DeviceBackupCommand::Start {
                destination,
                full,
                reply,
            } => {
                attempt += 1;
                status.set(DeviceBackupStatus {
                    state: DeviceBackupState::Starting,
                    full,
                    destination_name: executor.destination_name(&destination),
                    ..DeviceBackupStatus::default()
                });
                reporter.connecting(attempt);
                let result = run_backup(
                    &mut transport,
                    destination,
                    full,
                    &mut commands,
                    &status,
                    &executor,
                    &reporter,
                    attempt,
                    &mut shutdown,
                    reply,
                )
                .await;
                if result == BackupRunResult::SessionEnded {
                    return;
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackupRunResult {
    Continue,
    SessionEnded,
}

#[allow(clippy::too_many_arguments)]
async fn run_backup<Executor: DeviceBackupExecutor>(
    transport: &mut DeviceBackupTransport,
    destination: Executor::Destination,
    full: bool,
    commands: &mut mpsc::Receiver<DeviceBackupCommand<Executor::Destination>>,
    status: &DeviceBackupSlot,
    executor: &Executor,
    reporter: &ServiceReporter,
    attempt: u32,
    shutdown: &mut watch::Receiver<bool>,
    reply: oneshot::Sender<Result<(), String>>,
) -> BackupRunResult {
    if let Err(error) = validate_source_identifier(&transport.source_identifier) {
        fail_start(status, reporter, attempt, error, reply);
        return BackupRunResult::Continue;
    }
    let prepared = match executor
        .prepare(destination, &transport.source_identifier)
        .await
    {
        Ok(prepared) => prepared,
        Err(error) => {
            fail_start(status, reporter, attempt, error, reply);
            return BackupRunResult::Continue;
        }
    };
    let client = match connect_client(transport).await {
        Ok(client) => client,
        Err(error) => {
            fail_start(status, reporter, attempt, error, reply);
            return BackupRunResult::Continue;
        }
    };
    let started = Instant::now();
    let mut backup = executor.execute(
        client,
        prepared,
        transport.source_identifier.clone(),
        full,
        status.clone(),
        started,
    );

    status.update(|current| {
        current.state = DeviceBackupState::BackingUp;
        current.elapsed_ms = 0;
    });
    reporter.ready(attempt);
    tracing::info!(full, "MobileBackup2 backup started");
    let _ = reply.send(Ok(()));
    let mut ticker = tokio::time::interval(STATUS_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            result = &mut backup => {
                match result.and_then(validate_final_response) {
                    Ok(()) => {
                        status.update(|current| {
                            current.state = DeviceBackupState::Completed;
                            current.progress_percent = Some(100.0);
                            current.elapsed_ms = elapsed_ms(started);
                            current.error = None;
                        });
                        reporter.stopped(attempt);
                        tracing::info!(elapsed_ms = elapsed_ms(started), "MobileBackup2 backup completed");
                    }
                    Err(error) => {
                        let error = describe_error(&error);
                        status.update(|current| {
                            current.state = DeviceBackupState::Failed;
                            current.elapsed_ms = elapsed_ms(started);
                            current.error = Some(error.clone());
                        });
                        reporter.unavailable(attempt, error.clone());
                        tracing::warn!(elapsed_ms = elapsed_ms(started), error, "MobileBackup2 backup failed");
                    }
                }
                return BackupRunResult::Continue;
            }
            _ = ticker.tick() => status.update(|current| current.elapsed_ms = elapsed_ms(started)),
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    cancel_status(status, started, "device session ended");
                    reporter.stopped(attempt);
                    return BackupRunResult::SessionEnded;
                }
            }
            command = commands.recv() => match command {
                Some(DeviceBackupCommand::Stop { reply }) => {
                    cancel_status(status, started, "cancelled by user");
                    reporter.stopped(attempt);
                    let _ = reply.send(Ok(()));
                    tracing::info!(elapsed_ms = elapsed_ms(started), "MobileBackup2 backup cancelled");
                    return BackupRunResult::Continue;
                }
                Some(DeviceBackupCommand::Start { reply, .. }) => {
                    let _ = reply.send(Err("a device backup is already running".into()));
                }
                None => {
                    cancel_status(status, started, "device session ended");
                    reporter.stopped(attempt);
                    return BackupRunResult::SessionEnded;
                }
            }
        }
    }
}

fn fail_start(
    status: &DeviceBackupSlot,
    reporter: &ServiceReporter,
    attempt: u32,
    error: String,
    reply: oneshot::Sender<Result<(), String>>,
) {
    status.update(|current| {
        current.state = DeviceBackupState::Failed;
        current.error = Some(error.clone());
    });
    reporter.unavailable(attempt, error.clone());
    let _ = reply.send(Err(error));
}

fn cancel_status(status: &DeviceBackupSlot, started: Instant, reason: &str) {
    status.update(|current| {
        current.state = DeviceBackupState::Cancelled;
        current.elapsed_ms = elapsed_ms(started);
        current.error = Some(reason.into());
    });
}

async fn connect_client(
    transport: &mut DeviceBackupTransport,
) -> Result<MobileBackup2Client, String> {
    let mut failures = Vec::new();
    if transport.connection == ConnKind::Usb {
        match tokio::time::timeout(
            CONNECT_TIMEOUT,
            MobileBackup2Client::connect(transport.provider.as_ref()),
        )
        .await
        {
            Ok(Ok(client)) => {
                tracing::info!(
                    transport = "lockdown-usb",
                    "MobileBackup2 service connected"
                );
                return Ok(client);
            }
            Ok(Err(error)) => failures.push(format!("USB lockdown: {}", describe_error(&error))),
            Err(_) => failures.push("USB lockdown: connection timed out".into()),
        }
    }
    match tokio::time::timeout(
        CONNECT_TIMEOUT,
        MobileBackup2Client::connect_rsd(&mut transport.adapter, &mut transport.handshake),
    )
    .await
    {
        Ok(Ok(client)) => {
            tracing::info!(
                transport = "coredevice-rsd",
                "MobileBackup2 service connected"
            );
            Ok(client)
        }
        Ok(Err(error)) => {
            failures.push(format!("CoreDevice RSD: {}", describe_error(&error)));
            Err(format!(
                "MobileBackup2 service unavailable: {}",
                failures.join("; ")
            ))
        }
        Err(_) => {
            failures.push("CoreDevice RSD: connection timed out".into());
            Err(format!(
                "MobileBackup2 service unavailable: {}",
                failures.join("; ")
            ))
        }
    }
}

fn validate_final_response(response: Option<plist::Dictionary>) -> Result<(), IdeviceError> {
    let Some(response) = response else {
        return Ok(());
    };
    let code = response
        .get("ErrorCode")
        .and_then(|value| {
            value
                .as_signed_integer()
                .or_else(|| value.as_unsigned_integer().map(|value| value as i64))
        })
        .unwrap_or(0);
    if code == 0 {
        return Ok(());
    }
    let description = response
        .get("ErrorDescription")
        .and_then(plist::Value::as_string)
        .unwrap_or("the device reported an unknown backup error");
    Err(IdeviceError::InternalError(format!(
        "device backup error {code}: {description}"
    )))
}

fn validate_source_identifier(source: &str) -> Result<(), String> {
    if source.is_empty()
        || source.len() > 128
        || !source
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("device identifier cannot be used as a safe backup directory name".into());
    }
    Ok(())
}

fn describe_error(error: &IdeviceError) -> String {
    let message = match error {
        IdeviceError::UnknownErrorType(message)
            if message.eq_ignore_ascii_case("ServiceProhibited") =>
        {
            "the device prohibited the MobileBackup2 service".into()
        }
        _ => format!("{error:?}"),
    };
    sanitize_message(&message)
}

fn sanitize_message(message: &str) -> String {
    let message = message.replace(['\r', '\n'], " ");
    if message.len() <= MAX_ERROR_BYTES {
        return message;
    }
    let mut end = MAX_ERROR_BYTES;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &message[..end])
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_identifiers_cannot_introduce_paths() {
        assert!(validate_source_identifier("00008110-001234567890001E").is_ok());
        assert!(validate_source_identifier("../outside").is_err());
        assert!(validate_source_identifier("").is_err());
    }

    #[test]
    fn final_device_errors_are_not_treated_as_success() {
        let mut response = plist::Dictionary::new();
        response.insert("ErrorCode".into(), plist::Value::Integer(42.into()));
        response.insert(
            "ErrorDescription".into(),
            plist::Value::String("device locked".into()),
        );
        assert!(validate_final_response(Some(response)).is_err());
        assert!(validate_final_response(None).is_ok());
    }
}
