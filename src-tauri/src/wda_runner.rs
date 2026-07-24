//! Supervised lifecycle for an installed WebDriverAgent XCTest runner.

use std::future::pending;
use std::sync::Arc;
use std::time::Duration;

use idevice::IdeviceService;
use idevice::provider::IdeviceProvider;
use idevice::services::dvt::xctest::{TestConfig, XCUITestService, listener::XCUITestListener};
use idevice::services::installation_proxy::InstallationProxyClient;
use idevice::services::wda::WdaClient;
use serde::Serialize;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::supervisor::ServiceReporter;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const STATUS_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const MAX_ERROR_CHARS: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WdaRunnerPhase {
    Stopped,
    Starting,
    Running,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WdaRunnerStatus {
    pub phase: WdaRunnerPhase,
    pub managed: bool,
    pub runner_bundle_id: Option<String>,
    pub last_error: Option<String>,
}

impl Default for WdaRunnerStatus {
    fn default() -> Self {
        Self {
            phase: WdaRunnerPhase::Stopped,
            managed: false,
            runner_bundle_id: None,
            last_error: None,
        }
    }
}

#[derive(Debug)]
pub enum WdaRunnerCommand {
    Status {
        reply: oneshot::Sender<WdaRunnerStatus>,
    },
    Start {
        bundle_id: String,
        reply: oneshot::Sender<Result<WdaRunnerStatus, String>>,
    },
    Stop {
        reply: oneshot::Sender<Result<WdaRunnerStatus, String>>,
    },
}

impl WdaRunnerCommand {
    pub fn reject(self, reason: impl Into<String>) {
        let reason = reason.into();
        match self {
            Self::Status { reply } => {
                let _ = reply.send(WdaRunnerStatus {
                    phase: WdaRunnerPhase::Failed,
                    managed: false,
                    runner_bundle_id: None,
                    last_error: Some(reason),
                });
            }
            Self::Start { reply, .. } | Self::Stop { reply } => {
                let _ = reply.send(Err(reason));
            }
        }
    }
}

struct Startup {
    bundle_id: String,
    task: JoinHandle<Result<RunningRunner, String>>,
    reply: oneshot::Sender<Result<WdaRunnerStatus, String>>,
}

struct RunningRunner {
    bundle_id: String,
    task: JoinHandle<Result<(), idevice::IdeviceError>>,
}

struct AbortOnDrop<T>(Option<JoinHandle<T>>);

impl<T> AbortOnDrop<T> {
    fn new(task: JoinHandle<T>) -> Self {
        Self(Some(task))
    }

    fn is_finished(&self) -> bool {
        self.0.as_ref().is_none_or(JoinHandle::is_finished)
    }

    fn take(&mut self) -> JoinHandle<T> {
        self.0.take().expect("runner task is present")
    }
}

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
        }
    }
}

struct NoopListener;

impl XCUITestListener for NoopListener {}

pub fn validate_runner_bundle_id(bundle_id: &str) -> Result<(), &'static str> {
    if bundle_id.is_empty() || bundle_id.len() > 255 || !bundle_id.ends_with(".xctrunner") {
        return Err("WDA runner bundle ID must end with .xctrunner");
    }
    if bundle_id.starts_with('.')
        || bundle_id.contains("..")
        || bundle_id.split('.').any(|segment| segment.len() > 63)
        || bundle_id
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')))
    {
        return Err("invalid WDA runner bundle ID");
    }
    Ok(())
}

