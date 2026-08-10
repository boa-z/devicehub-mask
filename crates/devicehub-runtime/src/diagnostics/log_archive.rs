//! User-initiated, bounded unified-log archive export.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use devicehub_core::{
    LogArchiveSlot, LogArchiveState, LogArchiveStatus, ManagedOperationError, ManagedOperationKind,
    ManagedOperationRegistry, OperationErrorCode,
};
use idevice::RsdService;
use idevice::os_trace_relay::OsTraceRelayClient;
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch};

use crate::HostFileIo;
use crate::supervisor::ServiceReporter;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_DURATION: Duration = Duration::from_secs(10 * 60);
const STATUS_INTERVAL: Duration = Duration::from_millis(250);
const MAX_ERROR_BYTES: usize = 1_024;
const REQUESTED_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const TAR_BLOCK_BYTES: u64 = 512;
const TAR_END_BYTES: usize = 1_024;
pub const ALLOWED_LOG_ARCHIVE_AGE_LIMIT_HOURS: [u16; 3] = [1, 6, 24];

#[derive(Debug)]
pub enum LogArchiveCommand<Destination> {
    Start {
        destination: Destination,
        age_limit_hours: u16,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Stop {
        reply: oneshot::Sender<Result<(), String>>,
    },
}

pub fn validate_age_limit_hours(value: u16) -> Result<u16, String> {
    ALLOWED_LOG_ARCHIVE_AGE_LIMIT_HOURS
        .contains(&value)
        .then_some(value)
        .ok_or_else(|| "log archive age limit must be 1, 6, or 24 hours".into())
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn serve<FileIo>(
    mut adapter: AdapterHandle,
    mut handshake: RsdHandshake,
    mut commands: mpsc::Receiver<LogArchiveCommand<FileIo::Path>>,
    status: LogArchiveSlot,
    operations: ManagedOperationRegistry,
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
                if changed.is_err() || *shutdown.borrow() { return; }
                continue;
            }
            command = commands.recv() => command,
        };
        let Some(command) = command else { return };
        match command {
            LogArchiveCommand::Stop { reply } => {
                let _ = reply.send(Err("no log archive export is running".into()));
            }
            LogArchiveCommand::Start {
                destination,
                age_limit_hours,
                reply,
            } => {
                let managed_id = match operations.begin(
                    ManagedOperationKind::LogArchive,
                    Some(format!("{age_limit_hours}h")),
                    true,
                ) {
                    Ok(id) => id,
                    Err(error) => {
                        let _ = reply.send(Err(error.message));
                        continue;
                    }
                };
                operations.update(managed_id, Some("starting".into()), Some(0.0));
                attempt += 1;
                let outcome = run_export(
                    &mut adapter,
                    &mut handshake,
                    destination,
                    age_limit_hours,
                    &mut commands,
                    &status,
                    &file_io,
                    &reporter,
                    attempt,
                    &mut shutdown,
                    reply,
                )
                .await;
                match status.get() {
                    current if current.state == LogArchiveState::Completed => {
                        operations.succeed(managed_id)
                    }
                    current if current.state == LogArchiveState::Cancelled => operations.cancel(
                        managed_id,
                        current.error.as_deref().unwrap_or("log archive cancelled"),
                    ),
                    current => operations.fail(
                        managed_id,
                        ManagedOperationError::new(
                            OperationErrorCode::Internal,
                            current.error.unwrap_or_else(|| "log archive failed".into()),
                        ),
                    ),
                }
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
    age_limit_hours: u16,
    commands: &mut mpsc::Receiver<LogArchiveCommand<FileIo::Path>>,
    status: &LogArchiveSlot,
    file_io: &FileIo,
    reporter: &ServiceReporter,
    attempt: u32,
    shutdown: &mut watch::Receiver<bool>,
    reply: oneshot::Sender<Result<(), String>>,
) -> ExportOutcome
where
    FileIo: HostFileIo,
{
    let age_limit_hours = match validate_age_limit_hours(age_limit_hours) {
        Ok(value) => value,
        Err(error) => {
            fail_start(status, reporter, attempt, error, reply);
            return ExportOutcome::Continue;
        }
    };
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
    let temporary = match file_io.temporary_sibling(&destination, "log-archive") {
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
                format!("unable to create log archive export file: {error}"),
                reply,
            );
            return ExportOutcome::Continue;
        }
    };

