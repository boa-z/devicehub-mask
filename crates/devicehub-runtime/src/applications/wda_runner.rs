//! Supervised lifecycle for an installed WebDriverAgent XCTest runner.

use std::sync::Arc;
use std::time::Duration;

use idevice::IdeviceService;
use idevice::provider::IdeviceProvider;
use idevice::services::dvt::xctest::{TestConfig, WdaRunHandle, XCUITestService};
use idevice::services::installation_proxy::InstallationProxyClient;
use idevice::services::wda::WdaClient;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use devicehub_core::{
    ManagedOperationError, ManagedOperationKind, ManagedOperationRegistry, OperationErrorCode,
    WdaRunnerPhase, WdaRunnerStatus, validate_wda_runner_bundle_id,
};

use crate::supervisor::ServiceReporter;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const STATUS_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const MAX_ERROR_CHARS: usize = 512;

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
    handle: Option<WdaRunHandle>,
    provider: Arc<dyn IdeviceProvider>,
}

impl Drop for RunningRunner {
    fn drop(&mut self) {
        // `WdaRunHandle` intentionally exposes explicit cancellation rather
        // than aborting in its own `Drop` implementation. Keep the handle
        // supervised by the DeviceHub service on every exit path.
        if let Some(handle) = &self.handle {
            handle.abort();
        }
    }
}

impl RunningRunner {
    async fn stop(mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
            let _ = handle.wait().await;
        }
    }
}

pub(crate) async fn serve_wda_runner(
    provider: Arc<dyn IdeviceProvider>,
    mut commands: mpsc::Receiver<WdaRunnerCommand>,
    operations: ManagedOperationRegistry,
    reporter: ServiceReporter,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut status = WdaRunnerStatus::default();
    let mut startup: Option<Startup> = None;
    let mut running: Option<RunningRunner> = None;
    let mut managed_id = None;
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
                        if let Err(error) = validate_wda_runner_bundle_id(&bundle_id) {
                            let _ = reply.send(Err(error.into()));
                            continue;
                        }
                        if startup.is_some() || running.is_some() {
                            let active = status.runner_bundle_id.as_deref().unwrap_or("unknown");
                            let _ = reply.send(Err(format!("WDA runner {active} is already managed")));
                            continue;
                        }
                        let operation_id = match operations.begin(
                            ManagedOperationKind::WdaRunner,
                            Some(bundle_id.clone()),
                            true,
                        ) {
                            Ok(id) => id,
                            Err(error) => {
                                let _ = reply.send(Err(error.message));
                                continue;
                            }
                        };
                        operations.update(operation_id, Some("starting".into()), Some(0.0));
                        managed_id = Some(operation_id);
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
                        stop_runner_tasks(
                            &mut startup,
                            &mut running,
                            "WDA runner startup cancelled",
                        ).await;
                        if let Some(operation_id) = managed_id.take() {
                            operations.cancel(operation_id, "WDA runner stopped");
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
                        if let Some(operation_id) = managed_id {
                            operations.update(operation_id, Some("running".into()), None);
                        }
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
                        if let Some(operation_id) = managed_id.take() {
                            operations.fail(
                                operation_id,
                                ManagedOperationError::new(
                                    OperationErrorCode::Unavailable,
                                    error.clone(),
                                ),
                            );
                        }
                        fail_startup(&mut status, &reporter, attempt, starting, error);
                    }
                    Err(error) => {
                        let message = format!("WDA runner startup task failed: {error}");
                        if let Some(operation_id) = managed_id.take() {
                            operations.fail(
                                operation_id,
                                ManagedOperationError::new(
                                    OperationErrorCode::Internal,
                                    message.clone(),
                                ),
                            );
                        }
                        fail_startup(
                            &mut status,
                            &reporter,
                            attempt,
                            starting,
                            message,
                        );
                    }
                }
            }
            result = wait_runner(&mut running) => {
                let active = running.take().expect("completed runner exists");
                let bundle_id = active.bundle_id.clone();
                active.stop().await;
                let error = match result {
                    Ok(()) => "WDA runner exited unexpectedly".to_string(),
                    Err(error) => format!("WDA runner stopped: {error:?}"),
                };
                let error = bound_error(error);
                if let Some(operation_id) = managed_id.take() {
                    operations.fail(
                        operation_id,
                        ManagedOperationError::new(OperationErrorCode::Internal, error.clone()),
                    );
                }
                tracing::warn!(component = "wda_runner", operation = "exit", runner_bundle_id = %bundle_id, %error, "managed WebDriverAgent runner ended");
                reporter.unavailable(attempt, error.clone());
                status = WdaRunnerStatus {
                    phase: WdaRunnerPhase::Failed,
                    managed: false,
                    runner_bundle_id: Some(bundle_id),
                    last_error: Some(error),
                };
            }
        }
    }

    stop_runner_tasks(&mut startup, &mut running, "device session ended").await;
    if let Some(operation_id) = managed_id.take() {
        operations.cancel(operation_id, "device session ended");
    }
    reporter.stopped(attempt);
}

async fn stop_runner_tasks(
    startup: &mut Option<Startup>,
    running: &mut Option<RunningRunner>,
    startup_error: &str,
) {
    if let Some(starting) = startup.take() {
        // `run_until_wda_ready` owns the underlying XCTest task while it is
        // waiting for readiness. Let it finish so that a handle returned at
        // the cancellation boundary can be explicitly aborted instead of
        // being detached when the startup task is dropped.
        if let Ok(Ok(active)) = starting.task.await {
            active.stop().await;
        }
        let _ = starting.reply.send(Err(startup_error.into()));
    }
    if let Some(active) = running.take() {
        active.stop().await;
    }
}

async fn wait_startup(
    startup: &mut Option<Startup>,
) -> Result<Result<RunningRunner, String>, tokio::task::JoinError> {
    match startup.as_mut() {
        Some(startup) => (&mut startup.task).await,
        None => std::future::pending().await,
    }
}

async fn wait_runner(running: &mut Option<RunningRunner>) -> Result<(), idevice::IdeviceError> {
    let Some(runner) = running.as_mut() else {
        return std::future::pending::<Result<(), idevice::IdeviceError>>().await;
    };

    let wda = WdaClient::new(runner.provider.as_ref())
        .with_ports(
            runner
                .handle
                .as_ref()
                .expect("running WDA handle is present")
                .ports(),
        )
        .with_timeout(STATUS_PROBE_TIMEOUT);
    loop {
        match tokio::time::timeout(STATUS_PROBE_TIMEOUT, wda.status()).await {
            Ok(Ok(_)) => tokio::time::sleep(POLL_INTERVAL).await,
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                return Err(idevice::IdeviceError::UnknownErrorType(
                    "WDA runner stopped responding".into(),
                ));
            }
        }
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
        crate::device::is_developer_image_mounted_for_device(provider.as_ref()),
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

    let handle = XCUITestService::new(provider.clone())
        .run_until_wda_ready(config, STARTUP_TIMEOUT)
        .await
        .map_err(|error| format!("unable to start WDA runner: {error:?}"))?;

    Ok(RunningRunner {
        bundle_id,
        handle: Some(handle),
        provider,
    })
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
    fn errors_are_bounded_on_character_boundaries() {
        let error = bound_error("你".repeat(MAX_ERROR_CHARS + 1));
        assert_eq!(error.chars().count(), MAX_ERROR_CHARS);
    }
}
