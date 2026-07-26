//! User-initiated, cancellable CoreDevice sysdiagnose export.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devicehub_core::{SysdiagnoseState, SysdiagnoseStatus};
use futures_util::StreamExt;
use idevice::RsdService;
use idevice::core_device::DiagnostisServiceClient;
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot, watch};

use crate::HostFileIo;
use crate::supervisor::ServiceReporter;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_DURATION: Duration = Duration::from_secs(45 * 60);
const STATUS_INTERVAL: Duration = Duration::from_millis(250);
const MAX_ERROR_BYTES: usize = 1_024;
const MAX_CHUNK_BYTES: usize = 16 * 1024 * 1024;
const MAX_SYSDIAGNOSE_BYTES: u64 = 8 * 1024 * 1024 * 1024;

#[derive(Clone, Default)]
pub struct SysdiagnoseSlot(Arc<Mutex<SysdiagnoseStatus>>);

impl SysdiagnoseSlot {
    pub fn set(&self, status: SysdiagnoseStatus) {
        *self.0.lock().expect("sysdiagnose status lock poisoned") = status;
    }

    pub fn update(&self, update: impl FnOnce(&mut SysdiagnoseStatus)) {
        update(&mut self.0.lock().expect("sysdiagnose status lock poisoned"));
    }

    pub fn get(&self) -> SysdiagnoseStatus {
        self.0
            .lock()
            .expect("sysdiagnose status lock poisoned")
            .clone()
    }

    pub fn reset(&self) {
        self.set(SysdiagnoseStatus::default());
    }
}

