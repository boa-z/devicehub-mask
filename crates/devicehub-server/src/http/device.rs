//! HTTP adapter for general active-device queries and controls.
//!
//! The adapter receives only the command endpoint, location observation, and
//! screenshot service for one runtime state graph. Host filesystem policy,
//! listeners, and the outer device manager remain outside this module.

use std::path::PathBuf;
use std::time::Duration;

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::response::IntoResponse;
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use devicehub_core::{
    LocationStatus, LocationStatusSlot, ManagedOperation, ManagedOperationRegistry, SessionPhase,
    validate_device_name, validate_paste_text,
};
use devicehub_runtime::{
    DeveloperModeCommand, DeviceControlError, DeviceControlService, DeviceSessionCommand,
    SessionCommandSlot,
};

type InputCmd = DeviceSessionCommand<PathBuf>;
type InputSink = SessionCommandSlot<PathBuf>;
type RequestSession = Option<Extension<devicehub_runtime::DeviceSessionClient<PathBuf>>>;

const DEVICE_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const SCREENSHOT_REQUEST_TIMEOUT: Duration = Duration::from_secs(25);
const READ_ONLY_REQUEST_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Clone)]
pub struct DeviceHttpState {
    input: InputSink,
    location: LocationStatusSlot,
    device_control: DeviceControlService<PathBuf>,
    operations: ManagedOperationRegistry,
}

impl DeviceHttpState {
    pub fn new(
        input: InputSink,
        location: LocationStatusSlot,
        device_control: DeviceControlService<PathBuf>,
        operations: ManagedOperationRegistry,
    ) -> Self {
        Self {
            input,
            location,
            device_control,
            operations,
        }
    }

    fn input(&self, session: &RequestSession) -> InputSink {
        session
            .as_ref()
            .map(|session| session.commands.clone())
            .unwrap_or_else(|| self.input.clone())
    }

    fn location(&self, session: &RequestSession) -> LocationStatusSlot {
        session
            .as_ref()
            .map(|session| session.location.clone())
            .unwrap_or_else(|| self.location.clone())
    }

    fn device_control(&self, session: &RequestSession) -> DeviceControlService<PathBuf> {
        session
            .as_ref()
            .map(|session| session.device_control.clone())
            .unwrap_or_else(|| self.device_control.clone())
    }

    fn operations(&self, session: &RequestSession) -> ManagedOperationRegistry {
        session
            .as_ref()
            .map(|session| session.operations.clone())
            .unwrap_or_else(|| self.operations.clone())
    }
}

pub fn router<S>(state: DeviceHttpState) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/device/details", get(device_details))
        .route("/api/device/operations", get(device_operations))
        .route("/api/device/companions", get(device_companions))
        .route("/api/device/home-screen", get(device_home_screen))
        .route("/api/device/wallpaper/{kind}", get(device_wallpaper))
        .route("/api/device/name", put(rename_device))
        .route(
            "/api/device/developer-mode/reveal",
            put(reveal_developer_mode),
        )
        .route("/api/device/screenshot", get(device_screenshot))
        .route("/api/device/text/paste", put(paste_device_text))
        .route("/api/device/lock", put(lock_device))
        .route("/api/device/restart", put(restart_device))
        .route("/api/device/shutdown", put(shutdown_device))
        .route(
            "/api/device/location",
            get(device_location)
                .put(set_device_location)
                .delete(clear_device_location),
        )
        .with_state(state)
}

async fn device_operations(
    State(state): State<DeviceHttpState>,
    session: RequestSession,
) -> Json<Vec<ManagedOperation>> {
    Json(state.operations(&session).snapshot())
}

#[derive(Deserialize)]
struct SetLocationRequest {
    latitude: f64,
    longitude: f64,
}

#[derive(Deserialize)]
struct PasteDeviceTextRequest {
    text: String,
}

