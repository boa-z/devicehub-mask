//! HTTP adapter for application discovery, lifecycle control, and console capture.
//!
//! The active device session owns all CoreDevice, DVT, and console resources.
//! This adapter validates requests, applies bounded response deadlines, and
//! exposes the shared progress snapshot without owning background work.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::response::IntoResponse;
use axum::routing::{delete, get, put};
use axum::{Json, Router};
use serde::Deserialize;
use tokio::sync::oneshot;

use crate::protocol::{AppOperationSlot, InputCmd, InputSink};

const DEVICE_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Clone, Default)]
pub(crate) struct AppHttpState {
    input: InputSink,
    operation: AppOperationSlot,
}

impl AppHttpState {
    pub(crate) fn new(input: InputSink, operation: AppOperationSlot) -> Self {
        Self { input, operation }
    }
}

pub(crate) fn router<S>(state: AppHttpState) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/device/apps", get(device_apps))
        .route("/api/device/apps/{bundle_id}/icon", get(device_app_icon))
        .route("/api/device/apps/operation", get(app_operation))
        .route("/api/device/apps/{bundle_id}", delete(uninstall_app))
        .route("/api/device/apps/{bundle_id}/launch", put(launch_app))
        .route(
            "/api/device/apps/{bundle_id}/console",
            put(start_app_console),
        )
        .route("/api/device/apps/{bundle_id}/stop", put(stop_app))
        .route(
            "/api/device/app-console",
            get(app_console_snapshot).delete(stop_app_console),
        )
        .with_state(state)
}

#[derive(Debug, Default, Deserialize)]
struct DeviceAppsQuery {
    #[serde(default)]
    include_system: bool,
    #[serde(default)]
    include_app_clips: bool,
}

async fn device_apps(
    State(state): State<AppHttpState>,
    Query(query): Query<DeviceAppsQuery>,
) -> Result<Json<Vec<crate::protocol::DeviceApp>>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    require_active_session(state.input.try_send(InputCmd::Apps(
        devicehub_runtime::AppCommand::List {
            include_system: query.include_system,
            include_app_clips: query.include_app_clips,
            reply,
        },
    )))?;
    let apps = tokio::time::timeout(devicehub_runtime::APP_LIST_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "app list request timed out".into(),
            )
        })?
        .map_err(|_| session_ended())?
        .map_err(|error| (StatusCode::BAD_GATEWAY, error))?;
    Ok(Json(apps))
}

async fn device_app_icon(
    State(state): State<AppHttpState>,
    Path(bundle_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    validate_bundle_identifier(&bundle_id)?;
    let (reply, response) = oneshot::channel();
    require_active_session(
        state
            .input
            .try_send(InputCmd::GetAppIcon { bundle_id, reply }),
    )?;
    let icon = tokio::time::timeout(DEVICE_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "app icon request timed out".into(),
            )
        })?
        .map_err(|_| session_ended())?
        .map_err(|error| (StatusCode::BAD_GATEWAY, error))?;
    Ok((
        [
            (CONTENT_TYPE, "image/png"),
            (CACHE_CONTROL, "private, max-age=300"),
        ],
        icon,
    ))
}

async fn app_operation(
    State(state): State<AppHttpState>,
) -> Json<crate::protocol::AppOperationView> {
    Json(state.operation.get())
}

async fn uninstall_app(
    State(state): State<AppHttpState>,
    Path(bundle_id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    validate_bundle_identifier(&bundle_id)?;
    let (reply, response) = oneshot::channel();
    require_active_session(state.input.try_send(InputCmd::Apps(
        devicehub_runtime::AppCommand::Uninstall { bundle_id, reply },
    )))?;
    await_app_operation_acceptance(response, "app uninstall").await?;
    Ok(StatusCode::ACCEPTED)
}

async fn await_app_operation_acceptance(
    response: oneshot::Receiver<Result<(), String>>,
    operation: &str,
) -> Result<(), (StatusCode, String)> {
    let result = tokio::time::timeout(DEVICE_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                format!("{operation} request timed out"),
            )
        })?
        .map_err(|_| session_ended())?;
    result.map_err(|error| {
        let status = if error == "another app operation is already running" {
            StatusCode::CONFLICT
        } else {
            StatusCode::BAD_REQUEST
        };
        (status, error)
    })
}