    let started = Instant::now();
    status.set(LogArchiveStatus {
        state: LogArchiveState::Starting,
        destination_name: Some(destination_name),
        age_limit_hours: Some(age_limit_hours),
        ..LogArchiveStatus::default()
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
                OsTraceRelayClient::connect_rsd(adapter, handshake),
            )
            .await
            .map_err(|_| "unified log archive service connection timed out".to_string())?
            .map_err(|error| format!("unified log archive service unavailable: {error:?}"))?;
            task_reporter.ready(attempt);
            task_status.update(|current| current.state = LogArchiveState::Exporting);

            let cutoff = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| "system clock is before the Unix epoch".to_string())?
                .as_secs()
                .saturating_sub(u64::from(age_limit_hours) * 60 * 60);
            let mut writer = BoundedArchiveWriter::new(file, task_status.clone());
            client
                .create_archive(
                    &mut writer,
                    Some(REQUESTED_ARCHIVE_BYTES),
                    None,
                    Some(cutoff),
                )
                .await
                .map_err(|error| format!("unified log archive stream failed: {error:?}"))?;
            writer.validate_complete()?;
            writer
                .flush()
                .await
                .map_err(|error| format!("unable to flush log archive data: {error}"))?;
            let written = writer.written;
            let file = writer.file;
            file.sync()
                .await
                .map_err(|error| format!("unable to synchronize log archive data: {error}"))?;
            file_io
                .replace_file(&task_temporary, &task_destination)
                .await?;
            Ok::<u64, String>(written)
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
                                current.state = LogArchiveState::Completed;
                                current.bytes_written = written;
                                current.elapsed_ms = elapsed_ms(started);
                                current.error = None;
                            });
                            reporter.stopped(attempt);
                            tracing::info!(bytes = written, age_limit_hours, elapsed_ms = elapsed_ms(started), "unified log archive export completed");
                        }
                        Err(error) => {
                            let error = sanitize_message(&error);
                            status.update(|current| {
                                current.state = LogArchiveState::Failed;
                                current.elapsed_ms = elapsed_ms(started);
                                current.error = Some(error.clone());
                            });
                            reporter.unavailable(attempt, error.clone());
                            tracing::warn!(elapsed_ms = elapsed_ms(started), error, "unified log archive export failed");
                        }
                    }
                    break ExportOutcome::Continue;
                }
                _ = ticker.tick() => {
                    status.update(|current| current.elapsed_ms = elapsed_ms(started));
                }
                _ = &mut deadline => {
                    let error = "log archive export exceeded the 10 minute limit".to_string();
                    status.update(|current| {
                        current.state = LogArchiveState::Failed;
                        current.elapsed_ms = elapsed_ms(started);
                        current.error = Some(error.clone());
                    });
                    reporter.unavailable(attempt, error.clone());
                    tracing::warn!(error, "unified log archive export timed out");
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
                    Some(LogArchiveCommand::Stop { reply }) => {
                        cancel_status(status, started, "cancelled by user");
                        reporter.stopped(attempt);
                        let _ = reply.send(Ok(()));
                        tracing::info!(elapsed_ms = elapsed_ms(started), "unified log archive export cancelled");
                        break ExportOutcome::Continue;
                    }
                    Some(LogArchiveCommand::Start { reply, .. }) => {
                        let _ = reply.send(Err("a log archive export is already running".into()));
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
    if status.get().state != LogArchiveState::Completed {
        let _ = file_io.remove_file(&temporary).await;
    }
    outcome
}

struct BoundedArchiveWriter<W> {
    file: W,
    status: LogArchiveSlot,
    written: u64,
    last_status: Instant,
    tail: [u8; TAR_END_BYTES],
    tail_len: usize,
}

impl<W> BoundedArchiveWriter<W> {
    fn new(file: W, status: LogArchiveSlot) -> Self {
        Self {
            file,
            status,
            written: 0,
            last_status: Instant::now(),
            tail: [0; TAR_END_BYTES],
            tail_len: 0,
        }
    }

    fn record(&mut self, bytes: &[u8]) {
        if bytes.len() >= TAR_END_BYTES {
            self.tail
                .copy_from_slice(&bytes[bytes.len() - TAR_END_BYTES..]);
            self.tail_len = TAR_END_BYTES;
        } else {
            let retained = self.tail_len.min(TAR_END_BYTES - bytes.len());
            self.tail
                .copy_within(self.tail_len - retained..self.tail_len, 0);
            self.tail[retained..retained + bytes.len()].copy_from_slice(bytes);
            self.tail_len = retained + bytes.len();
        }
    }

    fn validate_complete(&self) -> Result<(), String> {
        if self.written == 0 {
            return Err("device returned an empty log archive".into());
        }
        if !self.written.is_multiple_of(TAR_BLOCK_BYTES)
            || self.tail_len < TAR_END_BYTES
            || self.tail[..self.tail_len].iter().any(|byte| *byte != 0)
        {
            return Err("device returned an incomplete log archive".into());
        }
        Ok(())
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for BoundedArchiveWriter<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        let remaining = MAX_ARCHIVE_BYTES.saturating_sub(self.written);
        if buffer.len() as u64 > remaining {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "log archive exceeded the 512 MiB application limit",
            )));
        }
        match Pin::new(&mut self.file).poll_write(cx, buffer) {
            Poll::Ready(Ok(written)) => {
                self.written = self.written.saturating_add(written as u64);
                self.record(&buffer[..written]);
                if self.last_status.elapsed() >= STATUS_INTERVAL {
                    self.status
                        .update(|current| current.bytes_written = self.written);
                    self.last_status = Instant::now();
                }
                Poll::Ready(Ok(written))
            }
            other => other,
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.file).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.file).poll_shutdown(cx)
    }
}