async fn paste_device_text(
    State(state): State<DeviceHttpState>,
    session: RequestSession,
    Json(request): Json<PasteDeviceTextRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    validate_paste_text(&request.text).map_err(|error| (StatusCode::BAD_REQUEST, error.into()))?;
    let (reply, response) = oneshot::channel();
    require_active_session(state.input(&session).try_send(InputCmd::PasteText {
        text: request.text,
        reply,
    }))?;
    await_device_command(response, "paste text").await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn device_location(
    State(state): State<DeviceHttpState>,
    session: RequestSession,
) -> Json<LocationStatus> {
    Json(state.location(&session).get())
}

async fn set_device_location(
    State(state): State<DeviceHttpState>,
    session: RequestSession,
    Json(request): Json<SetLocationRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    validate_coordinates(request.latitude, request.longitude)?;
    let (reply, response) = oneshot::channel();
    require_active_session(state.input(&session).try_send(InputCmd::SetLocation {
        latitude: request.latitude,
        longitude: request.longitude,
        reply,
    }))?;
    await_device_command(response, "set location").await?;
    Ok(StatusCode::OK)
}

async fn clear_device_location(
    State(state): State<DeviceHttpState>,
    session: RequestSession,
) -> Result<StatusCode, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    require_active_session(
        state
            .input(&session)
            .try_send(InputCmd::ClearLocation { reply }),
    )?;
    await_device_command(response, "clear location").await?;
    Ok(StatusCode::OK)
}

fn validate_coordinates(latitude: f64, longitude: f64) -> Result<(), (StatusCode, String)> {
    if !latitude.is_finite()
        || !longitude.is_finite()
        || !(-90.0..=90.0).contains(&latitude)
        || !(-180.0..=180.0).contains(&longitude)
    {
        return Err((StatusCode::BAD_REQUEST, "invalid coordinates".into()));
    }
    Ok(())
}

async fn await_device_command(
    response: oneshot::Receiver<Result<(), String>>,
    operation: &str,
) -> Result<(), (StatusCode, String)> {
    tokio::time::timeout(DEVICE_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                format!("{operation} request timed out"),
            )
        })?
        .map_err(|_| session_ended())?
        .map_err(|error| (StatusCode::SERVICE_UNAVAILABLE, error))
}

async fn device_details(
    State(state): State<DeviceHttpState>,
    session: RequestSession,
) -> Result<Json<devicehub_core::DeviceDetails>, (StatusCode, String)> {
    require_session_ready(&session)?;
    let (reply, response) = oneshot::channel();
    require_active_session(
        state
            .input(&session)
            .try_send(InputCmd::GetDeviceDetails(reply)),
    )?;
    let details = tokio::time::timeout(DEVICE_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "device metadata request timed out".into(),
            )
        })?
        .map_err(|_| session_ended())?
        .map_err(|error| (StatusCode::SERVICE_UNAVAILABLE, error))?;
    Ok(Json(details))
}

fn require_session_ready(session: &RequestSession) -> Result<(), (StatusCode, String)> {
    let Some(Extension(session)) = session else {
        return Ok(());
    };
    let status = session.status.snapshot();
    if matches!(
        status.phase,
        SessionPhase::Connecting | SessionPhase::Recovering
    ) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            format!("device session is not ready: {}", status.message),
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
struct RenameDeviceRequest {
    name: String,
}

#[derive(Serialize)]
struct RenameDeviceResponse {
    name: String,
}

async fn rename_device(
    State(state): State<DeviceHttpState>,
    session: RequestSession,
    Json(request): Json<RenameDeviceRequest>,
) -> Result<Json<RenameDeviceResponse>, (StatusCode, String)> {
    let name = validate_device_name(&request.name)
        .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?;
    let (reply, response) = oneshot::channel();
    require_active_session(
        state
            .input(&session)
            .try_send(InputCmd::RenameDevice { name, reply }),
    )?;
    let name = tokio::time::timeout(DEVICE_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "device rename request timed out".into(),
            )
        })?
        .map_err(|_| session_ended())?
        .map_err(|error| (StatusCode::BAD_GATEWAY, error))?;
    Ok(Json(RenameDeviceResponse { name }))
}

async fn reveal_developer_mode(
    State(state): State<DeviceHttpState>,
    session: RequestSession,
) -> Result<Json<devicehub_runtime::DeveloperModePreparation>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    require_active_session(state.input(&session).try_send(InputCmd::DeveloperMode(
        DeveloperModeCommand::RevealOption { reply },
    )))?;
    let result = tokio::time::timeout(DEVICE_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "developer mode preparation request timed out".into(),
            )
        })?
        .map_err(|_| session_ended())?
        .map_err(|error| (StatusCode::BAD_GATEWAY, error))?;
    Ok(Json(result))
}

async fn device_screenshot(
    State(state): State<DeviceHttpState>,
    session: RequestSession,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let png = state
        .device_control(&session)
        .capture_screenshot(SCREENSHOT_REQUEST_TIMEOUT)
        .await
        .map_err(map_device_control_error)?;
    Ok((
        [(CONTENT_TYPE, "image/png"), (CACHE_CONTROL, "no-store")],
        png,
    ))
}

