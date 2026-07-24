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

#[derive(Debug)]
pub enum RunningProcessCommand {
    List {
        reply: oneshot::Sender<Result<RunningProcessList, String>>,
    },
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
                let Some(RunningProcessCommand::List { reply }) = command else { return };
                attempt += 1;
                reporter.connecting(attempt);
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
}
