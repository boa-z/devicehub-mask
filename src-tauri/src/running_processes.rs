//! Bounded, on-demand running-process inventory from DVT DeviceInfo.

use std::collections::HashSet;
use std::time::Duration;

use idevice::dvt::device_info::{DeviceInfoClient, RunningProcess as RawRunningProcess};
use idevice::dvt::remote_server::RemoteServerClient;
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use idevice::{ReadWrite, RsdService};
use serde::Serialize;
use tokio::sync::{mpsc, oneshot, watch};

use crate::supervisor::ServiceReporter;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const WAIT_TIMEOUT_MAX: Duration = Duration::from_secs(10);
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(250);
const MAX_PROCESSES: usize = 1_024;
const MAX_NAME_CHARS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunningProcess {
    pub pid: u32,
    pub name: String,
    pub app_name: Option<String>,
    pub is_application: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunningProcessList {
    pub processes: Vec<RunningProcess>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunningProcessStatus {
    pub pid: u32,
    pub running: bool,
    pub executable_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunningProcessWaitResult {
    pub condition_met: bool,
    pub expected_running: bool,
    pub elapsed_ms: u64,
    pub process: RunningProcessStatus,
}

#[derive(Debug)]
pub enum RunningProcessCommand {
    List {
        reply: oneshot::Sender<Result<RunningProcessList, String>>,
    },
    Inspect {
        pid: u32,
        reply: oneshot::Sender<Result<RunningProcessStatus, String>>,
    },
    Wait {
        pid: u32,
        expected_running: bool,
        timeout_ms: u64,
        reply: oneshot::Sender<Result<RunningProcessWaitResult, String>>,
    },
}

impl RunningProcessCommand {
    pub fn reject(self, error: impl Into<String>) {
        let error = error.into();
        match self {
            Self::List { reply } => {
                let _ = reply.send(Err(error));
            }
            Self::Inspect { reply, .. } => {
                let _ = reply.send(Err(error));
            }
            Self::Wait { reply, .. } => {
                let _ = reply.send(Err(error));
            }
        }
    }
}

pub async fn serve(
    adapter: AdapterHandle,
    handshake: RsdHandshake,
    mut commands: mpsc::Receiver<RunningProcessCommand>,
    reporter: ServiceReporter,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut attempt = 0;
    reporter.stopped(attempt);
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
            }
            command = commands.recv() => {
                let Some(command) = command else { return };
                attempt += 1;
                reporter.connecting(attempt);
                match command {
                    RunningProcessCommand::List { reply } => {
                        let result = tokio::time::timeout(
                            REQUEST_TIMEOUT,
                            load_processes(adapter.clone(), handshake.clone()),
                        )
                        .await
                        .map_err(|_| "running process request timed out".to_string())
                        .and_then(|result| result);
                        match &result {
                            Ok(list) => {
                                reporter.ready(attempt);
                                tracing::info!(
                                    count = list.processes.len(),
                                    truncated = list.truncated,
                                    "running processes listed"
                                );
                            }
                            Err(error) => reporter.unavailable(attempt, error.clone()),
                        }
                        let _ = reply.send(result);
                    }
                    RunningProcessCommand::Inspect { pid, reply } => {
                        let result = tokio::time::timeout(
                            REQUEST_TIMEOUT,
                            inspect_process(adapter.clone(), handshake.clone(), pid),
                        )
                        .await
                        .map_err(|_| "running process status request timed out".to_string())
                        .and_then(|result| result);
                        match &result {
                            Ok(status) => {
                                reporter.ready(attempt);
                                tracing::info!(pid, running = status.running, "running process status inspected");
                            }
                            Err(error) => reporter.unavailable(attempt, error.clone()),
                        }
                        let _ = reply.send(result);
                    }
                    RunningProcessCommand::Wait { pid, expected_running, timeout_ms, reply } => {
                        let requested_timeout = Duration::from_millis(timeout_ms);
                        let result = tokio::time::timeout(
                            REQUEST_TIMEOUT + requested_timeout.min(WAIT_TIMEOUT_MAX),
                            wait_for_process(
                                adapter.clone(),
                                handshake.clone(),
                                pid,
                                expected_running,
                                timeout_ms,
                            ),
                        )
                        .await
                        .map_err(|_| "running process wait request timed out".to_string())
                        .and_then(|result| result);
                        match &result {
                            Ok(wait) => {
                                reporter.ready(attempt);
                                tracing::info!(
                                    pid,
                                    expected_running,
                                    condition_met = wait.condition_met,
                                    elapsed_ms = wait.elapsed_ms,
                                    "running process wait completed"
                                );
                            }
                            Err(error) => reporter.unavailable(attempt, error.clone()),
                        }
                        let _ = reply.send(result);
                    }
                }
            }
        }
    }
}

