//! User-initiated, bounded unified-log archive export.

use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use idevice::RsdService;
use idevice::os_trace_relay::OsTraceRelayClient;
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use serde::Serialize;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch};

use crate::supervisor::ServiceReporter;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_DURATION: Duration = Duration::from_secs(10 * 60);
const STATUS_INTERVAL: Duration = Duration::from_millis(250);
const MAX_PATH_BYTES: usize = 4_096;
const MAX_ERROR_BYTES: usize = 1_024;
const REQUESTED_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const TAR_BLOCK_BYTES: u64 = 512;
const TAR_END_BYTES: usize = 1_024;
pub const ALLOWED_AGE_LIMIT_HOURS: [u16; 3] = [1, 6, 24];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogArchiveState {
    #[default]
    Idle,
    Starting,
    Exporting,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LogArchiveStatus {
    pub state: LogArchiveState,
    pub bytes_written: u64,
    pub elapsed_ms: u64,
    pub destination_name: Option<String>,
    pub age_limit_hours: Option<u16>,
    pub error: Option<String>,
}

#[derive(Clone, Default)]
pub struct LogArchiveSlot(Arc<Mutex<LogArchiveStatus>>);

impl LogArchiveSlot {
    pub fn set(&self, status: LogArchiveStatus) {
        *self.0.lock().expect("log archive status lock poisoned") = status;
    }

    pub fn update(&self, update: impl FnOnce(&mut LogArchiveStatus)) {
        update(&mut self.0.lock().expect("log archive status lock poisoned"));
    }

    pub fn get(&self) -> LogArchiveStatus {
        self.0
            .lock()
            .expect("log archive status lock poisoned")
            .clone()
    }

    pub fn reset(&self) {
        self.set(LogArchiveStatus::default());
    }
}

#[derive(Debug)]
pub enum LogArchiveCommand {
    Start {
        destination: PathBuf,
        age_limit_hours: u16,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Stop {
        reply: oneshot::Sender<Result<(), String>>,
    },
}

pub fn validate_age_limit_hours(value: u16) -> Result<u16, String> {
    ALLOWED_AGE_LIMIT_HOURS
        .contains(&value)
        .then_some(value)
        .ok_or_else(|| "log archive age limit must be 1, 6, or 24 hours".into())
}

pub async fn prepare_destination(destination: &Path) -> Result<PathBuf, String> {
    if !destination.is_absolute() || destination.file_name().is_none() {
        return Err("log archive destination must be an absolute file path".into());
    }
    if destination.to_string_lossy().len() > MAX_PATH_BYTES {
        return Err("log archive destination path is too long".into());
    }
    match tokio::fs::symlink_metadata(destination).await {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err("log archive destination cannot be a symbolic link".into());
        }
        Ok(metadata) if !metadata.is_file() => {
            return Err("log archive destination must be a regular file".into());
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "unable to inspect log archive destination: {error}"
            ));
        }
    }
    let parent = destination
        .parent()
        .ok_or_else(|| "log archive destination has no parent directory".to_string())?;
    let parent = tokio::fs::canonicalize(parent)
        .await
        .map_err(|error| format!("log archive destination is unavailable: {error}"))?;
    if !tokio::fs::metadata(&parent)
        .await
        .map_err(|error| format!("log archive destination is unavailable: {error}"))?
        .is_dir()
    {
        return Err("log archive destination parent is not a directory".into());
    }
    Ok(parent.join(destination.file_name().expect("file name checked above")))
}

pub async fn serve(
    mut adapter: AdapterHandle,
    mut handshake: RsdHandshake,
    mut commands: mpsc::Receiver<LogArchiveCommand>,
    status: LogArchiveSlot,
    reporter: ServiceReporter,
    mut shutdown: watch::Receiver<bool>,
) {
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
                attempt += 1;
                let outcome = run_export(
                    &mut adapter,
                    &mut handshake,
                    destination,
                    age_limit_hours,
                    &mut commands,
                    &status,
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
async fn run_export(
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
    destination: PathBuf,
    age_limit_hours: u16,
    commands: &mut mpsc::Receiver<LogArchiveCommand>,
    status: &LogArchiveSlot,
    reporter: &ServiceReporter,
    attempt: u32,
    shutdown: &mut watch::Receiver<bool>,
    reply: oneshot::Sender<Result<(), String>>,
) -> ExportOutcome {
    let age_limit_hours = match validate_age_limit_hours(age_limit_hours) {
        Ok(value) => value,
        Err(error) => {
            fail_start(status, reporter, attempt, error, reply);
            return ExportOutcome::Continue;
        }
    };
    let destination = match prepare_destination(&destination).await {
        Ok(destination) => destination,
        Err(error) => {
            fail_start(status, reporter, attempt, error, reply);
            return ExportOutcome::Continue;
        }
    };
    let temporary = match crate::app_documents::temporary_sibling(&destination, "log-archive") {
        Ok(path) => path,
        Err(error) => {
            fail_start(status, reporter, attempt, error, reply);
            return ExportOutcome::Continue;
        }
    };
    let file = match tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .await
    {
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
        destination_name: destination
            .file_name()
            .map(|name| name.to_string_lossy().chars().take(255).collect()),
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
            writer
                .file
                .sync_data()
                .await
                .map_err(|error| format!("unable to synchronize log archive data: {error}"))?;
            let written = writer.written;
            drop(writer);
            crate::app_documents::replace_local_file(&task_temporary, &task_destination).await?;
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
        let _ = tokio::fs::remove_file(&temporary).await;
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

    #[tokio::test]
    async fn destination_and_age_limits_are_validated() {
        assert!(
            prepare_destination(Path::new("relative.tar"))
                .await
                .is_err()
        );
        assert!(prepare_destination(&std::env::temp_dir()).await.is_err());
        assert_eq!(validate_age_limit_hours(6).unwrap(), 6);
        assert!(validate_age_limit_hours(2).is_err());

        let destination = std::env::temp_dir().join(format!(
            "devicehub-mask-log-archive-{}.tar",
            uuid::Uuid::new_v4()
        ));
        let expected = tokio::fs::canonicalize(std::env::temp_dir())
            .await
            .unwrap()
            .join(destination.file_name().unwrap());
        assert_eq!(prepare_destination(&destination).await.unwrap(), expected);
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
