//! Session-owned, bounded stdout/stderr capture for explicitly launched applications.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use idevice::RsdService;
use idevice::core_device::{AppServiceClient, OpenStdioSocketClient};
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use serde::Serialize;
use tokio::io::AsyncReadExt;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::supervisor::ServiceReporter;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_LINES: usize = 1_000;
const MAX_BUFFER_BYTES: usize = 1024 * 1024;
const MAX_LINE_BYTES: usize = 8 * 1024;
const READ_CHUNK_BYTES: usize = 4 * 1024;
const MAX_ERROR_CHARS: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppConsolePhase {
    Stopped,
    Starting,
    Running,
    Exited,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppConsoleLine {
    pub sequence: u64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppConsoleSnapshot {
    pub phase: AppConsolePhase,
    pub bundle_id: Option<String>,
    pub started_at_ms: Option<u64>,
    pub ended_at_ms: Option<u64>,
    pub total_bytes: u64,
    pub total_lines: u64,
    pub dropped_lines: u64,
    pub next_sequence: u64,
    pub reset: bool,
    pub lines: Vec<AppConsoleLine>,
    pub last_error: Option<String>,
}

#[derive(Debug)]
pub enum AppConsoleCommand {
    Start {
        bundle_id: String,
        reply: oneshot::Sender<Result<AppConsoleSnapshot, String>>,
    },
    Stop {
        clear: bool,
        reply: oneshot::Sender<AppConsoleSnapshot>,
    },
    Snapshot {
        after: Option<u64>,
        reply: oneshot::Sender<AppConsoleSnapshot>,
    },
}

impl AppConsoleCommand {
    pub fn reject(self, reason: &str) {
        match self {
            Self::Start { reply, .. } => {
                let _ = reply.send(Err(reason.into()));
            }
            Self::Stop { reply, .. } | Self::Snapshot { reply, .. } => {
                let state = ConsoleState {
                    last_error: Some(bound_text(reason, MAX_ERROR_CHARS)),
                    ..ConsoleState::default()
                };
                let _ = reply.send(state.snapshot(None));
            }
        }
    }
}

struct ConsoleState {
    phase: AppConsolePhase,
    bundle_id: Option<String>,
    started_at_ms: Option<u64>,
    ended_at_ms: Option<u64>,
    total_bytes: u64,
    total_lines: u64,
    dropped_lines: u64,
    next_sequence: u64,
    retained_bytes: usize,
    lines: VecDeque<AppConsoleLine>,
    last_error: Option<String>,
}

impl Default for ConsoleState {
    fn default() -> Self {
        Self {
            phase: AppConsolePhase::Stopped,
            bundle_id: None,
            started_at_ms: None,
            ended_at_ms: None,
            total_bytes: 0,
            total_lines: 0,
            dropped_lines: 0,
            next_sequence: 1,
            retained_bytes: 0,
            lines: VecDeque::new(),
            last_error: None,
        }
    }
}

impl ConsoleState {
    fn begin(&mut self, bundle_id: String) {
        *self = Self {
            phase: AppConsolePhase::Starting,
            bundle_id: Some(bundle_id),
            started_at_ms: Some(unix_millis()),
            ..Self::default()
        };
    }

    fn push_line(&mut self, bytes: &[u8]) {
        self.total_lines = self.total_lines.saturating_add(1);
        let text = normalize_line(bytes);
        let line_bytes = text.len();
        let line = AppConsoleLine {
            sequence: self.next_sequence,
            text,
        };
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.retained_bytes = self.retained_bytes.saturating_add(line_bytes);
        self.lines.push_back(line);
        while self.lines.len() > MAX_LINES || self.retained_bytes > MAX_BUFFER_BYTES {
            let Some(dropped) = self.lines.pop_front() else {
                break;
            };
            self.retained_bytes = self.retained_bytes.saturating_sub(dropped.text.len());
            self.dropped_lines = self.dropped_lines.saturating_add(1);
        }
    }

    fn snapshot(&self, after: Option<u64>) -> AppConsoleSnapshot {
        let first_sequence = self.lines.front().map(|line| line.sequence);
        let reset = after.is_some_and(|cursor| {
            first_sequence.is_some_and(|first| cursor.saturating_add(1) < first)
        });
        let lines = if reset || after.is_none() {
            self.lines.iter().cloned().collect()
        } else {
            let cursor = after.unwrap_or_default();
            self.lines
                .iter()
                .filter(|line| line.sequence > cursor)
                .cloned()
                .collect()
        };
        AppConsoleSnapshot {
            phase: self.phase,
            bundle_id: self.bundle_id.clone(),
            started_at_ms: self.started_at_ms,
            ended_at_ms: self.ended_at_ms,
            total_bytes: self.total_bytes,
            total_lines: self.total_lines,
            dropped_lines: self.dropped_lines,
            next_sequence: self.next_sequence,
            reset,
            lines,
            last_error: self.last_error.clone(),
        }
    }
}

pub async fn serve(
    adapter: AdapterHandle,
    handshake: RsdHandshake,
    mut commands: mpsc::Receiver<AppConsoleCommand>,
    reporter: ServiceReporter,
    mut shutdown: watch::Receiver<bool>,
) {
    let state = Arc::new(Mutex::new(ConsoleState::default()));
    let mut reader: Option<JoinHandle<Result<(), String>>> = None;
    let mut attempt = 0;
    reporter.stopped(attempt);

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { break; }
            }
            command = commands.recv() => {
                let Some(command) = command else { break };
                match command {
                    AppConsoleCommand::Start { bundle_id, reply } => {
                        if let Some(task) = reader.take() { task.abort(); }
                        state.lock().unwrap().begin(bundle_id.clone());
                        attempt += 1;
                        reporter.connecting(attempt);
                        match start_stream(adapter.clone(), handshake.clone(), &bundle_id).await {
                            Ok(stream) => {
                                state.lock().unwrap().phase = AppConsolePhase::Running;
                                reporter.ready(attempt);
                                tracing::info!(component = "app_console", operation = "start", %bundle_id, "application console capture started");
                                let task_state = state.clone();
                                reader = Some(tokio::task::spawn_local(read_stream(stream, task_state)));
                                let _ = reply.send(Ok(state.lock().unwrap().snapshot(None)));
                            }
                            Err(error) => {
                                let error = bound_text(&error, MAX_ERROR_CHARS);
                                {
                                    let mut current = state.lock().unwrap();
                                    current.phase = AppConsolePhase::Failed;
                                    current.ended_at_ms = Some(unix_millis());
                                    current.last_error = Some(error.clone());
                                }
                                reporter.unavailable(attempt, error.clone());
                                let _ = reply.send(Err(error));
                            }
                        }
                    }
                    AppConsoleCommand::Stop { clear, reply } => {
                        if let Some(task) = reader.take() { task.abort(); }
                        let mut current = state.lock().unwrap();
                        if clear {
                            *current = ConsoleState::default();
                        } else {
                            current.phase = AppConsolePhase::Stopped;
                            current.ended_at_ms.get_or_insert_with(unix_millis);
                        }
                        reporter.stopped(attempt);
                        let _ = reply.send(current.snapshot(None));
                    }
                    AppConsoleCommand::Snapshot { after, reply } => {
                        let _ = reply.send(state.lock().unwrap().snapshot(after));
                    }
                }
            }
            result = wait_reader(&mut reader) => {
                reader.take();
                let mut current = state.lock().unwrap();
                current.ended_at_ms = Some(unix_millis());
                match result {
                    Ok(Ok(())) => {
                        current.phase = AppConsolePhase::Exited;
                        reporter.stopped(attempt);
                    }
                    Ok(Err(error)) => {
                        let error = bound_text(&error, MAX_ERROR_CHARS);
                        current.phase = AppConsolePhase::Failed;
                        current.last_error = Some(error.clone());
                        reporter.unavailable(attempt, error);
                    }
                    Err(error) if error.is_cancelled() => {
                        current.phase = AppConsolePhase::Stopped;
                        reporter.stopped(attempt);
                    }
                    Err(error) => {
                        let error = bound_text(&format!("application console task failed: {error}"), MAX_ERROR_CHARS);
                        current.phase = AppConsolePhase::Failed;
                        current.last_error = Some(error.clone());
                        reporter.unavailable(attempt, error);
                    }
                }
            }
        }
    }

    if let Some(task) = reader.take() {
        task.abort();
    }
    *state.lock().unwrap() = ConsoleState::default();
    reporter.stopped(attempt);
}