pub async fn serve(
    provider: Arc<dyn IdeviceProvider>,
    mut commands: mpsc::Receiver<WdaRunnerCommand>,
    reporter: ServiceReporter,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut status = WdaRunnerStatus::default();
    let mut startup: Option<Startup> = None;
    let mut running: Option<RunningRunner> = None;
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
                    WdaRunnerCommand::Status { reply } => {
                        let _ = reply.send(status.clone());
                    }
                    WdaRunnerCommand::Start { bundle_id, reply } => {
                        if let Err(error) = validate_runner_bundle_id(&bundle_id) {
                            let _ = reply.send(Err(error.into()));
                            continue;
                        }
                        if startup.is_some() || running.is_some() {
                            let active = status.runner_bundle_id.as_deref().unwrap_or("unknown");
                            let _ = reply.send(Err(format!("WDA runner {active} is already managed")));
                            continue;
                        }
                        attempt += 1;
                        status = WdaRunnerStatus {
                            phase: WdaRunnerPhase::Starting,
                            managed: true,
                            runner_bundle_id: Some(bundle_id.clone()),
                            last_error: None,
                        };
                        reporter.connecting(attempt);
                        tracing::info!(
                            component = "wda_runner",
                            operation = "start",
                            runner_bundle_id = %bundle_id,
                            "starting WebDriverAgent XCTest runner"
                        );
                        startup = Some(Startup {
                            bundle_id: bundle_id.clone(),
                            task: tokio::spawn(start_runner(provider.clone(), bundle_id)),
                            reply,
                        });
                    }
                    WdaRunnerCommand::Stop { reply } => {
                        let was_managed = startup.is_some() || running.is_some();
                        if let Some(starting) = startup.take() {
                            starting.task.abort();
                            let _ = starting.reply.send(Err("WDA runner startup cancelled".into()));
                        }
                        if let Some(active) = running.take() {
                            active.task.abort();
                        }
                        status = WdaRunnerStatus::default();
                        reporter.stopped(attempt);
                        if was_managed {
                            tracing::info!(component = "wda_runner", operation = "stop", "stopped managed WebDriverAgent runner");
                        }
                        let _ = reply.send(Ok(status.clone()));
                    }
                }
            }
            result = wait_startup(&mut startup) => {
                let starting = startup.take().expect("completed startup exists");
                match result {
                    Ok(Ok(active)) => {
                        status = WdaRunnerStatus {
                            phase: WdaRunnerPhase::Running,
                            managed: true,
                            runner_bundle_id: Some(active.bundle_id.clone()),
                            last_error: None,
                        };
                        reporter.ready(attempt);
                        tracing::info!(
                            component = "wda_runner",
                            operation = "ready",
                            runner_bundle_id = %active.bundle_id,
                            "WebDriverAgent runner is ready"
                        );
                        let _ = starting.reply.send(Ok(status.clone()));
                        running = Some(active);
                    }
                    Ok(Err(error)) => {
                        fail_startup(&mut status, &reporter, attempt, starting, error);
                    }
                    Err(error) => {
                        fail_startup(
                            &mut status,
                            &reporter,
                            attempt,
                            starting,
                            format!("WDA runner startup task failed: {error}"),
                        );
                    }
                }
            }
            result = wait_runner(&mut running) => {
                let active = running.take().expect("completed runner exists");
                let error = match result {
                    Ok(Ok(())) => "WDA runner exited unexpectedly".to_string(),
                    Ok(Err(error)) => format!("WDA runner stopped: {error:?}"),
                    Err(error) => format!("WDA runner task failed: {error}"),
                };
                let error = bound_error(error);
                tracing::warn!(component = "wda_runner", operation = "exit", runner_bundle_id = %active.bundle_id, %error, "managed WebDriverAgent runner ended");
                reporter.unavailable(attempt, error.clone());
                status = WdaRunnerStatus {
                    phase: WdaRunnerPhase::Failed,
                    managed: false,
                    runner_bundle_id: Some(active.bundle_id),
                    last_error: Some(error),
                };
            }
        }
    }

    if let Some(starting) = startup.take() {
        starting.task.abort();
        let _ = starting.reply.send(Err("device session ended".into()));
    }
    if let Some(active) = running.take() {
        active.task.abort();
    }
    reporter.stopped(attempt);
}

async fn wait_startup(
    startup: &mut Option<Startup>,
) -> Result<Result<RunningRunner, String>, tokio::task::JoinError> {
    match startup.as_mut() {
        Some(startup) => (&mut startup.task).await,
        None => pending().await,
    }
}

async fn wait_runner(
    running: &mut Option<RunningRunner>,
) -> Result<Result<(), idevice::IdeviceError>, tokio::task::JoinError> {
    match running.as_mut() {
        Some(runner) => (&mut runner.task).await,
        None => pending().await,
    }
}

fn fail_startup(
    status: &mut WdaRunnerStatus,
    reporter: &ServiceReporter,
    attempt: u32,
    startup: Startup,
    error: String,
) {
    let error = bound_error(error);
    tracing::warn!(component = "wda_runner", operation = "start", runner_bundle_id = %startup.bundle_id, %error, "unable to start WebDriverAgent runner");
    reporter.unavailable(attempt, error.clone());
    *status = WdaRunnerStatus {
        phase: WdaRunnerPhase::Failed,
        managed: false,
        runner_bundle_id: Some(startup.bundle_id.clone()),
        last_error: Some(error.clone()),
    };
    let _ = startup.reply.send(Err(error));
}