async fn load_processes(
    mut adapter: AdapterHandle,
    mut handshake: RsdHandshake,
) -> Result<RunningProcessList, String> {
    let mut remote =
        RemoteServerClient::<Box<dyn ReadWrite>>::connect_rsd(&mut adapter, &mut handshake)
            .await
            .map_err(|error| format!("DVT process inventory connection failed: {error:?}"))?;
    let mut client = DeviceInfoClient::new(&mut remote)
        .await
        .map_err(|error| format!("DVT DeviceInfo channel unavailable: {error:?}"))?;
    let processes = client
        .running_processes()
        .await
        .map_err(|error| format!("unable to list running processes: {error:?}"))?;
    Ok(normalize_processes(processes))
}

async fn inspect_process(
    mut adapter: AdapterHandle,
    mut handshake: RsdHandshake,
    pid: u32,
) -> Result<RunningProcessStatus, String> {
    validate_pid(pid)?;
    let mut remote =
        RemoteServerClient::<Box<dyn ReadWrite>>::connect_rsd(&mut adapter, &mut handshake)
            .await
            .map_err(|error| format!("DVT process status connection failed: {error:?}"))?;
    let mut client = DeviceInfoClient::new(&mut remote)
        .await
        .map_err(|error| format!("DVT DeviceInfo channel unavailable: {error:?}"))?;
    read_process_status(&mut client, pid).await
}

async fn wait_for_process(
    mut adapter: AdapterHandle,
    mut handshake: RsdHandshake,
    pid: u32,
    expected_running: bool,
    timeout_ms: u64,
) -> Result<RunningProcessWaitResult, String> {
    validate_pid(pid)?;
    let timeout = Duration::from_millis(timeout_ms);
    validate_wait_timeout(timeout)?;
    let started = tokio::time::Instant::now();
    let deadline = started + timeout;
    let mut remote = tokio::time::timeout(
        REQUEST_TIMEOUT,
        RemoteServerClient::<Box<dyn ReadWrite>>::connect_rsd(&mut adapter, &mut handshake),
    )
    .await
    .map_err(|_| "DVT process wait connection timed out".to_string())?
    .map_err(|error| format!("DVT process wait connection failed: {error:?}"))?;
    let mut client = DeviceInfoClient::new(&mut remote)
        .await
        .map_err(|error| format!("DVT DeviceInfo channel unavailable: {error:?}"))?;
    loop {
        let running = read_process_running(&mut client, pid).await?;
        let condition_met = running == expected_running;
        let elapsed_ms = started.elapsed().as_millis() as u64;
        if condition_met || timeout.is_zero() || tokio::time::Instant::now() >= deadline {
            let process = process_status_with_state(&mut client, pid, running).await;
            return Ok(RunningProcessWaitResult {
                condition_met,
                expected_running,
                elapsed_ms,
                process,
            });
        }
        tokio::time::sleep_until((tokio::time::Instant::now() + WAIT_POLL_INTERVAL).min(deadline))
            .await;
    }
}

async fn read_process_status<R: ReadWrite>(
    client: &mut DeviceInfoClient<'_, R>,
    pid: u32,
) -> Result<RunningProcessStatus, String> {
    let running = read_process_running(client, pid).await?;
    Ok(process_status_with_state(client, pid, running).await)
}

async fn read_process_running<R: ReadWrite>(
    client: &mut DeviceInfoClient<'_, R>,
    pid: u32,
) -> Result<bool, String> {
    client
        .is_running_pid(pid)
        .await
        .map_err(|error| format!("unable to inspect running process: {error:?}"))
}

async fn process_status_with_state<R: ReadWrite>(
    client: &mut DeviceInfoClient<'_, R>,
    pid: u32,
    running: bool,
) -> RunningProcessStatus {
    let executable_name = if running {
        match client.execname_for_pid(pid).await {
            Ok(name) => normalize_executable_name(&name),
            Err(error) => {
                tracing::debug!(pid, ?error, "process executable name unavailable");
                None
            }
        }
    } else {
        None
    };
    RunningProcessStatus {
        pid,
        running,
        executable_name,
    }
}