fn map_device_control_error(error: DeviceControlError) -> (StatusCode, String) {
    let status = match error {
        DeviceControlError::Unavailable | DeviceControlError::SessionEnded => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        DeviceControlError::Timeout(_) => StatusCode::GATEWAY_TIMEOUT,
        DeviceControlError::Operation(_) => StatusCode::BAD_GATEWAY,
    };
    (status, error.to_string())
}

#[derive(Clone, Copy)]
enum DevicePowerRequest {
    Lock,
    Restart,
    Shutdown,
}

async fn lock_device(
    State(state): State<DeviceHttpState>,
    session: RequestSession,
) -> Result<StatusCode, (StatusCode, String)> {
    dispatch_device_power_command(&state.input(&session), DevicePowerRequest::Lock).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn restart_device(
    State(state): State<DeviceHttpState>,
    session: RequestSession,
) -> Result<StatusCode, (StatusCode, String)> {
    dispatch_device_power_command(&state.input(&session), DevicePowerRequest::Restart).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn shutdown_device(
    State(state): State<DeviceHttpState>,
    session: RequestSession,
) -> Result<StatusCode, (StatusCode, String)> {
    dispatch_device_power_command(&state.input(&session), DevicePowerRequest::Shutdown).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn dispatch_device_power_command(
    input: &InputSink,
    action: DevicePowerRequest,
) -> Result<(), (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    let command = match action {
        DevicePowerRequest::Lock => InputCmd::LockDevice(reply),
        DevicePowerRequest::Restart => InputCmd::RestartDevice(reply),
        DevicePowerRequest::Shutdown => InputCmd::ShutdownDevice(reply),
    };
    require_active_session(input.try_send(command))?;
    tokio::time::timeout(DEVICE_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "device power request timed out".into(),
            )
        })?
        .map_err(|_| session_ended())?
        .map_err(|error| {
            let status = if error == "another device power command is already running" {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_GATEWAY
            };
            (status, error)
        })
}

async fn device_companions(
    State(state): State<DeviceHttpState>,
    session: RequestSession,
) -> Result<Json<Vec<devicehub_core::CompanionDevice>>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    require_active_session(
        state
            .input(&session)
            .try_send(InputCmd::ListCompanionDevices(reply)),
    )?;
    let devices = tokio::time::timeout(DEVICE_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "companion device request timed out".into(),
            )
        })?
        .map_err(|_| session_ended())?
        .map_err(|error| (StatusCode::BAD_GATEWAY, error))?;
    Ok(Json(devices))
}

async fn device_home_screen(
    State(state): State<DeviceHttpState>,
    session: RequestSession,
) -> Result<Json<devicehub_core::HomeScreenLayout>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    require_active_session(
        state
            .input(&session)
            .try_send(InputCmd::GetHomeScreenLayout(reply)),
    )?;
    let layout = tokio::time::timeout(READ_ONLY_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "home screen layout request timed out".into(),
            )
        })?
        .map_err(|_| session_ended())?
        .map_err(|error| (StatusCode::BAD_GATEWAY, error))?;
    Ok(Json(layout))
}

