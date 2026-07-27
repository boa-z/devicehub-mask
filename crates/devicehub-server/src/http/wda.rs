//! HTTP adapter for the supervised WebDriverAgent runner lifecycle.

use std::path::PathBuf;
use std::time::Duration;

use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use tokio::sync::oneshot;

use devicehub_runtime::{DeviceSessionCommand, SessionCommandSlot, WdaRunnerCommand};

type InputCmd = DeviceSessionCommand<PathBuf>;
type InputSink = SessionCommandSlot<PathBuf>;
type RequestSession = Option<Extension<devicehub_runtime::DeviceSessionClient<PathBuf>>>;

const DEVICE_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const WDA_RUNNER_START_TIMEOUT: Duration = Duration::from_secs(35);

#[derive(Clone, Default)]
pub struct WdaHttpState {
    input: InputSink,
}

impl WdaHttpState {
    pub fn new(input: InputSink) -> Self {
        Self { input }
    }

    fn input(&self, session: &RequestSession) -> InputSink {
        session
            .as_ref()
            .map(|session| session.commands.clone())
            .unwrap_or_else(|| self.input.clone())
    }
}

pub fn router<S>(state: WdaHttpState) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route(
            "/api/device/wda-runner",
            get(wda_runner_status)
                .put(start_wda_runner)
                .delete(stop_wda_runner),
        )
        .with_state(state)
}

#[derive(Deserialize)]
struct StartWdaRunnerRequest {
    bundle_id: String,
}

async fn wda_runner_status(
    State(state): State<WdaHttpState>,
    session: RequestSession,
) -> Result<Json<devicehub_core::WdaRunnerStatus>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    require_active_session(
        state
            .input(&session)
            .try_send(InputCmd::WdaRunner(WdaRunnerCommand::Status { reply })),
    )?;
    let status = tokio::time::timeout(DEVICE_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "WDA runner status timed out".into(),
            )
        })?
        .map_err(|_| session_ended())?;
    Ok(Json(status))
}

async fn start_wda_runner(
    State(state): State<WdaHttpState>,
    session: RequestSession,
    Json(request): Json<StartWdaRunnerRequest>,
) -> Result<Json<devicehub_core::WdaRunnerStatus>, (StatusCode, String)> {
    devicehub_core::validate_wda_runner_bundle_id(&request.bundle_id)
        .map_err(|error| (StatusCode::BAD_REQUEST, error.into()))?;
    let (reply, response) = oneshot::channel();
    require_active_session(state.input(&session).try_send(InputCmd::WdaRunner(
        WdaRunnerCommand::Start {
            bundle_id: request.bundle_id,
            reply,
        },
    )))?;
    let status = tokio::time::timeout(WDA_RUNNER_START_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "WDA runner startup timed out".into(),
            )
        })?
        .map_err(|_| session_ended())?
        .map_err(|error| {
            let status = if error.contains("already") {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_GATEWAY
            };
            (status, error)
        })?;
    Ok(Json(status))
}

async fn stop_wda_runner(
    State(state): State<WdaHttpState>,
    session: RequestSession,
) -> Result<Json<devicehub_core::WdaRunnerStatus>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    require_active_session(
        state
            .input(&session)
            .try_send(InputCmd::WdaRunner(WdaRunnerCommand::Stop { reply })),
    )?;
    let status = tokio::time::timeout(DEVICE_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "WDA runner stop timed out".into(),
            )
        })?
        .map_err(|_| session_ended())?
        .map_err(|error| (StatusCode::BAD_GATEWAY, error))?;
    Ok(Json(status))
}

fn require_active_session(sent: bool) -> Result<(), (StatusCode, String)> {
    sent.then_some(()).ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        )
    })
}

fn session_ended() -> (StatusCode, String) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "device session ended".into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use devicehub_core::{WdaRunnerPhase, WdaRunnerStatus};

    fn test_state() -> (WdaHttpState, tokio::sync::mpsc::UnboundedReceiver<InputCmd>) {
        let input = InputSink::default();
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        input.set(Some(sender));
        (WdaHttpState::new(input), receiver)
    }

    fn running() -> WdaRunnerStatus {
        WdaRunnerStatus {
            phase: WdaRunnerPhase::Running,
            managed: true,
            runner_bundle_id: Some("com.example.WDARunner.xctrunner".into()),
            last_error: None,
        }
    }

    #[tokio::test]
    async fn lifecycle_routes_validate_and_dispatch_commands() {
        let (state, mut commands) = test_state();
        let running = running();

        let status = tokio::spawn(wda_runner_status(State(state.clone()), None));
        let InputCmd::WdaRunner(WdaRunnerCommand::Status { reply }) =
            commands.recv().await.unwrap()
        else {
            panic!("expected status command");
        };
        reply.send(running.clone()).unwrap();
        assert_eq!(status.await.unwrap().unwrap().0, running);

        let start = tokio::spawn(start_wda_runner(
            State(state.clone()),
            None,
            Json(StartWdaRunnerRequest {
                bundle_id: "com.example.WDARunner.xctrunner".into(),
            }),
        ));
        let InputCmd::WdaRunner(WdaRunnerCommand::Start { bundle_id, reply }) =
            commands.recv().await.unwrap()
        else {
            panic!("expected start command");
        };
        assert_eq!(bundle_id, "com.example.WDARunner.xctrunner");
        reply.send(Ok(running.clone())).unwrap();
        assert_eq!(start.await.unwrap().unwrap().0, running);

        let stop = tokio::spawn(stop_wda_runner(State(state.clone()), None));
        let InputCmd::WdaRunner(WdaRunnerCommand::Stop { reply }) = commands.recv().await.unwrap()
        else {
            panic!("expected stop command");
        };
        reply.send(Ok(WdaRunnerStatus::default())).unwrap();
        assert_eq!(stop.await.unwrap().unwrap().0, WdaRunnerStatus::default());

        assert!(matches!(
            start_wda_runner(
                State(state),
                None,
                Json(StartWdaRunnerRequest {
                    bundle_id: "com.example.not-a-runner".into(),
                }),
            )
            .await,
            Err((StatusCode::BAD_REQUEST, _))
        ));
        assert!(commands.try_recv().is_err());
    }

    #[tokio::test]
    async fn routes_require_an_active_session() {
        let state = WdaHttpState::default();
        assert!(matches!(
            wda_runner_status(State(state.clone()), None).await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
        assert!(matches!(
            stop_wda_runner(State(state), None).await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
    }

    #[test]
    fn router_constructs_without_manager_or_host_state() {
        let _: Router = router(WdaHttpState::default());
    }
}