async fn start_runner(
    provider: Arc<dyn IdeviceProvider>,
    bundle_id: String,
) -> Result<RunningRunner, String> {
    let probe = WdaClient::new(provider.as_ref()).with_timeout(STATUS_PROBE_TIMEOUT);
    if tokio::time::timeout(STATUS_PROBE_TIMEOUT, probe.status())
        .await
        .is_ok_and(|result| result.is_ok())
    {
        return Err("WebDriverAgent is already reachable; DeviceHub Mask will not replace an externally managed runner".into());
    }

    match tokio::time::timeout(
        Duration::from_secs(4),
        crate::developer_image::is_mounted_for_device(provider.as_ref()),
    )
    .await
    {
        Ok(Ok(true)) => {}
        Ok(Ok(false)) => {
            return Err("a compatible Developer Disk Image is not mounted; mount it before starting WebDriverAgent".into());
        }
        Ok(Err(error)) => {
            tracing::warn!(%error, "developer image preflight unavailable; continuing WDA startup");
        }
        Err(_) => {
            tracing::warn!("developer image preflight timed out; continuing WDA startup");
        }
    }

    let mut installation = InstallationProxyClient::connect(provider.as_ref())
        .await
        .map_err(|error| format!("unable to inspect WDA runner: {error:?}"))?;
    validate_installed_runner(&mut installation, &bundle_id).await?;
    let config = TestConfig::from_installation_proxy(&mut installation, &bundle_id, None)
        .await
        .map_err(|error| format!("unable to prepare WDA runner: {error:?}"))?;

    let service = XCUITestService::new(provider.clone());
    let mut runner_task = AbortOnDrop::new(tokio::spawn(async move {
        let mut listener = NoopListener;
        service.run(config, &mut listener, None).await
    }));
    let wda = WdaClient::new(provider.as_ref()).with_timeout(STATUS_PROBE_TIMEOUT);
    let deadline = tokio::time::Instant::now() + STARTUP_TIMEOUT;

    loop {
        if runner_task.is_finished() {
            let result = runner_task
                .take()
                .await
                .map_err(|error| format!("WDA runner task failed: {error}"))?;
            result.map_err(|error| format!("WDA runner exited during startup: {error:?}"))?;
            return Err("WDA runner exited before becoming reachable".into());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("WDA runner did not become reachable within 30 seconds".into());
        }
        match tokio::time::timeout_at(deadline, wda.status()).await {
            Ok(Ok(_)) => {
                return Ok(RunningRunner {
                    bundle_id,
                    task: runner_task.take(),
                });
            }
            Ok(Err(_)) => tokio::time::sleep(POLL_INTERVAL).await,
            Err(_) => return Err("WDA runner did not become reachable within 30 seconds".into()),
        }
    }
}

async fn validate_installed_runner(
    installation: &mut InstallationProxyClient,
    bundle_id: &str,
) -> Result<(), String> {
    let apps = installation
        .get_apps(Some("User"), Some(vec![bundle_id.to_owned()]))
        .await
        .map_err(|error| format!("unable to inspect WDA runner: {error:?}"))?;
    let fields = apps
        .get(bundle_id)
        .and_then(plist::Value::as_dictionary)
        .ok_or_else(|| {
            "the selected WDA runner is not installed as a user application".to_string()
        })?;
    let signer = fields
        .get("SignerIdentity")
        .and_then(plist::Value::as_string)
        .unwrap_or_default();
    let developer = fields
        .get("IsXcodeManaged")
        .and_then(plist::Value::as_boolean)
        .unwrap_or(false)
        || signer.contains("Apple Development");
    if !developer {
        return Err("the selected .xctrunner is not identified as a developer application".into());
    }
    Ok(())
}

fn bound_error(error: impl Into<String>) -> String {
    error.into().chars().take(MAX_ERROR_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runner_bundle_ids_are_suffix_and_character_bounded() {
        assert!(validate_runner_bundle_id("com.example.WebDriverAgentRunner.xctrunner").is_ok());
        assert!(validate_runner_bundle_id("com.example.Runner").is_err());
        assert!(validate_runner_bundle_id("../bad.xctrunner").is_err());
        assert!(validate_runner_bundle_id("com.example.bad_name.xctrunner").is_err());
        assert!(validate_runner_bundle_id(&format!("com.{}.xctrunner", "a".repeat(64))).is_err());
        assert!(validate_runner_bundle_id(&format!("{}.xctrunner", "a".repeat(256))).is_err());
    }

    #[test]
    fn errors_are_bounded_on_character_boundaries() {
        let error = bound_error("你".repeat(MAX_ERROR_CHARS + 1));
        assert_eq!(error.chars().count(), MAX_ERROR_CHARS);
    }
}