fn normalize_executable_name(value: &str) -> Option<String> {
    let basename = value.rsplit(['/', '\\']).next().unwrap_or(value);
    normalize_name(basename)
}

fn validate_pid(pid: u32) -> Result<(), String> {
    (pid > 0)
        .then_some(())
        .ok_or_else(|| "running process PID must be greater than zero".to_string())
}

fn validate_wait_timeout(timeout: Duration) -> Result<(), String> {
    if timeout > WAIT_TIMEOUT_MAX {
        Err(format!(
            "running process wait cannot exceed {} milliseconds",
            WAIT_TIMEOUT_MAX.as_millis()
        ))
    } else {
        Ok(())
    }
}

fn normalize_processes(processes: Vec<RawRunningProcess>) -> RunningProcessList {
    let truncated = processes.len() > MAX_PROCESSES;
    let mut seen = HashSet::new();
    let mut normalized = processes
        .into_iter()
        .filter(|process| seen.insert(process.pid))
        .take(MAX_PROCESSES)
        .map(|process| {
            let name =
                normalize_name(&process.name).unwrap_or_else(|| format!("Process {}", process.pid));
            let app_name = normalize_name(&process.real_app_name).filter(|value| value != &name);
            RunningProcess {
                pid: process.pid,
                name,
                app_name,
                is_application: process.is_application,
            }
        })
        .collect::<Vec<_>>();
    normalized.sort_by(|left, right| {
        right
            .is_application
            .cmp(&left.is_application)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.pid.cmp(&right.pid))
    });
    RunningProcessList {
        processes: normalized,
        truncated,
    }
}

fn normalize_name(value: &str) -> Option<String> {
    if value
        .chars()
        .any(|character| character.is_control() && !character.is_whitespace())
    {
        return None;
    }
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    (!normalized.is_empty()).then(|| normalized.chars().take(MAX_NAME_CHARS).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(pid: u32, name: &str, app_name: &str, is_application: bool) -> RawRunningProcess {
        RawRunningProcess {
            pid,
            name: name.into(),
            real_app_name: app_name.into(),
            is_application,
            start_page_count: 0,
        }
    }

    #[test]
    fn process_inventory_is_sanitized_deduplicated_and_app_first() {
        let list = normalize_processes(vec![
            raw(2, " daemon\nname ", "", false),
            raw(1, "App", "Visible App", true),
            raw(2, "duplicate", "", false),
            raw(3, "", "", false),
        ]);
        assert!(!list.truncated);
        assert_eq!(list.processes.len(), 3);
        assert_eq!(list.processes[0].pid, 1);
        assert_eq!(list.processes[0].app_name.as_deref(), Some("Visible App"));
        assert_eq!(list.processes[1].name, "daemon name");
        assert_eq!(list.processes[2].name, "Process 3");
        assert!(normalize_name("unsafe\0name").is_none());
    }

    #[test]
    fn process_inventory_is_bounded() {
        let input = (0..=MAX_PROCESSES)
            .map(|pid| raw(pid as u32, "process", "", false))
            .collect();
        let list = normalize_processes(input);
        assert_eq!(list.processes.len(), MAX_PROCESSES);
        assert!(list.truncated);
    }

    #[test]
    fn process_status_inputs_and_results_are_bounded() {
        assert!(validate_pid(0).is_err());
        assert!(validate_pid(1).is_ok());
        assert!(validate_wait_timeout(WAIT_TIMEOUT_MAX).is_ok());
        assert!(validate_wait_timeout(WAIT_TIMEOUT_MAX + Duration::from_millis(1)).is_err());

        let result = RunningProcessWaitResult {
            condition_met: true,
            expected_running: false,
            elapsed_ms: 250,
            process: RunningProcessStatus {
                pid: 42,
                running: false,
                executable_name: None,
            },
        };
        let serialized = serde_json::to_value(result).unwrap();
        assert_eq!(serialized["condition_met"], true);
        assert_eq!(serialized["expected_running"], false);
        assert_eq!(serialized["process"]["pid"], 42);
        assert_eq!(serialized["process"]["running"], false);
        assert!(serialized["process"]["executable_name"].is_null());
    }

    #[test]
    fn executable_names_never_expose_paths() {
        assert_eq!(
            normalize_executable_name("/private/var/containers/App"),
            Some("App".into())
        );
        assert_eq!(
            normalize_executable_name(r"C:\\private\\App.exe"),
            Some("App.exe".into())
        );
        assert!(normalize_executable_name("/private/unsafe\0name").is_none());
    }
}