async fn launch_app(
    State(state): State<AppHttpState>,
    Path(bundle_id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    validate_bundle_identifier(&bundle_id)?;
    let (reply, response) = oneshot::channel();
    require_active_session(state.input.try_send(InputCmd::Apps(
        devicehub_runtime::AppCommand::Launch { bundle_id, reply },
    )))?;
    tokio::time::timeout(devicehub_runtime::APP_CONTROL_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "app launch request timed out".into(),
            )
        })?
        .map_err(|_| session_ended())?
        .map_err(|error| (StatusCode::BAD_GATEWAY, error))?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Default, Deserialize)]
struct AppConsoleQuery {
    after: Option<u64>,
    #[serde(default)]
    clear: bool,
}

async fn start_app_console(
    State(state): State<AppHttpState>,
    Path(bundle_id): Path<String>,
) -> Result<Json<devicehub_runtime::AppConsoleSnapshot>, (StatusCode, String)> {
    validate_bundle_identifier(&bundle_id)?;
    let (reply, response) = oneshot::channel();
    require_active_session(state.input.try_send(InputCmd::AppConsole(
        devicehub_runtime::AppConsoleCommand::Start { bundle_id, reply },
    )))?;
    let snapshot = tokio::time::timeout(DEVICE_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "application console startup timed out".into(),
            )
        })?
        .map_err(|_| session_ended())?
        .map_err(|error| (StatusCode::BAD_GATEWAY, error))?;
    Ok(Json(snapshot))
}

async fn app_console_snapshot(
    State(state): State<AppHttpState>,
    Query(query): Query<AppConsoleQuery>,
) -> Result<Json<devicehub_runtime::AppConsoleSnapshot>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    require_active_session(state.input.try_send(InputCmd::AppConsole(
        devicehub_runtime::AppConsoleCommand::Snapshot {
            after: query.after,
            reply,
        },
    )))?;
    let snapshot = tokio::time::timeout(DEVICE_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "application console request timed out".into(),
            )
        })?
        .map_err(|_| session_ended())?;
    Ok(Json(snapshot))
}

async fn stop_app_console(
    State(state): State<AppHttpState>,
    Query(query): Query<AppConsoleQuery>,
) -> Result<Json<devicehub_runtime::AppConsoleSnapshot>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    require_active_session(state.input.try_send(InputCmd::AppConsole(
        devicehub_runtime::AppConsoleCommand::Stop {
            clear: query.clear,
            reply,
        },
    )))?;
    let snapshot = tokio::time::timeout(DEVICE_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "application console stop timed out".into(),
            )
        })?
        .map_err(|_| session_ended())?;
    Ok(Json(snapshot))
}

async fn stop_app(
    State(state): State<AppHttpState>,
    Path(bundle_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    validate_bundle_identifier(&bundle_id)?;
    let (reply, response) = oneshot::channel();
    require_active_session(state.input.try_send(InputCmd::Apps(
        devicehub_runtime::AppCommand::Stop { bundle_id, reply },
    )))?;
    let was_running =
        tokio::time::timeout(devicehub_runtime::APP_CONTROL_REQUEST_TIMEOUT, response)
            .await
            .map_err(|_| {
                (
                    StatusCode::GATEWAY_TIMEOUT,
                    "app stop request timed out".into(),
                )
            })?
            .map_err(|_| session_ended())?
            .map_err(|error| (StatusCode::BAD_GATEWAY, error))?;
    Ok(Json(serde_json::json!({ "was_running": was_running })))
}

fn require_active_session(sent: bool) -> Result<(), (StatusCode, String)> {
    if sent {
        Ok(())
    } else {
        Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ))
    }
}