async fn wait_reader(
    reader: &mut Option<JoinHandle<Result<(), String>>>,
) -> Result<Result<(), String>, tokio::task::JoinError> {
    match reader.as_mut() {
        Some(task) => task.await,
        None => std::future::pending().await,
    }
}

async fn start_stream(
    mut adapter: AdapterHandle,
    mut handshake: RsdHandshake,
    bundle_id: &str,
) -> Result<OpenStdioSocketClient, String> {
    tokio::time::timeout(CONNECT_TIMEOUT, async {
        let mut stdio = OpenStdioSocketClient::connect_rsd(&mut adapter, &mut handshake)
            .await
            .map_err(|error| format!("application console service unavailable: {error:?}"))?;
        let stdio_uuid = stdio
            .read_uuid()
            .await
            .map_err(|error| format!("unable to initialize application console: {error:?}"))?;
        let mut apps = AppServiceClient::connect_rsd(&mut adapter, &mut handshake)
            .await
            .map_err(|error| format!("CoreDevice AppService unavailable: {error:?}"))?;
        let installed = apps
            .list_apps(true, true, false, false, true)
            .await
            .map_err(|error| format!("unable to verify application: {error:?}"))?
            .into_iter()
            .any(|app| app.bundle_identifier == bundle_id);
        if !installed {
            return Err("application is not installed on the active device".into());
        }
        apps.launch_application(bundle_id, &[], true, false, None, None, Some(stdio_uuid))
            .await
            .map_err(|error| format!("unable to launch application with console: {error:?}"))?;
        Ok(stdio)
    })
    .await
    .map_err(|_| "application console startup timed out".to_string())?
}

