//! Bounded, bundle-aware application lifecycle queries through CoreDevice AppService.

use std::time::Duration;

use idevice::core_device::AppServiceClient;
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use idevice::{ReadWrite, RsdService};
use serde::Serialize;
use tokio::sync::{mpsc, oneshot, watch};

use crate::supervisor::ServiceReporter;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const WAIT_TIMEOUT_MAX: Duration = Duration::from_secs(10);
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppLifecycleStatus {
    pub bundle_id: String,
    pub installed: bool,
    pub running: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppLifecycleWaitResult {
    pub condition_met: bool,
    pub expected_running: bool,
    pub elapsed_ms: u64,
    pub app: AppLifecycleStatus,
}

#[derive(Debug)]
pub enum AppLifecycleCommand {
    Inspect {
        bundle_id: String,
        reply: oneshot::Sender<Result<AppLifecycleStatus, String>>,
    },
    Wait {
        bundle_id: String,
        expected_running: bool,
        timeout_ms: u64,
        reply: oneshot::Sender<Result<AppLifecycleWaitResult, String>>,
    },
}

impl AppLifecycleCommand {
    pub fn reject(self, error: impl Into<String>) {
        let error = error.into();
        match self {
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
    mut commands: mpsc::Receiver<AppLifecycleCommand>,
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
                    AppLifecycleCommand::Inspect { bundle_id, reply } => {
                        let result = inspect_app(adapter.clone(), handshake.clone(), bundle_id).await;
                        report_result(&reporter, attempt, &result);
                        let _ = reply.send(result);
                    }
                    AppLifecycleCommand::Wait { bundle_id, expected_running, timeout_ms, reply } => {
                        let result = wait_for_app(
                            adapter.clone(),
                            handshake.clone(),
                            bundle_id,
                            expected_running,
                            timeout_ms,
                        ).await;
                        report_result(&reporter, attempt, &result);
                        let _ = reply.send(result);
                    }
                }
            }
        }
    }
}

fn report_result<T>(reporter: &ServiceReporter, attempt: u32, result: &Result<T, String>) {
    match result {
        Ok(_) => reporter.ready(attempt),
        Err(error) => reporter.unavailable(attempt, error.clone()),
    }
}

async fn inspect_app(
    adapter: AdapterHandle,
    handshake: RsdHandshake,
    bundle_id: String,
) -> Result<AppLifecycleStatus, String> {
    validate_bundle_id(&bundle_id)?;
    let mut client = connect(adapter, handshake).await?;
    let app_path = resolve_app_path(&mut client, &bundle_id).await?;
    read_app_status(&mut client, bundle_id, app_path.as_deref()).await
}

async fn wait_for_app(
    adapter: AdapterHandle,
    handshake: RsdHandshake,
    bundle_id: String,
    expected_running: bool,
    timeout_ms: u64,
) -> Result<AppLifecycleWaitResult, String> {
    validate_bundle_id(&bundle_id)?;
    let timeout = Duration::from_millis(timeout_ms);
    validate_wait_timeout(timeout)?;
    let mut client = connect(adapter, handshake).await?;
    let app_path = resolve_app_path(&mut client, &bundle_id).await?;
    let started = tokio::time::Instant::now();
    let deadline = started + timeout;
    loop {
        let app = read_app_status(&mut client, bundle_id.clone(), app_path.as_deref()).await?;
        let condition_met = app.running == expected_running;
        let elapsed_ms = started.elapsed().as_millis() as u64;
        if condition_met || timeout.is_zero() || tokio::time::Instant::now() >= deadline {
            return Ok(AppLifecycleWaitResult {
                condition_met,
                expected_running,
                elapsed_ms,
                app,
            });
        }
        tokio::time::sleep_until((tokio::time::Instant::now() + WAIT_POLL_INTERVAL).min(deadline))
            .await;
    }
}

async fn connect(
    mut adapter: AdapterHandle,
    mut handshake: RsdHandshake,
) -> Result<AppServiceClient<Box<dyn ReadWrite>>, String> {
    tokio::time::timeout(
        CONNECT_TIMEOUT,
        AppServiceClient::connect_rsd(&mut adapter, &mut handshake),
    )
    .await
    .map_err(|_| "CoreDevice app lifecycle connection timed out".to_string())?
    .map_err(|error| format!("CoreDevice app lifecycle service unavailable: {error:?}"))
}