async fn device_wallpaper(
    State(state): State<DeviceHttpState>,
    session: RequestSession,
    Path(kind): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let kind = devicehub_core::WallpaperKind::parse(&kind).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "wallpaper kind must be home or lock".into(),
        )
    })?;
    let (reply, response) = oneshot::channel();
    require_active_session(
        state
            .input(&session)
            .try_send(InputCmd::GetWallpaper { kind, reply }),
    )?;
    let image = tokio::time::timeout(READ_ONLY_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "wallpaper preview request timed out".into(),
            )
        })?
        .map_err(|_| session_ended())?
        .map_err(|error| (StatusCode::BAD_GATEWAY, error))?;
    Ok((
        [(CONTENT_TYPE, "image/png"), (CACHE_CONTROL, "no-store")],
        image,
    ))
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
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    fn test_state() -> (DeviceHttpState, UnboundedReceiver<InputCmd>) {
        let input = InputSink::default();
        let (sender, receiver) = unbounded_channel();
        input.set(Some(sender));
        let (application, _control) = devicehub_runtime::RuntimeClientFixture::<PathBuf>::default()
            .with_commands(input.clone())
            .build();
        (
            DeviceHttpState::new(
                input,
                application.device.location,
                application.device.device_control,
                application.device.operations,
            ),
            receiver,
        )
    }

    #[test]
    fn coordinates_are_finite_and_geographically_bounded() {
        assert!(validate_coordinates(-90.0, -180.0).is_ok());
        assert!(validate_coordinates(90.0, 180.0).is_ok());
        for coordinates in [
            (90.000_001, 0.0),
            (0.0, 180.000_001),
            (f64::NAN, 0.0),
            (0.0, f64::INFINITY),
        ] {
            assert_eq!(
                validate_coordinates(coordinates.0, coordinates.1)
                    .unwrap_err()
                    .0,
                StatusCode::BAD_REQUEST
            );
        }
    }

    #[test]
    fn metadata_requests_do_not_queue_while_the_session_is_connecting() {
        let status = devicehub_core::StatusSlot::default();
        status.set_phase(
            devicehub_core::SessionPhase::Connecting,
            "connecting to device...",
        );
        let (client, _control) = devicehub_runtime::RuntimeClientFixture::<PathBuf>::default()
            .with_status(status)
            .build();
        let session = Some(Extension(client.device));

        let error = require_session_ready(&session).unwrap_err();
        assert_eq!(error.0, StatusCode::SERVICE_UNAVAILABLE);
        assert!(error.1.contains("connecting to device"));
    }

    #[tokio::test]
    async fn unavailable_metadata_is_reported_as_retryable() {
        let (state, mut commands) = test_state();
        let request = tokio::spawn(device_details(State(state), None));
        let InputCmd::GetDeviceDetails(reply) = commands.recv().await.unwrap() else {
            panic!("expected device details command");
        };
        reply
            .send(Err("device metadata is temporarily unavailable".into()))
            .unwrap();

        let error = request.await.unwrap().unwrap_err();
        assert_eq!(error.0, StatusCode::SERVICE_UNAVAILABLE);
        assert!(error.1.contains("temporarily unavailable"));
    }

    #[tokio::test]
    async fn location_and_paste_dispatch_validated_commands() {
        let (state, mut commands) = test_state();
        let set = tokio::spawn(set_device_location(
            State(state.clone()),
            None,
            Json(SetLocationRequest {
                latitude: 25.033,
                longitude: 121.5654,
            }),
        ));
        let InputCmd::SetLocation {
            latitude,
            longitude,
            reply,
        } = commands.recv().await.unwrap()
        else {
            panic!("expected set location command");
        };
        assert_eq!((latitude, longitude), (25.033, 121.5654));
        reply.send(Ok(())).unwrap();
        assert_eq!(set.await.unwrap().unwrap(), StatusCode::OK);

        let clear = tokio::spawn(clear_device_location(State(state.clone()), None));
        let InputCmd::ClearLocation { reply } = commands.recv().await.unwrap() else {
            panic!("expected clear location command");
        };
        reply.send(Ok(())).unwrap();
        assert_eq!(clear.await.unwrap().unwrap(), StatusCode::OK);

        let paste = tokio::spawn(paste_device_text(
            State(state.clone()),
            None,
            Json(PasteDeviceTextRequest {
                text: "hello".into(),
            }),
        ));
        let InputCmd::PasteText { text, reply } = commands.recv().await.unwrap() else {
            panic!("expected paste command");
        };
        assert_eq!(text, "hello");
        reply.send(Ok(())).unwrap();
        assert_eq!(paste.await.unwrap().unwrap(), StatusCode::NO_CONTENT);

        assert!(matches!(
            paste_device_text(
                State(state),
                None,
                Json(PasteDeviceTextRequest {
                    text: "bad\0text".into(),
                }),
            )
            .await,
            Err((StatusCode::BAD_REQUEST, _))
        ));
    }

    #[tokio::test]
    async fn rename_and_developer_mode_dispatch_typed_commands() {
        let (state, mut commands) = test_state();
        let rename = tokio::spawn(rename_device(
            State(state.clone()),
            None,
            Json(RenameDeviceRequest {
                name: "  Test iPhone  ".into(),
            }),
        ));
        let InputCmd::RenameDevice { name, reply } = commands.recv().await.unwrap() else {
            panic!("expected rename command");
        };
        assert_eq!(name, "Test iPhone");
        reply.send(Ok(name)).unwrap();
        assert_eq!(rename.await.unwrap().unwrap().0.name, "Test iPhone");

        let reveal = tokio::spawn(reveal_developer_mode(State(state), None));
        let InputCmd::DeveloperMode(DeveloperModeCommand::RevealOption { reply }) =
            commands.recv().await.unwrap()
        else {
            panic!("expected developer mode command");
        };
        reply
            .send(Ok(devicehub_runtime::DeveloperModePreparation {
                already_enabled: false,
            }))
            .unwrap();
        assert!(!reveal.await.unwrap().unwrap().0.already_enabled);
    }

    #[tokio::test]
    async fn read_only_queries_preserve_png_cache_policy() {
        let (state, mut commands) = test_state();
        let companions = tokio::spawn(device_companions(State(state.clone()), None));
        let InputCmd::ListCompanionDevices(reply) = commands.recv().await.unwrap() else {
            panic!("expected companion query");
        };
        reply.send(Ok(Vec::new())).unwrap();
        assert!(companions.await.unwrap().unwrap().0.is_empty());

        let home = tokio::spawn(device_home_screen(State(state.clone()), None));
        let InputCmd::GetHomeScreenLayout(reply) = commands.recv().await.unwrap() else {
            panic!("expected home screen query");
        };
        reply
            .send(Ok(devicehub_core::HomeScreenLayout {
                apps: Vec::new(),
                page_count: 0,
                metrics: None,
                truncated: false,
            }))
            .unwrap();
        assert!(home.await.unwrap().unwrap().0.apps.is_empty());

        let invalid = device_wallpaper(State(state.clone()), None, Path("desktop".into()))
            .await
            .err()
            .unwrap();
        assert_eq!(invalid.0, StatusCode::BAD_REQUEST);

        let wallpaper = tokio::spawn(device_wallpaper(State(state), None, Path("lock".into())));
        let InputCmd::GetWallpaper { kind, reply } = commands.recv().await.unwrap() else {
            panic!("expected wallpaper query");
        };
        assert_eq!(kind, devicehub_core::WallpaperKind::Lock);
        reply.send(Ok(vec![1, 2, 3])).unwrap();
        let response = wallpaper.await.unwrap().unwrap().into_response();
        assert_eq!(response.headers().get(CONTENT_TYPE).unwrap(), "image/png");
        assert_eq!(response.headers().get(CACHE_CONTROL).unwrap(), "no-store");
    }

    #[tokio::test]
    async fn screenshot_and_power_controls_are_session_scoped() {
        let (state, mut commands) = test_state();
        let screenshot = tokio::spawn(device_screenshot(State(state.clone()), None));
        let InputCmd::TakeScreenshot(reply) = commands.recv().await.unwrap() else {
            panic!("expected screenshot command");
        };
        reply.send(Ok(vec![1, 2, 3])).unwrap();
        let response = screenshot.await.unwrap().unwrap().into_response();
        assert_eq!(response.headers().get(CONTENT_TYPE).unwrap(), "image/png");
        assert_eq!(response.headers().get(CACHE_CONTROL).unwrap(), "no-store");

        for action in [
            DevicePowerRequest::Lock,
            DevicePowerRequest::Restart,
            DevicePowerRequest::Shutdown,
        ] {
            let request_state = state.clone();
            let request = tokio::spawn(async move {
                dispatch_device_power_command(&request_state.input, action).await
            });
            match commands.recv().await.unwrap() {
                InputCmd::LockDevice(reply)
                | InputCmd::RestartDevice(reply)
                | InputCmd::ShutdownDevice(reply) => reply.send(Ok(())).unwrap(),
                _ => panic!("expected power command"),
            }
            request.await.unwrap().unwrap();
        }

        let conflict = tokio::spawn(restart_device(State(state), None));
        let InputCmd::RestartDevice(reply) = commands.recv().await.unwrap() else {
            panic!("expected restart command");
        };
        reply
            .send(Err("another device power command is already running".into()))
            .unwrap();
        assert!(matches!(
            conflict.await.unwrap(),
            Err((StatusCode::CONFLICT, _))
        ));
    }

    #[tokio::test]
    async fn routes_require_an_active_session() {
        let (state, _commands) = test_state();
        state.input.set(None);

        assert!(matches!(
            device_details(State(state.clone()), None).await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
        assert!(matches!(
            rename_device(
                State(state.clone()),
                None,
                Json(RenameDeviceRequest {
                    name: "Test iPhone".into(),
                }),
            )
            .await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
        assert!(device_screenshot(State(state.clone()), None).await.is_err());
        assert!(device_companions(State(state.clone()), None).await.is_err());
        assert!(
            device_home_screen(State(state.clone()), None)
                .await
                .is_err()
        );
        assert!(lock_device(State(state), None).await.is_err());
    }

    #[test]
    fn router_constructs_without_manager_or_host_state() {
        let (state, _commands) = test_state();
        let _: Router = router(state);
    }
}