async fn read_stream(
    mut stream: OpenStdioSocketClient,
    state: Arc<Mutex<ConsoleState>>,
) -> Result<(), String> {
    let mut chunk = [0_u8; READ_CHUNK_BYTES];
    let mut pending = Vec::new();
    loop {
        let read = stream
            .inner
            .read(&mut chunk)
            .await
            .map_err(|error| format!("application console stream failed: {error}"))?;
        if read == 0 {
            if !pending.is_empty() {
                state.lock().unwrap().push_line(&pending);
            }
            return Ok(());
        }
        {
            let mut current = state.lock().unwrap();
            current.total_bytes = current.total_bytes.saturating_add(read as u64);
        }
        pending.extend_from_slice(&chunk[..read]);
        while let Some(index) = pending.iter().position(|byte| *byte == b'\n') {
            let line = pending.drain(..=index).collect::<Vec<_>>();
            state
                .lock()
                .unwrap()
                .push_line(&line[..line.len().saturating_sub(1)]);
        }
        while pending.len() > MAX_LINE_BYTES {
            let remainder = pending.split_off(MAX_LINE_BYTES);
            state.lock().unwrap().push_line(&pending);
            pending = remainder;
        }
    }
}

fn normalize_line(bytes: &[u8]) -> String {
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_LINE_BYTES)]);
    text.chars()
        .filter(|character| *character == '\t' || !character.is_control())
        .collect()
}

fn bound_text(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
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
    fn output_is_sanitized_and_incremental() {
        let mut state = ConsoleState::default();
        state.begin("com.example.App".into());
        state.push_line(b"hello\r");
        state.push_line(b"bad\0\x1btext");
        let first = state.snapshot(None);
        assert_eq!(first.lines[0].text, "hello");
        assert_eq!(first.lines[1].text, "badtext");
        let incremental = state.snapshot(Some(first.lines[0].sequence));
        assert_eq!(incremental.lines.len(), 1);
        assert_eq!(incremental.lines[0].text, "badtext");
    }

    #[test]
    fn output_buffer_is_bounded_and_reports_cursor_reset() {
        let mut state = ConsoleState::default();
        state.begin("com.example.App".into());
        for index in 0..=MAX_LINES {
            state.push_line(format!("line {index}").as_bytes());
        }
        let snapshot = state.snapshot(Some(0));
        assert_eq!(snapshot.lines.len(), MAX_LINES);
        assert_eq!(snapshot.dropped_lines, 1);
        assert!(snapshot.reset);
    }
}