async fn resolve_app_path(
    client: &mut AppServiceClient<Box<dyn ReadWrite>>,
    bundle_id: &str,
) -> Result<Option<String>, String> {
    let apps = tokio::time::timeout(
        REQUEST_TIMEOUT,
        client.list_apps(true, true, false, false, true),
    )
    .await
    .map_err(|_| "CoreDevice application lookup timed out".to_string())?
    .map_err(|error| format!("unable to resolve application: {error:?}"))?;
    Ok(apps
        .into_iter()
        .find(|app| app.bundle_identifier == bundle_id)
        .map(|app| app.path))
}

async fn read_app_status(
    client: &mut AppServiceClient<Box<dyn ReadWrite>>,
    bundle_id: String,
    app_path: Option<&str>,
) -> Result<AppLifecycleStatus, String> {
    let Some(app_path) = app_path else {
        return Ok(AppLifecycleStatus {
            bundle_id,
            installed: false,
            running: false,
        });
    };
    let processes = tokio::time::timeout(REQUEST_TIMEOUT, client.list_processes())
        .await
        .map_err(|_| "CoreDevice process lookup timed out".to_string())?
        .map_err(|error| format!("unable to inspect application processes: {error:?}"))?;
    let running = processes.iter().any(|process| {
        process.executable_url.as_ref().is_some_and(|executable| {
            process_executable_belongs_to_app(app_path, &executable.relative)
        })
    });
    Ok(AppLifecycleStatus {
        bundle_id,
        installed: true,
        running,
    })
}

fn validate_bundle_id(bundle_id: &str) -> Result<(), String> {
    let valid = !bundle_id.is_empty()
        && bundle_id.len() <= 255
        && bundle_id.contains('.')
        && bundle_id.split('.').all(|part| {
            !part.is_empty()
                && part.len() <= 63
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        });
    valid
        .then_some(())
        .ok_or_else(|| "invalid application bundle identifier".to_string())
}

fn validate_wait_timeout(timeout: Duration) -> Result<(), String> {
    if timeout > WAIT_TIMEOUT_MAX {
        Err(format!(
            "application wait cannot exceed {} milliseconds",
            WAIT_TIMEOUT_MAX.as_millis()
        ))
    } else {
        Ok(())
    }
}

fn normalized_app_path(path: &str) -> &str {
    path.strip_prefix("file://localhost")
        .or_else(|| path.strip_prefix("file://"))
        .unwrap_or(path)
        .trim_end_matches('/')
}

pub(crate) fn process_executable_belongs_to_app(app_path: &str, executable_path: &str) -> bool {
    let app_path = normalized_app_path(app_path);
    let executable_path = normalized_app_path(executable_path);
    executable_path
        .rsplit_once('/')
        .is_some_and(|(parent, executable)| parent == app_path && !executable.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_inputs_are_bounded() {
        assert!(validate_bundle_id("com.example.game").is_ok());
        assert!(validate_bundle_id("com..game").is_err());
        assert!(validate_bundle_id("invalid bundle").is_err());
        assert!(validate_wait_timeout(WAIT_TIMEOUT_MAX).is_ok());
        assert!(validate_wait_timeout(WAIT_TIMEOUT_MAX + Duration::from_millis(1)).is_err());
    }

    #[test]
    fn matches_only_an_apps_main_executable() {
        let app = "/private/var/containers/Bundle/Application/UUID/Example.app/";
        assert!(process_executable_belongs_to_app(
            app,
            "file:///private/var/containers/Bundle/Application/UUID/Example.app/Example"
        ));
        assert!(process_executable_belongs_to_app(
            "file://localhost/private/var/containers/Bundle/Application/UUID/Example.app",
            "/private/var/containers/Bundle/Application/UUID/Example.app/Example"
        ));
        assert!(!process_executable_belongs_to_app(
            app,
            "/private/var/containers/Bundle/Application/UUID/Example.app/PlugIns/Widget.appex/Widget"
        ));
        assert!(!process_executable_belongs_to_app(
            app,
            "/private/var/containers/Bundle/Application/OTHER/Example.app/Example"
        ));
    }

    #[test]
    fn lifecycle_results_have_a_stable_shape() {
        let result = AppLifecycleWaitResult {
            condition_met: true,
            expected_running: false,
            elapsed_ms: 250,
            app: AppLifecycleStatus {
                bundle_id: "com.example.game".into(),
                installed: true,
                running: false,
            },
        };
        let serialized = serde_json::to_value(result).unwrap();
        assert_eq!(serialized["condition_met"], true);
        assert_eq!(serialized["app"]["bundle_id"], "com.example.game");
        assert_eq!(serialized["app"]["installed"], true);
        assert_eq!(serialized["app"]["running"], false);
    }
}