fn session_ended() -> (StatusCode, String) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "device session ended".into(),
    )
}

fn validate_bundle_identifier(bundle_id: &str) -> Result<(), (StatusCode, String)> {
    valid_bundle_identifier(bundle_id)
        .then_some(())
        .ok_or((StatusCode::BAD_REQUEST, "invalid bundle identifier".into()))
}

fn valid_bundle_identifier(bundle_id: &str) -> bool {
    !bundle_id.is_empty()
        && bundle_id.len() <= 255
        && bundle_id.contains('.')
        && bundle_id.split('.').all(|part| {
            !part.is_empty()
                && part.len() <= 63
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    fn test_state() -> (AppHttpState, UnboundedReceiver<InputCmd>) {
        let input = InputSink::default();
        let (input_tx, input_rx) = unbounded_channel();
        input.set(Some(input_tx));
        (
            AppHttpState::new(input, AppOperationSlot::default()),
            input_rx,
        )
    }

    fn console_snapshot_fixture(
        phase: devicehub_runtime::AppConsolePhase,
    ) -> devicehub_runtime::AppConsoleSnapshot {
        devicehub_runtime::AppConsoleSnapshot {
            phase,
            bundle_id: Some("com.example.game".into()),
            started_at_ms: Some(1),
            ended_at_ms: None,
            total_bytes: 5,
            total_lines: 1,
            dropped_lines: 0,
            next_sequence: 2,
            reset: false,
            lines: vec![devicehub_runtime::AppConsoleLine {
                sequence: 1,
                text: "ready".into(),
            }],
            last_error: None,
        }
    }

    #[tokio::test]
    async fn app_list_scope_is_dispatched_to_the_device_session() {
        let (state, mut input_rx) = test_state();
        let request = tokio::spawn(device_apps(
            State(state),
            Query(DeviceAppsQuery {
                include_system: true,
                include_app_clips: true,
            }),
        ));
        let InputCmd::Apps(devicehub_runtime::AppCommand::List {
            include_system,
            include_app_clips,
            reply,
        }) = input_rx.recv().await.unwrap()
        else {
            panic!("expected app list command");
        };
        assert!(include_system);
        assert!(include_app_clips);
        reply.send(Ok(Vec::new())).unwrap();
        assert!(request.await.unwrap().unwrap().0.is_empty());
    }

    #[tokio::test]
    async fn app_routes_require_an_active_session() {
        let (state, _input_rx) = test_state();
        state.input.set(None);
        assert!(matches!(
            device_apps(State(state.clone()), Query(DeviceAppsQuery::default())).await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
        assert!(matches!(
            device_app_icon(State(state.clone()), Path("com.example.game".into())).await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
        assert!(matches!(
            uninstall_app(State(state), Path("com.example.game".into())).await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
    }

    #[tokio::test]
    async fn app_lifecycle_validates_and_dispatches_bundle_identifiers() {
        let (state, mut input_rx) = test_state();
        for bundle_id in ["", "no-domain", "com.example.bad value", "com/example/app"] {
            assert!(matches!(
                launch_app(State(state.clone()), Path(bundle_id.into())).await,
                Err((StatusCode::BAD_REQUEST, _))
            ));
            assert!(matches!(
                stop_app(State(state.clone()), Path(bundle_id.into())).await,
                Err((StatusCode::BAD_REQUEST, _))
            ));
            assert!(matches!(
                uninstall_app(State(state.clone()), Path(bundle_id.into())).await,
                Err((StatusCode::BAD_REQUEST, _))
            ));
        }
        assert!(input_rx.try_recv().is_err());

        let launch = tokio::spawn(launch_app(
            State(state.clone()),
            Path("com.example.game".into()),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::Apps(devicehub_runtime::AppCommand::Launch { bundle_id, reply }) => {
                assert_eq!(bundle_id, "com.example.game");
                reply.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(launch.await.unwrap().unwrap(), StatusCode::NO_CONTENT);

        let stop = tokio::spawn(stop_app(State(state), Path("com.example.game".into())));
        match input_rx.recv().await.unwrap() {
            InputCmd::Apps(devicehub_runtime::AppCommand::Stop { bundle_id, reply }) => {
                assert_eq!(bundle_id, "com.example.game");
                reply.send(Ok(true)).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        let Json(result) = stop.await.unwrap().unwrap();
        assert_eq!(result, serde_json::json!({ "was_running": true }));
    }

    #[tokio::test]
    async fn app_console_endpoints_validate_and_dispatch_session_commands() {
        let (state, mut input_rx) = test_state();
        assert!(matches!(
            start_app_console(State(state.clone()), Path("bad value".into())).await,
            Err((StatusCode::BAD_REQUEST, _))
        ));
        assert!(input_rx.try_recv().is_err());

        let start = tokio::spawn(start_app_console(
            State(state.clone()),
            Path("com.example.game".into()),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::AppConsole(devicehub_runtime::AppConsoleCommand::Start {
                bundle_id,
                reply,
            }) => {
                assert_eq!(bundle_id, "com.example.game");
                reply
                    .send(Ok(console_snapshot_fixture(
                        devicehub_runtime::AppConsolePhase::Running,
                    )))
                    .unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(
            start.await.unwrap().unwrap().0.phase,
            devicehub_runtime::AppConsolePhase::Running
        );

        let snapshot = tokio::spawn(app_console_snapshot(
            State(state.clone()),
            Query(AppConsoleQuery {
                after: Some(7),
                clear: false,
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::AppConsole(devicehub_runtime::AppConsoleCommand::Snapshot {
                after,
                reply,
            }) => {
                assert_eq!(after, Some(7));
                reply
                    .send(console_snapshot_fixture(
                        devicehub_runtime::AppConsolePhase::Running,
                    ))
                    .unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(snapshot.await.unwrap().unwrap().0.lines[0].text, "ready");

        let stop = tokio::spawn(stop_app_console(
            State(state),
            Query(AppConsoleQuery {
                after: None,
                clear: true,
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::AppConsole(devicehub_runtime::AppConsoleCommand::Stop { clear, reply }) => {
                assert!(clear);
                reply
                    .send(console_snapshot_fixture(
                        devicehub_runtime::AppConsolePhase::Stopped,
                    ))
                    .unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(
            stop.await.unwrap().unwrap().0.phase,
            devicehub_runtime::AppConsolePhase::Stopped
        );
    }

    #[tokio::test]
    async fn app_icon_validates_and_dispatches_bundle_identifier() {
        let (state, mut input_rx) = test_state();
        assert!(matches!(
            device_app_icon(State(state.clone()), Path("bad value".into())).await,
            Err((StatusCode::BAD_REQUEST, _))
        ));
        assert!(input_rx.try_recv().is_err());
        let request = tokio::spawn(device_app_icon(
            State(state),
            Path("com.example.game".into()),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::GetAppIcon { bundle_id, reply } => {
                assert_eq!(bundle_id, "com.example.game");
                reply.send(Ok(vec![1, 2, 3])).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        let response = request.await.unwrap().unwrap().into_response();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers().get(CONTENT_TYPE).unwrap(), "image/png");
    }

    #[tokio::test]
    async fn app_operation_endpoint_returns_shared_state() {
        let (state, _input_rx) = test_state();
        let id = state
            .operation
            .start(
                devicehub_core::AppOperationKind::Uninstall,
                "com.example.app".into(),
            )
            .unwrap();
        state.operation.update(id, "uninstalling", Some(42));
        let view = app_operation(State(state)).await.0;
        assert_eq!(view.id, id);
        assert_eq!(view.progress, Some(42));
    }
}