#[derive(Debug)]
pub enum SysdiagnoseCommand<Destination> {
    Start {
        destination: Destination,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Stop {
        reply: oneshot::Sender<Result<(), String>>,
    },
}

pub(crate) async fn serve<FileIo>(
    mut adapter: AdapterHandle,
    mut handshake: RsdHandshake,
    mut commands: mpsc::Receiver<SysdiagnoseCommand<FileIo::Path>>,
    status: SysdiagnoseSlot,
    file_io: FileIo,
    reporter: ServiceReporter,
    mut shutdown: watch::Receiver<bool>,
) where
    FileIo: HostFileIo,
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
            SysdiagnoseCommand::Stop { reply } => {
                let _ = reply.send(Err("no sysdiagnose export is running".into()));
            }
            SysdiagnoseCommand::Start { destination, reply } => {
                attempt += 1;
                let outcome = run_export(
                    &mut adapter,
                    &mut handshake,
                    destination,
                    &mut commands,
                    &status,
                    &file_io,
                    &reporter,
                    attempt,
                    &mut shutdown,
                    reply,
                )
                .await;
                if outcome == ExportOutcome::SessionEnded {
                    return;
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExportOutcome {
    Continue,
    SessionEnded,
}

#[allow(clippy::too_many_arguments)]
async fn run_export<FileIo>(
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
    destination: FileIo::Path,
    commands: &mut mpsc::Receiver<SysdiagnoseCommand<FileIo::Path>>,
    status: &SysdiagnoseSlot,
    file_io: &FileIo,
    reporter: &ServiceReporter,
    attempt: u32,
    shutdown: &mut watch::Receiver<bool>,
    reply: oneshot::Sender<Result<(), String>>,
) -> ExportOutcome
where
    FileIo: HostFileIo,
{
    if let Err(error) = file_io.validate_export_file(&destination).await {
        fail_start(status, reporter, attempt, error, reply);
        return ExportOutcome::Continue;
    }
    let destination_name = match file_io.file_name(&destination) {
        Ok(name) => name.chars().take(255).collect(),
        Err(error) => {
            fail_start(status, reporter, attempt, error, reply);
            return ExportOutcome::Continue;
        }
    };
    let temporary = match file_io.temporary_sibling(&destination, "sysdiagnose") {
        Ok(path) => path,
        Err(error) => {
            fail_start(status, reporter, attempt, error, reply);
            return ExportOutcome::Continue;
        }
    };
    let file = match file_io.create_new_writer(&temporary).await {
        Ok(file) => file,
        Err(error) => {
            fail_start(
                status,
                reporter,
                attempt,
                format!("unable to create sysdiagnose export file: {error}"),
                reply,
            );
            return ExportOutcome::Continue;
        }
    };
    let started = Instant::now();
    status.set(SysdiagnoseStatus {
        state: SysdiagnoseState::Starting,
        destination_name: Some(destination_name),
        ..SysdiagnoseStatus::default()
    });
    reporter.connecting(attempt);
    let _ = reply.send(Ok(()));

    let task_status = status.clone();
    let task_reporter = reporter.clone();
    let task_destination = destination.clone();
    let task_temporary = temporary.clone();
    let outcome = {
        let task = async move {
            let mut client = tokio::time::timeout(
                CONNECT_TIMEOUT,
                DiagnostisServiceClient::connect_rsd(adapter, handshake),
            )
            .await
            .map_err(|_| "CoreDevice DiagnosticsService connection timed out".to_string())?
            .map_err(|error| format!("CoreDevice DiagnosticsService unavailable: {error:?}"))?;
            task_reporter.ready(attempt);
            task_status.update(|current| current.state = SysdiagnoseState::Collecting);
            let mut response = client
                .capture_sysdiagnose(false)
                .await
                .map_err(|error| format!("unable to collect sysdiagnose: {error:?}"))?;
            let expected = response.expected_length as u64;
            if expected == 0 || expected > MAX_SYSDIAGNOSE_BYTES {
                return Err(format!(
                    "device reported an invalid sysdiagnose size: {expected} bytes"
                ));
            }
            task_status.update(|current| {
                current.state = SysdiagnoseState::Downloading;
                current.bytes_total = expected;
                current.progress_percent = Some(0.0);
            });
            let mut file = file;
            let mut written = 0u64;
            while let Some(chunk) = response.stream.next().await {
                let chunk =
                    chunk.map_err(|error| format!("sysdiagnose stream failed: {error:?}"))?;
                if chunk.len() > MAX_CHUNK_BYTES {
                    return Err("device returned an oversized sysdiagnose chunk".into());
                }
                let next = written
                    .checked_add(chunk.len() as u64)
                    .ok_or_else(|| "sysdiagnose byte count overflowed".to_string())?;
                if next > expected || next > MAX_SYSDIAGNOSE_BYTES {
                    return Err("device returned more sysdiagnose data than declared".into());
                }
                file.write_all(&chunk)
                    .await
                    .map_err(|error| format!("unable to write sysdiagnose data: {error}"))?;
                written = next;
                task_status.update(|current| {
                    current.bytes_written = written;
                    current.progress_percent = Some(written as f64 * 100.0 / expected as f64);
                });
            }
            if written != expected {
                return Err(format!(
                    "sysdiagnose stream ended after {written} of {expected} bytes"
                ));
            }
            drop(response);
            file.flush()
                .await
                .map_err(|error| format!("unable to flush sysdiagnose data: {error}"))?;
            file.sync()
                .await
                .map_err(|error| format!("unable to synchronize sysdiagnose data: {error}"))?;
            file_io
                .replace_file(&task_temporary, &task_destination)
                .await?;
            Ok(written)
        };
        tokio::pin!(task);
        let mut ticker = tokio::time::interval(STATUS_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let deadline = tokio::time::sleep(MAX_DURATION);
        tokio::pin!(deadline);

        loop {
            tokio::select! {
            biased;
            result = &mut task => {
                match result {
                    Ok(written) => {
                        status.update(|current| {
                            current.state = SysdiagnoseState::Completed;
                            current.bytes_written = written;
                            current.progress_percent = Some(100.0);
                            current.elapsed_ms = elapsed_ms(started);
                            current.error = None;
                        });
                        reporter.stopped(attempt);
                        tracing::info!(bytes = written, elapsed_ms = elapsed_ms(started), "sysdiagnose export completed");
                    }
                    Err(error) => {
                        let error = sanitize_message(&error);
                        status.update(|current| {
                            current.state = SysdiagnoseState::Failed;
                            current.elapsed_ms = elapsed_ms(started);
                            current.error = Some(error.clone());
                        });
                        reporter.unavailable(attempt, error.clone());
                        tracing::warn!(elapsed_ms = elapsed_ms(started), error, "sysdiagnose export failed");
                    }
                }
                break ExportOutcome::Continue;
            }
            _ = ticker.tick() => {
                status.update(|current| current.elapsed_ms = elapsed_ms(started));
            }
            _ = &mut deadline => {
                let error = "sysdiagnose export exceeded the 45 minute limit".to_string();
                status.update(|current| {
                    current.state = SysdiagnoseState::Failed;
                    current.elapsed_ms = elapsed_ms(started);
                    current.error = Some(error.clone());
                });
                reporter.unavailable(attempt, error.clone());
                tracing::warn!(error, "sysdiagnose export timed out");
                break ExportOutcome::Continue;
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    cancel_status(status, started, "device session ended");
                    reporter.stopped(attempt);
                    break ExportOutcome::SessionEnded;
                }
            }
            command = commands.recv() => match command {
                Some(SysdiagnoseCommand::Stop { reply }) => {
                    cancel_status(status, started, "cancelled by user");
                    reporter.stopped(attempt);
                    let _ = reply.send(Ok(()));
                    tracing::info!(elapsed_ms = elapsed_ms(started), "sysdiagnose export cancelled");
                    break ExportOutcome::Continue;
                }
                Some(SysdiagnoseCommand::Start { reply, .. }) => {
                    let _ = reply.send(Err("a sysdiagnose export is already running".into()));
                }
                None => {
                    cancel_status(status, started, "device session ended");
                    reporter.stopped(attempt);
                    break ExportOutcome::SessionEnded;
                }
            }
            }
        }
    };
    if status.get().state != SysdiagnoseState::Completed {
        let _ = file_io.remove_file(&temporary).await;
    }
    outcome
}

fn fail_start(
    status: &SysdiagnoseSlot,
    reporter: &ServiceReporter,
    attempt: u32,
    error: String,
    reply: oneshot::Sender<Result<(), String>>,
) {
    let error = sanitize_message(&error);
    status.set(SysdiagnoseStatus {
        state: SysdiagnoseState::Failed,
        error: Some(error.clone()),
        ..SysdiagnoseStatus::default()
    });
    reporter.unavailable(attempt, error.clone());
    let _ = reply.send(Err(error));
}

fn cancel_status(status: &SysdiagnoseSlot, started: Instant, reason: &str) {
    status.update(|current| {
        current.state = SysdiagnoseState::Cancelled;
        current.elapsed_ms = elapsed_ms(started);
        current.error = Some(reason.into());
    });
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
    fn errors_are_single_line_and_bounded() {
        let message = format!("{}\nprivate", "x".repeat(MAX_ERROR_BYTES + 20));
        let sanitized = sanitize_message(&message);
        assert!(!sanitized.contains('\n'));
        assert!(sanitized.len() <= MAX_ERROR_BYTES + 3);
    }
}