fn fail_start(
    status: &LogArchiveSlot,
    reporter: &ServiceReporter,
    attempt: u32,
    error: String,
    reply: oneshot::Sender<Result<(), String>>,
) {
    let error = sanitize_message(&error);
    status.set(LogArchiveStatus {
        state: LogArchiveState::Failed,
        error: Some(error.clone()),
        ..LogArchiveStatus::default()
    });
    reporter.unavailable(attempt, error.clone());
    let _ = reply.send(Err(error));
}

fn cancel_status(status: &LogArchiveSlot, started: Instant, reason: &str) {
    status.update(|current| {
        current.state = LogArchiveState::Cancelled;
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
    fn age_limits_are_validated() {
        assert_eq!(validate_age_limit_hours(6).unwrap(), 6);
        assert!(validate_age_limit_hours(2).is_err());
    }

    #[tokio::test]
    async fn bounded_writer_accepts_only_complete_tar_streams() {
        let mut complete = BoundedArchiveWriter::new(Vec::new(), LogArchiveSlot::default());
        complete.write_all(&vec![0; TAR_END_BYTES]).await.unwrap();
        assert!(complete.validate_complete().is_ok());

        let mut partial = BoundedArchiveWriter::new(Vec::new(), LogArchiveSlot::default());
        partial.write_all(b"partial").await.unwrap();
        assert!(partial.validate_complete().is_err());
    }

    #[test]
    fn errors_are_single_line_and_bounded() {
        let message = format!("{}\nprivate", "x".repeat(MAX_ERROR_BYTES + 20));
        let sanitized = sanitize_message(&message);
        assert!(!sanitized.contains('\n'));
        assert!(sanitized.len() <= MAX_ERROR_BYTES + 3);
    }
}
