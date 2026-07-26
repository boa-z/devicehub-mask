#[cfg(test)]
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[cfg(test)]
use axum::extract::Query;
use axum::extract::{Path, Request, State, WebSocketUpgrade};
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, SEC_WEBSOCKET_PROTOCOL};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use serde_json::json;
use tokio::sync::oneshot;
use tower_http::cors::CorsLayer;

use crate::device_runtime::{ControlCmd, InputCmd, InputSink};
#[cfg(test)]
use crate::websocket_input::{
    ClientVideoFeedback, WebContact, handle_client_message, send_all_up, valid_frontend_metrics,
    valid_keyboard_usage, validate_contacts,
};
use devicehub_core::{
    ForgetDeviceResult, LocationStatus, PairDeviceResult, VideoCounters, validate_device_name,
    validate_paste_text,
};
use devicehub_runtime::ClipboardSlot;

#[derive(Clone)]
pub struct AppState {
    pub application: devicehub_runtime::RuntimeClient<PathBuf>,
    pub performance_http: crate::http_performance::PerformanceHttpState,
    pub profiles_http: crate::http_profiles::ProfileHttpState,
    pub storage_http: crate::http_storage::StorageHttpState,
    pub diagnostics_http: crate::http_diagnostics::DiagnosticsHttpState,
    pub apps_http: crate::http_apps::AppHttpState,
    pub crash_reports_http: crate::http_crash_reports::CrashReportHttpState,
    pub browser_frames: crate::browser_video::BrowserVideoSlot,
    pub clipboard: ClipboardSlot,
    pub developer_image: crate::developer_image::DeveloperImageMountSlot,
    pub video_counters: VideoCounters,
    pub input: InputSink,
}

#[derive(Clone)]
struct ApiToken(Arc<str>);

pub fn router(state: AppState, token: String) -> Router {
    let performance_routes = crate::http_performance::router(state.performance_http.clone());
    let profile_routes = crate::http_profiles::router(state.profiles_http.clone());
    let storage_routes = crate::http_storage::router(state.storage_http.clone());
    let diagnostics_routes = crate::http_diagnostics::router(state.diagnostics_http.clone());
    let app_routes = crate::http_apps::router(state.apps_http.clone());
    let crash_report_routes = crate::http_crash_reports::router(state.crash_reports_http.clone());
    Router::new()
        .route("/api/status", get(status))
        .merge(performance_routes)
        .merge(profile_routes)
        .merge(storage_routes)
        .merge(diagnostics_routes)
        .merge(app_routes)
        .merge(crash_report_routes)
        .route("/api/devices/refresh", put(refresh_devices))
        .route("/api/devices/{udid}/connect", put(connect_device))
        .route("/api/devices/{udid}/reconnect", put(reconnect_device))
        .route(
            "/api/devices/{udid}/pair",
            put(pair_device).delete(forget_device),
        )
        .route("/api/device/details", get(device_details))
        .route("/api/device/companions", get(device_companions))
        .route("/api/device/home-screen", get(device_home_screen))
        .route("/api/device/wallpaper/{kind}", get(device_wallpaper))
        .route(
            "/api/device/wda-runner",
            get(wda_runner_status)
                .put(start_wda_runner)
                .delete(stop_wda_runner),
        )
        .route("/api/device/name", put(rename_device))
        .route(
            "/api/device/developer-mode/reveal",
            put(reveal_developer_mode),
        )
        .route(
            "/api/device/developer-image",
            get(developer_image_status)
                .put(start_developer_image_mount)
                .delete(stop_developer_image_mount),
        )
        .route(
            "/api/device/developer-image/unmount",
            put(unmount_developer_image),
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
        .route(
            "/api/device/provisioning-profiles",
            get(device_provisioning_profiles).put(install_provisioning_profile),
        )
        .route(
            "/api/device/provisioning-profiles/{uuid}",
            delete(remove_provisioning_profile),
        )
        .route(
            "/api/device/provisioning-profiles/{uuid}/trust",
            put(trust_provisioning_profile_signer),
        )
        .route("/api/ws", get(ws_upgrade))
        .layer(from_fn_with_state(
            ApiToken(Arc::from(token)),
            authorize_private_api,
        ))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn authorize_private_api(
    State(token): State<ApiToken>,
    request: Request,
    next: Next,
) -> Response {
    if private_api_authorized(request.headers(), token.0.as_ref()) {
        next.run(request).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

fn private_api_authorized(headers: &HeaderMap, token: &str) -> bool {
    let bearer_matches = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|value| value == token);
    let websocket_protocol_matches = headers
        .get(SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(',').any(|protocol| protocol.trim() == token));
    bearer_matches || websocket_protocol_matches
}

async fn status(State(state): State<AppState>) -> Json<crate::web_status::StatusView> {
    Json(crate::web_status::snapshot(&state.application))
}

async fn refresh_devices(State(state): State<AppState>) -> StatusCode {
    let _ = state.application.control.send(ControlCmd::Refresh);
    StatusCode::ACCEPTED
}

async fn connect_device(State(state): State<AppState>, Path(udid): Path<String>) -> StatusCode {
    let _ = state.application.control.send(ControlCmd::Connect(udid));
    StatusCode::ACCEPTED
}

async fn reconnect_device(State(state): State<AppState>, Path(udid): Path<String>) -> StatusCode {
    let _ = state.application.control.send(ControlCmd::Reconnect(udid));
    StatusCode::ACCEPTED
}

async fn pair_device(
    State(state): State<AppState>,
    Path(selection_id): Path<String>,
) -> Result<Json<PairDeviceResult>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    state
        .application
        .control
        .send(ControlCmd::Pair {
            selection_id,
            reply,
        })
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device session manager is not running".into(),
            )
        })?;
    let result = tokio::time::timeout(PAIRING_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "device pairing request timed out".into(),
            )
        })?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device pairing request was interrupted".into(),
            )
        })?;
    Ok(Json(result))
}

async fn forget_device(
    State(state): State<AppState>,
    Path(selection_id): Path<String>,
) -> Result<Json<ForgetDeviceResult>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    state
        .application
        .control
        .send(ControlCmd::Forget {
            selection_id,
            reply,
        })
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device session manager is not running".into(),
            )
        })?;
    let result = tokio::time::timeout(FORGET_DEVICE_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "device trust removal request timed out".into(),
            )
        })?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device trust removal request was interrupted".into(),
            )
        })?;
    Ok(Json(result))
}

const DEVICE_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const PAIRING_REQUEST_TIMEOUT: Duration = Duration::from_secs(95);
const FORGET_DEVICE_REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
const WDA_RUNNER_START_TIMEOUT: Duration = Duration::from_secs(35);
const PROVISIONING_REQUEST_TIMEOUT: Duration = Duration::from_secs(22);
const SCREENSHOT_REQUEST_TIMEOUT: Duration = Duration::from_secs(25);

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
    State(state): State<AppState>,
    Json(request): Json<PasteDeviceTextRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    validate_paste_text(&request.text).map_err(|error| (StatusCode::BAD_REQUEST, error.into()))?;
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::PasteText {
        text: request.text,
        reply,
    }) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    await_device_command(response, "paste text").await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn device_location(State(state): State<AppState>) -> Json<LocationStatus> {
    Json(state.application.location.get())
}

async fn set_device_location(
    State(state): State<AppState>,
    Json(request): Json<SetLocationRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    validate_coordinates(request.latitude, request.longitude)?;
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::SetLocation {
        latitude: request.latitude,
        longitude: request.longitude,
        reply,
    }) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    await_location_response(response, "set location").await?;
    Ok(StatusCode::OK)
}

async fn clear_device_location(
    State(state): State<AppState>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::ClearLocation { reply }) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    await_location_response(response, "clear location").await?;
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

async fn await_location_response(
    response: oneshot::Receiver<Result<(), String>>,
    operation: &str,
) -> Result<(), (StatusCode, String)> {
    await_device_command(response, operation).await
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
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device session ended".into(),
            )
        })?
        .map_err(|error| (StatusCode::SERVICE_UNAVAILABLE, error))
}

async fn device_details(
    State(state): State<AppState>,
) -> Result<Json<devicehub_core::DeviceDetails>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::GetDeviceDetails(reply)) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    let details = tokio::time::timeout(DEVICE_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "device metadata request timed out".into(),
            )
        })?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device session ended".into(),
            )
        })?
        .map_err(|error| (StatusCode::BAD_GATEWAY, error))?;
    Ok(Json(details))
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
    State(state): State<AppState>,
    Json(request): Json<RenameDeviceRequest>,
) -> Result<Json<RenameDeviceResponse>, (StatusCode, String)> {
    let name = validate_device_name(&request.name)
        .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?;
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::RenameDevice { name, reply }) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    let name = tokio::time::timeout(DEVICE_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "device rename request timed out".into(),
            )
        })?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device session ended".into(),
            )
        })?
        .map_err(|error| (StatusCode::BAD_GATEWAY, error))?;
    Ok(Json(RenameDeviceResponse { name }))
}

async fn reveal_developer_mode(
    State(state): State<AppState>,
) -> Result<Json<devicehub_runtime::DeveloperModePreparation>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::DeveloperMode(
        devicehub_runtime::DeveloperModeCommand::RevealOption { reply },
    )) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    let result = tokio::time::timeout(DEVICE_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "developer mode preparation request timed out".into(),
            )
        })?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device session ended".into(),
            )
        })?
        .map_err(|error| (StatusCode::BAD_GATEWAY, error))?;
    Ok(Json(result))
}

async fn developer_image_status(
    State(state): State<AppState>,
) -> Json<crate::developer_image::DeveloperImageMountStatus> {
    Json(state.developer_image.get())
}

async fn start_developer_image_mount(
    State(state): State<AppState>,
    Json(request): Json<crate::developer_image::DeveloperImageMountRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::DeveloperImageMount(
        crate::developer_image::DeveloperImageMountCommand::Start { request, reply },
    )) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    await_developer_image_command(response, "start developer image mount").await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn stop_developer_image_mount(
    State(state): State<AppState>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::DeveloperImageMount(
        crate::developer_image::DeveloperImageMountCommand::Stop { reply },
    )) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    await_developer_image_command(response, "stop developer image mount").await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn unmount_developer_image(
    State(state): State<AppState>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::DeveloperImageMount(
        crate::developer_image::DeveloperImageMountCommand::Unmount { reply },
    )) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    await_developer_image_command(response, "unmount developer image").await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn await_developer_image_command(
    response: oneshot::Receiver<Result<(), String>>,
    operation: &str,
) -> Result<(), (StatusCode, String)> {
    let result = tokio::time::timeout(Duration::from_secs(10), response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                format!("{operation} request timed out"),
            )
        })?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device session ended".into(),
            )
        })?;
    result.map_err(|error| {
        let status = if error.contains("already running") || error.contains("no developer image") {
            StatusCode::CONFLICT
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        };
        (status, error)
    })
}

async fn device_screenshot(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let png = state
        .application
        .device_control
        .capture_screenshot(SCREENSHOT_REQUEST_TIMEOUT)
        .await
        .map_err(|error| match error {
            devicehub_runtime::DeviceControlError::Unavailable
            | devicehub_runtime::DeviceControlError::SessionEnded => {
                (StatusCode::SERVICE_UNAVAILABLE, error.to_string())
            }
            devicehub_runtime::DeviceControlError::Timeout(_) => {
                (StatusCode::GATEWAY_TIMEOUT, error.to_string())
            }
            devicehub_runtime::DeviceControlError::Operation(_) => {
                (StatusCode::BAD_GATEWAY, error.to_string())
            }
        })?;
    Ok((
        [(CONTENT_TYPE, "image/png"), (CACHE_CONTROL, "no-store")],
        png,
    ))
}

#[derive(Clone, Copy)]
enum DevicePowerRequest {
    Lock,
    Restart,
    Shutdown,
}

async fn lock_device(State(state): State<AppState>) -> Result<StatusCode, (StatusCode, String)> {
    dispatch_device_power_command(&state, DevicePowerRequest::Lock).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn restart_device(State(state): State<AppState>) -> Result<StatusCode, (StatusCode, String)> {
    dispatch_device_power_command(&state, DevicePowerRequest::Restart).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn shutdown_device(
    State(state): State<AppState>,
) -> Result<StatusCode, (StatusCode, String)> {
    dispatch_device_power_command(&state, DevicePowerRequest::Shutdown).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn dispatch_device_power_command(
    state: &AppState,
    action: DevicePowerRequest,
) -> Result<(), (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    let command = match action {
        DevicePowerRequest::Lock => InputCmd::LockDevice(reply),
        DevicePowerRequest::Restart => InputCmd::RestartDevice(reply),
        DevicePowerRequest::Shutdown => InputCmd::ShutdownDevice(reply),
    };
    if !state.input.try_send(command) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    tokio::time::timeout(DEVICE_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "device power request timed out".into(),
            )
        })?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device session ended".into(),
            )
        })?
        .map_err(|error| {
            let status = if error == "another device power command is already running" {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_GATEWAY
            };
            (status, error)
        })
}

#[derive(Deserialize)]
struct StartWdaRunnerRequest {
    bundle_id: String,
}

async fn wda_runner_status(
    State(state): State<AppState>,
) -> Result<Json<devicehub_runtime::WdaRunnerStatus>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::WdaRunner(
        devicehub_runtime::WdaRunnerCommand::Status { reply },
    )) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    let status = tokio::time::timeout(DEVICE_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "WDA runner status timed out".into(),
            )
        })?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device session ended".into(),
            )
        })?;
    Ok(Json(status))
}

async fn start_wda_runner(
    State(state): State<AppState>,
    Json(request): Json<StartWdaRunnerRequest>,
) -> Result<Json<devicehub_runtime::WdaRunnerStatus>, (StatusCode, String)> {
    devicehub_runtime::validate_runner_bundle_id(&request.bundle_id)
        .map_err(|error| (StatusCode::BAD_REQUEST, error.into()))?;
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::WdaRunner(
        devicehub_runtime::WdaRunnerCommand::Start {
            bundle_id: request.bundle_id,
            reply,
        },
    )) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    let status = tokio::time::timeout(WDA_RUNNER_START_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "WDA runner startup timed out".into(),
            )
        })?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device session ended".into(),
            )
        })?
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
    State(state): State<AppState>,
) -> Result<Json<devicehub_runtime::WdaRunnerStatus>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::WdaRunner(
        devicehub_runtime::WdaRunnerCommand::Stop { reply },
    )) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    let status = tokio::time::timeout(DEVICE_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "WDA runner stop timed out".into(),
            )
        })?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device session ended".into(),
            )
        })?
        .map_err(|error| (StatusCode::BAD_GATEWAY, error))?;
    Ok(Json(status))
}

async fn device_companions(
    State(state): State<AppState>,
) -> Result<Json<Vec<devicehub_core::CompanionDevice>>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::ListCompanionDevices(reply)) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    let devices = tokio::time::timeout(DEVICE_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "companion device request timed out".into(),
            )
        })?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device session ended".into(),
            )
        })?
        .map_err(|error| (StatusCode::BAD_GATEWAY, error))?;
    Ok(Json(devices))
}

async fn device_home_screen(
    State(state): State<AppState>,
) -> Result<Json<devicehub_core::HomeScreenLayout>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::GetHomeScreenLayout(reply)) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    let layout = tokio::time::timeout(Duration::from_secs(12), response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "home screen layout request timed out".into(),
            )
        })?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device session ended".into(),
            )
        })?
        .map_err(|error| (StatusCode::BAD_GATEWAY, error))?;
    Ok(Json(layout))
}

async fn device_wallpaper(
    State(state): State<AppState>,
    Path(kind): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let kind = devicehub_core::WallpaperKind::parse(&kind).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "wallpaper kind must be home or lock".into(),
        )
    })?;
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::GetWallpaper { kind, reply }) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    let image = tokio::time::timeout(Duration::from_secs(12), response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "wallpaper preview request timed out".into(),
            )
        })?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device session ended".into(),
            )
        })?
        .map_err(|error| (StatusCode::BAD_GATEWAY, error))?;
    Ok((
        [(CONTENT_TYPE, "image/png"), (CACHE_CONTROL, "no-store")],
        image,
    ))
}

async fn device_provisioning_profiles(
    State(state): State<AppState>,
) -> Result<Json<Vec<devicehub_core::ProvisioningProfile>>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::Provisioning(
        crate::provisioning::ProvisioningCommand::List {
            expires_at: tokio::time::Instant::now() + Duration::from_secs(20),
            reply,
        },
    )) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    let profiles = await_provisioning_response(response, "provisioning profile request").await?;
    Ok(Json(profiles))
}

#[derive(Deserialize)]
struct InstallProvisioningProfileRequest {
    path: PathBuf,
}

async fn install_provisioning_profile(
    State(state): State<AppState>,
    Json(request): Json<InstallProvisioningProfileRequest>,
) -> Result<Json<devicehub_core::ProvisioningProfile>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::Provisioning(
        crate::provisioning::ProvisioningCommand::Install {
            source: request.path,
            expires_at: tokio::time::Instant::now() + Duration::from_secs(20),
            reply,
        },
    )) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    let profile =
        await_provisioning_response(response, "provisioning profile installation").await?;
    Ok(Json(profile))
}

async fn remove_provisioning_profile(
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    if uuid::Uuid::parse_str(&uuid).is_err() {
        return Err((
            StatusCode::BAD_REQUEST,
            "invalid provisioning profile UUID".into(),
        ));
    }
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::Provisioning(
        crate::provisioning::ProvisioningCommand::Remove {
            uuid,
            expires_at: tokio::time::Instant::now() + Duration::from_secs(20),
            reply,
        },
    )) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    await_provisioning_response(response, "provisioning profile removal").await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn trust_provisioning_profile_signer(
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let profile_uuid = uuid::Uuid::parse_str(&uuid)
        .map(|uuid| uuid.to_string())
        .map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                "invalid provisioning profile UUID".into(),
            )
        })?;
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::Provisioning(
        crate::provisioning::ProvisioningCommand::TrustSigner {
            uuid: profile_uuid,
            expires_at: tokio::time::Instant::now() + Duration::from_secs(20),
            reply,
        },
    )) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    await_provisioning_response(response, "app signer trust request").await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn await_provisioning_response<T>(
    response: oneshot::Receiver<Result<T, crate::provisioning::ProvisioningFailure>>,
    operation: &str,
) -> Result<T, (StatusCode, String)> {
    tokio::time::timeout(PROVISIONING_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                format!("{operation} timed out"),
            )
        })?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device session ended".into(),
            )
        })?
        .map_err(|error| {
            use crate::provisioning::ProvisioningFailure;
            let status = match error {
                ProvisioningFailure::Invalid(_) => StatusCode::BAD_REQUEST,
                ProvisioningFailure::NotFound(_) => StatusCode::NOT_FOUND,
                ProvisioningFailure::Conflict(_) => StatusCode::CONFLICT,
                ProvisioningFailure::Operation(_) => StatusCode::BAD_GATEWAY,
                ProvisioningFailure::Unavailable(_) => StatusCode::BAD_GATEWAY,
                ProvisioningFailure::Deadline(_) => StatusCode::GATEWAY_TIMEOUT,
                ProvisioningFailure::Timeout(_) => StatusCode::GATEWAY_TIMEOUT,
            };
            (status, error.to_string())
        })
}

async fn ws_upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    crate::websocket_transport::upgrade(
        ws,
        crate::websocket_transport::WebSocketState::new(
            state.application,
            state.browser_frames,
            state.clipboard,
            state.video_counters,
            state.input,
        ),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_crash_reports::{
        CrashReportSummaryQuery, DeleteCrashReportRequest, ExportCrashReportRequest,
        crash_report_summary, delete_crash_report, export_crash_report,
    };
    use devicehub_core::{Orientation, norm};
    use devicehub_runtime::DeviceInputCommand;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    fn test_state() -> (AppState, UnboundedReceiver<InputCmd>) {
        let (state, input_rx, _control_rx) = test_state_with_control();
        (state, input_rx)
    }

    fn handle_test_client_message(
        state: &AppState,
        text: &str,
        pressed_keyboard: &mut HashSet<u64>,
    ) -> ClientVideoFeedback {
        handle_client_message(
            &state.input,
            state.application.orientation.get(),
            &state.browser_frames,
            text,
            pressed_keyboard,
            &AtomicBool::new(true),
            &AtomicBool::new(false),
        )
    }

    fn test_state_with_control() -> (
        AppState,
        UnboundedReceiver<InputCmd>,
        UnboundedReceiver<ControlCmd>,
    ) {
        let input = InputSink::default();
        let (input_tx, input_rx) = unbounded_channel();
        input.set(Some(input_tx));
        let (control, control_rx) = unbounded_channel();
        let runtime_state = devicehub_runtime::CoreRuntimeState::<std::path::PathBuf> {
            commands: input.clone(),
            ..Default::default()
        };
        let browser_frames = runtime_state.browser_frames.clone();
        let network_capture = crate::network_capture::NetworkCaptureSlot::default();
        let bluetooth_capture = crate::bluetooth_capture::BluetoothCaptureSlot::default();
        let services = crate::supervisor::ServiceRegistry::default();
        let app_document_activity = crate::app_documents::AppDocumentActivitySlot::default();
        let device_file_activity = crate::device_files::DeviceFileActivitySlot::default();
        (
            AppState {
                application: runtime_state.client(control),
                performance_http: crate::http_performance::PerformanceHttpState::new(
                    runtime_state.performance.clone(),
                    runtime_state.performance_demand.clone(),
                    runtime_state.device_logs.clone(),
                    runtime_state.device_log_demand.clone(),
                    runtime_state.device_conditions.clone(),
                    network_capture,
                    bluetooth_capture,
                    services,
                    input.clone(),
                ),
                profiles_http: crate::http_profiles::ProfileHttpState::new(PathBuf::new()),
                storage_http: crate::http_storage::StorageHttpState::new(
                    input.clone(),
                    app_document_activity,
                    device_file_activity,
                ),
                diagnostics_http: crate::http_diagnostics::DiagnosticsHttpState::new(
                    input.clone(),
                    crate::device_backup::DeviceBackupSlot::default(),
                    crate::sysdiagnose::SysdiagnoseSlot::default(),
                    crate::log_archive::LogArchiveSlot::default(),
                ),
                apps_http: crate::http_apps::AppHttpState::new(
                    input.clone(),
                    devicehub_core::AppOperationSlot::default(),
                ),
                crash_reports_http: crate::http_crash_reports::CrashReportHttpState::new(
                    input.clone(),
                ),
                browser_frames,
                clipboard: ClipboardSlot::default(),
                developer_image: crate::developer_image::DeveloperImageMountSlot::default(),
                video_counters: VideoCounters::default(),
                input,
            },
            input_rx,
            control_rx,
        )
    }

    #[test]
    fn location_coordinates_accept_boundaries_and_reject_invalid_values() {
        assert!(validate_coordinates(-90.0, -180.0).is_ok());
        assert!(validate_coordinates(90.0, 180.0).is_ok());
        for (latitude, longitude) in [
            (90.000_001, 0.0),
            (0.0, 180.000_001),
            (f64::NAN, 0.0),
            (0.0, f64::INFINITY),
        ] {
            assert_eq!(
                validate_coordinates(latitude, longitude).unwrap_err().0,
                StatusCode::BAD_REQUEST
            );
        }
    }

    #[tokio::test]
    async fn set_location_endpoint_dispatches_to_the_device_session() {
        let (state, mut input_rx) = test_state();
        let request_state = state.clone();
        let request = tokio::spawn(async move {
            set_device_location(
                State(request_state),
                Json(SetLocationRequest {
                    latitude: 25.033,
                    longitude: 121.5654,
                }),
            )
            .await
        });

        let InputCmd::SetLocation {
            latitude,
            longitude,
            reply,
        } = input_rx.recv().await.unwrap()
        else {
            panic!("expected set location command");
        };
        assert_eq!((latitude, longitude), (25.033, 121.5654));
        reply.send(Ok(())).unwrap();
        assert_eq!(request.await.unwrap().unwrap(), StatusCode::OK);
    }

    #[tokio::test]
    async fn clear_location_endpoint_dispatches_to_the_device_session() {
        let (state, mut input_rx) = test_state();
        let request = tokio::spawn(clear_device_location(State(state)));
        let InputCmd::ClearLocation { reply } = input_rx.recv().await.unwrap() else {
            panic!("expected clear location command");
        };
        reply.send(Ok(())).unwrap();
        assert_eq!(request.await.unwrap().unwrap(), StatusCode::OK);
    }

    #[tokio::test]
    async fn provisioning_endpoints_dispatch_typed_commands() {
        let (state, mut input_rx) = test_state();
        let list_state = state.clone();
        let list = tokio::spawn(device_provisioning_profiles(State(list_state)));
        let InputCmd::Provisioning(crate::provisioning::ProvisioningCommand::List {
            reply, ..
        }) = input_rx.recv().await.unwrap()
        else {
            panic!("expected provisioning list command");
        };
        reply.send(Ok(Vec::new())).unwrap();
        assert!(list.await.unwrap().unwrap().0.is_empty());

        let install_state = state.clone();
        let install = tokio::spawn(install_provisioning_profile(
            State(install_state),
            Json(InstallProvisioningProfileRequest {
                path: PathBuf::from("/tmp/Game.mobileprovision"),
            }),
        ));
        let InputCmd::Provisioning(crate::provisioning::ProvisioningCommand::Install {
            source,
            reply,
            ..
        }) = input_rx.recv().await.unwrap()
        else {
            panic!("expected provisioning install command");
        };
        assert_eq!(source, PathBuf::from("/tmp/Game.mobileprovision"));
        let profile = devicehub_core::ProvisioningProfile {
            name: "Game Development".into(),
            uuid: "00000000-1111-2222-3333-444444444444".into(),
            team_identifiers: vec!["TEAM123".into()],
            application_identifier: Some("TEAM123.com.example.game".into()),
            creation_date: None,
            expiration_date: None,
            provisioned_devices: 1,
            is_expired: false,
            get_task_allow: true,
            removal_supported: true,
            parse_error: None,
        };
        reply.send(Ok(profile.clone())).unwrap();
        assert_eq!(install.await.unwrap().unwrap().0.uuid, profile.uuid);

        let remove = tokio::spawn(remove_provisioning_profile(
            State(state.clone()),
            Path("00000000-1111-2222-3333-444444444444".into()),
        ));
        let InputCmd::Provisioning(crate::provisioning::ProvisioningCommand::Remove {
            uuid,
            reply,
            ..
        }) = input_rx.recv().await.unwrap()
        else {
            panic!("expected provisioning remove command");
        };
        assert_eq!(uuid, "00000000-1111-2222-3333-444444444444");
        reply.send(Ok(())).unwrap();
        assert_eq!(remove.await.unwrap().unwrap(), StatusCode::NO_CONTENT);

        let trust = tokio::spawn(trust_provisioning_profile_signer(
            State(state),
            Path("00000000-AAAA-BBBB-CCCC-111111111111".into()),
        ));
        let InputCmd::Provisioning(crate::provisioning::ProvisioningCommand::TrustSigner {
            uuid,
            reply,
            ..
        }) = input_rx.recv().await.unwrap()
        else {
            panic!("expected app signer trust command");
        };
        assert_eq!(uuid, "00000000-aaaa-bbbb-cccc-111111111111");
        reply.send(Ok(())).unwrap();
        assert_eq!(trust.await.unwrap().unwrap(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn provisioning_remove_rejects_invalid_uuid_before_dispatch() {
        let (state, mut input_rx) = test_state();
        let error = remove_provisioning_profile(State(state.clone()), Path("not-a-uuid".into()))
            .await
            .unwrap_err();
        assert_eq!(error.0, StatusCode::BAD_REQUEST);
        assert!(input_rx.try_recv().is_err());

        let error =
            trust_provisioning_profile_signer(State(state), Path("still-not-a-uuid".into()))
                .await
                .unwrap_err();
        assert_eq!(error.0, StatusCode::BAD_REQUEST);
        assert!(input_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn signer_trust_requires_an_installed_development_profile() {
        let (state, mut input_rx) = test_state();
        let request = tokio::spawn(trust_provisioning_profile_signer(
            State(state),
            Path("00000000-aaaa-bbbb-cccc-111111111111".into()),
        ));
        let InputCmd::Provisioning(crate::provisioning::ProvisioningCommand::TrustSigner {
            reply,
            ..
        }) = input_rx.recv().await.unwrap()
        else {
            panic!("expected app signer trust command");
        };
        reply
            .send(Err(crate::provisioning::ProvisioningFailure::NotFound(
                "provisioning profile is not installed".into(),
            )))
            .unwrap();

        let error = request.await.unwrap().unwrap_err();
        assert_eq!(error.0, StatusCode::NOT_FOUND);
        assert!(input_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn provisioning_failures_map_to_actionable_http_statuses() {
        use crate::provisioning::ProvisioningFailure;

        let cases = [
            (
                ProvisioningFailure::Invalid("invalid".into()),
                StatusCode::BAD_REQUEST,
            ),
            (
                ProvisioningFailure::NotFound("missing".into()),
                StatusCode::NOT_FOUND,
            ),
            (
                ProvisioningFailure::Conflict("conflict".into()),
                StatusCode::CONFLICT,
            ),
            (
                ProvisioningFailure::Operation("failed".into()),
                StatusCode::BAD_GATEWAY,
            ),
            (
                ProvisioningFailure::Unavailable("closed".into()),
                StatusCode::BAD_GATEWAY,
            ),
            (
                ProvisioningFailure::Deadline("expired".into()),
                StatusCode::GATEWAY_TIMEOUT,
            ),
            (
                ProvisioningFailure::Timeout("slow".into()),
                StatusCode::GATEWAY_TIMEOUT,
            ),
        ];
        for (failure, expected) in cases {
            let (reply, response) = oneshot::channel();
            reply.send(Err(failure)).unwrap();
            let error = await_provisioning_response::<()>(response, "test")
                .await
                .unwrap_err();
            assert_eq!(error.0, expected);
        }
    }

    #[tokio::test]
    async fn reconnect_endpoint_forces_a_new_session_for_the_selected_device() {
        let (state, _input_rx, mut control_rx) = test_state_with_control();

        assert_eq!(
            reconnect_device(State(state), Path("device-1".into())).await,
            StatusCode::ACCEPTED
        );
        assert!(matches!(
            control_rx.recv().await,
            Some(ControlCmd::Reconnect(udid)) if udid == "device-1"
        ));
    }

    #[tokio::test]
    async fn pairing_endpoint_dispatches_to_the_device_manager() {
        let (state, _input_rx, mut control_rx) = test_state_with_control();
        let request = tokio::spawn(pair_device(State(state), Path("device-1::usb".into())));

        let Some(ControlCmd::Pair {
            selection_id,
            reply,
        }) = control_rx.recv().await
        else {
            panic!("expected pairing command");
        };
        assert_eq!(selection_id, "device-1::usb");
        reply
            .send(PairDeviceResult {
                outcome: devicehub_core::PairDeviceOutcome::Paired,
                error: None,
            })
            .unwrap();

        let response = request.await.unwrap().unwrap().0;
        assert_eq!(response.outcome, devicehub_core::PairDeviceOutcome::Paired);
        assert!(response.error.is_none());
    }

    #[tokio::test]
    async fn forget_endpoint_dispatches_to_the_device_manager() {
        let (state, _input_rx, mut control_rx) = test_state_with_control();
        let request = tokio::spawn(forget_device(State(state), Path("device-1::usb".into())));

        let Some(ControlCmd::Forget {
            selection_id,
            reply,
        }) = control_rx.recv().await
        else {
            panic!("expected forget command");
        };
        assert_eq!(selection_id, "device-1::usb");
        reply
            .send(ForgetDeviceResult {
                outcome: devicehub_core::ForgetDeviceOutcome::Forgotten,
                error: None,
            })
            .unwrap();

        let response = request.await.unwrap().unwrap().0;
        assert_eq!(
            response.outcome,
            devicehub_core::ForgetDeviceOutcome::Forgotten
        );
        assert!(response.error.is_none());
    }

    #[test]
    fn browser_feedback_messages_keep_acceptance_and_presentation_distinct() {
        let (state, _input_rx) = test_state();
        let mut pressed = HashSet::new();
        assert_eq!(
            handle_test_client_message(
                &state,
                r#"{"type":"browser_frame_accepted","sequence":"42"}"#,
                &mut pressed,
            ),
            ClientVideoFeedback::BrowserAccepted(42)
        );
        assert_eq!(
            handle_test_client_message(
                &state,
                r#"{"type":"frame_presented","sequence":"42"}"#,
                &mut pressed,
            ),
            ClientVideoFeedback::FramePresented(42)
        );
    }

    #[tokio::test]
    async fn video_demand_resumes_with_a_keyframe_request() {
        let (state, _input_rx) = test_state();
        let active = AtomicBool::new(true);
        let resync = AtomicBool::new(false);
        let keyframes = state.browser_frames.clone();
        let mut pressed = HashSet::new();

        assert_eq!(
            handle_client_message(
                &state.input,
                state.application.orientation.get(),
                &state.browser_frames,
                r#"{"type":"video_demand","active":false}"#,
                &mut pressed,
                &active,
                &resync,
            ),
            ClientVideoFeedback::ResetAll
        );
        assert!(!active.load(Ordering::Relaxed));
        assert_eq!(
            handle_client_message(
                &state.input,
                state.application.orientation.get(),
                &state.browser_frames,
                r#"{"type":"video_demand","active":true}"#,
                &mut pressed,
                &active,
                &resync,
            ),
            ClientVideoFeedback::None
        );
        assert!(active.load(Ordering::Relaxed));
        assert!(resync.load(Ordering::Relaxed));
        tokio::time::timeout(Duration::from_millis(10), keyframes.keyframe_requested())
            .await
            .expect("video demand resume should request a keyframe");
    }

    #[tokio::test]
    async fn browser_decoder_keyframe_request_enters_resync() {
        let (state, _input_rx) = test_state();
        let active = AtomicBool::new(true);
        let resync = AtomicBool::new(false);
        let keyframes = state.browser_frames.clone();

        assert_eq!(
            handle_client_message(
                &state.input,
                state.application.orientation.get(),
                &state.browser_frames,
                r#"{"type":"browser_video_keyframe"}"#,
                &mut HashSet::new(),
                &active,
                &resync,
            ),
            ClientVideoFeedback::ResetBrowser
        );
        assert!(resync.load(Ordering::Acquire));
        tokio::time::timeout(Duration::from_millis(10), keyframes.keyframe_requested())
            .await
            .expect("browser decoder recovery should request a keyframe");
    }

    #[test]
    fn frontend_metrics_reject_impossible_or_unbounded_values() {
        assert!(valid_frontend_metrics(
            5_000.0, 300, 0, 299, 600.0, 100.0, 2, 1
        ));
        assert!(!valid_frontend_metrics(
            5_000.0, 300, 301, 299, 600.0, 100.0, 2, 1,
        ));
        assert!(!valid_frontend_metrics(f64::NAN, 0, 0, 0, 0.0, 0.0, 0, 0,));
    }

    #[test]
    fn contact_validation_rejects_duplicate_ids() {
        let contacts = vec![
            WebContact {
                identity: 1,
                touching: true,
                x: 0.2,
                y: 0.3,
            },
            WebContact {
                identity: 1,
                touching: true,
                x: 0.4,
                y: 0.5,
            },
        ];
        assert!(validate_contacts(contacts, Orientation::Portrait).is_none());
    }

    #[test]
    fn contact_validation_unrotates_landscape() {
        let contacts = vec![WebContact {
            identity: 2,
            touching: true,
            x: 0.25,
            y: 0.75,
        }];
        let result = validate_contacts(contacts, Orientation::LandscapeRight).unwrap();
        assert_eq!(result[0].x, norm(0.75));
        assert_eq!(result[0].y, norm(0.75));
    }

    #[test]
    fn keyboard_messages_validate_and_track_pressed_usages() {
        let (state, mut input_rx) = test_state();
        let mut pressed = HashSet::new();

        handle_test_client_message(
            &state,
            r#"{"type":"keyboard_down","usage":4}"#,
            &mut pressed,
        );
        handle_test_client_message(
            &state,
            r#"{"type":"keyboard_down","usage":4}"#,
            &mut pressed,
        );
        handle_test_client_message(
            &state,
            r#"{"type":"keyboard_down","usage":65535}"#,
            &mut pressed,
        );

        assert!(matches!(
            input_rx.try_recv(),
            Ok(InputCmd::DeviceInput(DeviceInputCommand::KeyboardDown(4)))
        ));
        assert!(input_rx.try_recv().is_err());
        assert_eq!(pressed, HashSet::from([4]));

        handle_test_client_message(&state, r#"{"type":"keyboard_up","usage":4}"#, &mut pressed);
        assert!(matches!(
            input_rx.try_recv(),
            Ok(InputCmd::DeviceInput(DeviceInputCommand::KeyboardUp(4)))
        ));
        assert!(pressed.is_empty());
    }

    #[test]
    fn text_messages_are_bounded_before_dispatch() {
        let (state, mut input_rx) = test_state();
        let mut pressed = HashSet::new();

        handle_test_client_message(
            &state,
            r#"{"type":"text","text":"Hello, iPhone!"}"#,
            &mut pressed,
        );
        handle_test_client_message(&state, r#"{"type":"text","text":""}"#, &mut pressed);
        let oversized =
            serde_json::to_string(&json!({ "type": "text", "text": "x".repeat(129) })).unwrap();
        handle_test_client_message(&state, &oversized, &mut pressed);

        assert!(matches!(
            input_rx.try_recv(),
            Ok(InputCmd::DeviceInput(DeviceInputCommand::Text(text)))
                if text == "Hello, iPhone!"
        ));
        assert!(input_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn paste_text_endpoint_dispatches_unicode_and_waits_for_completion() {
        let (state, mut input_rx) = test_state();
        let request = tokio::spawn(paste_device_text(
            State(state.clone()),
            Json(PasteDeviceTextRequest {
                text: "你好, iPhone".into(),
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::PasteText { text, reply } => {
                assert_eq!(text, "你好, iPhone");
                reply.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(request.await.unwrap().unwrap(), StatusCode::NO_CONTENT);

        assert!(matches!(
            paste_device_text(
                State(state),
                Json(PasteDeviceTextRequest {
                    text: "bad\0text".into(),
                }),
            )
            .await,
            Err((StatusCode::BAD_REQUEST, _))
        ));
    }

    #[test]
    fn websocket_cleanup_releases_pressed_keyboard_usages() {
        let (state, mut input_rx) = test_state();
        send_all_up(&state.input, &HashSet::from([0x04, 0xe1]));

        let commands: Vec<_> = std::iter::from_fn(|| input_rx.try_recv().ok()).collect();
        assert!(commands.iter().any(|command| matches!(
            command,
            InputCmd::DeviceInput(DeviceInputCommand::KeyboardUp(0x04))
        )));
        assert!(commands.iter().any(|command| matches!(
            command,
            InputCmd::DeviceInput(DeviceInputCommand::KeyboardUp(0xe1))
        )));
    }

    #[test]
    fn keyboard_usage_validation_matches_frontend_ranges() {
        for usage in [0x04, 0x65, 0x67, 0x73, 0x85, 0x87, 0x89, 0xe0, 0xe7] {
            assert!(valid_keyboard_usage(usage));
        }
        for usage in [0x00, 0x03, 0x74, 0x84, 0x86, 0x88, 0x8a, 0xdf, 0xe8] {
            assert!(!valid_keyboard_usage(usage));
        }
    }

    #[test]
    fn private_api_requires_exact_bearer_or_websocket_token() {
        let mut headers = HeaderMap::new();
        assert!(!private_api_authorized(&headers, "secret"));

        headers.insert(AUTHORIZATION, "Bearer wrong".parse().unwrap());
        assert!(!private_api_authorized(&headers, "secret"));
        headers.insert(AUTHORIZATION, "Bearer secret".parse().unwrap());
        assert!(private_api_authorized(&headers, "secret"));

        headers.remove(AUTHORIZATION);
        headers.insert(
            SEC_WEBSOCKET_PROTOCOL,
            "devicehub-mask, secret".parse().unwrap(),
        );
        assert!(private_api_authorized(&headers, "secret"));
    }

    #[tokio::test]
    async fn companion_endpoint_dispatches_a_read_only_device_query() {
        let (state, mut input_rx) = test_state();
        let request = tokio::spawn(device_companions(State(state)));
        let InputCmd::ListCompanionDevices(reply) = input_rx.recv().await.unwrap() else {
            panic!("expected companion device query");
        };
        reply
            .send(Ok(vec![devicehub_core::CompanionDevice {
                identifier: "watch-id".into(),
                name: Some("Test Watch".into()),
                product_type: Some("Watch7,5".into()),
                product_version: Some("27.0".into()),
                build_version: Some("24A123".into()),
            }]))
            .unwrap();
        let response = request.await.unwrap().unwrap();
        assert_eq!(response.0.len(), 1);
        assert_eq!(response.0[0].name.as_deref(), Some("Test Watch"));
    }

    #[tokio::test]
    async fn home_screen_endpoint_dispatches_a_normalized_read_only_query() {
        let (state, mut input_rx) = test_state();
        let request = tokio::spawn(device_home_screen(State(state)));
        let InputCmd::GetHomeScreenLayout(reply) = input_rx.recv().await.unwrap() else {
            panic!("expected home screen layout query");
        };
        reply
            .send(Ok(devicehub_core::HomeScreenLayout {
                apps: vec![devicehub_core::HomeScreenAppLocation {
                    bundle_id: "com.example.game".into(),
                    name: Some("Game".into()),
                    container: devicehub_core::HomeScreenContainer::Page,
                    page: Some(2),
                    position: 3,
                    folders: Vec::new(),
                }],
                page_count: 2,
                metrics: Some(devicehub_core::HomeScreenIconMetrics {
                    screen_width: Some(810),
                    screen_height: Some(1080),
                    icon_width: Some(68),
                    icon_height: Some(68),
                    columns: Some(5),
                    rows: Some(6),
                    dock_max_count: Some(20),
                    folder_columns: Some(4),
                    folder_rows: Some(4),
                    max_pages: Some(15),
                    folder_max_pages: Some(15),
                }),
                truncated: false,
            }))
            .unwrap();
        let response = request.await.unwrap().unwrap();
        assert_eq!(response.0.apps[0].bundle_id, "com.example.game");
        assert_eq!(response.0.metrics.unwrap().columns, Some(5));
        assert_eq!(response.0.apps[0].page, Some(2));
    }

    #[tokio::test]
    async fn wallpaper_endpoint_accepts_only_bounded_read_only_kinds() {
        let (state, mut input_rx) = test_state();
        let invalid = match device_wallpaper(State(state.clone()), Path("desktop".into())).await {
            Err(error) => error,
            Ok(_) => panic!("invalid wallpaper kind should be rejected"),
        };
        assert_eq!(invalid.0, StatusCode::BAD_REQUEST);
        assert!(input_rx.try_recv().is_err());

        let request = tokio::spawn(device_wallpaper(State(state), Path("lock".into())));
        match input_rx.recv().await.unwrap() {
            InputCmd::GetWallpaper { kind, reply } => {
                assert_eq!(kind, devicehub_core::WallpaperKind::Lock);
                reply.send(Ok(vec![1, 2, 3])).unwrap();
            }
            _ => panic!("expected lock-screen wallpaper query"),
        }
        let response = request.await.unwrap().unwrap().into_response();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers().get(CONTENT_TYPE).unwrap(), "image/png");
        assert_eq!(response.headers().get(CACHE_CONTROL).unwrap(), "no-store");
    }

    #[tokio::test]
    async fn wda_runner_endpoints_validate_and_dispatch_lifecycle_commands() {
        let (state, mut input_rx) = test_state();
        let running = devicehub_runtime::WdaRunnerStatus {
            phase: devicehub_runtime::WdaRunnerPhase::Running,
            managed: true,
            runner_bundle_id: Some("com.example.WDARunner.xctrunner".into()),
            last_error: None,
        };

        let status_request = tokio::spawn(wda_runner_status(State(state.clone())));
        let InputCmd::WdaRunner(devicehub_runtime::WdaRunnerCommand::Status { reply }) =
            input_rx.recv().await.unwrap()
        else {
            panic!("expected WDA runner status command");
        };
        reply.send(running.clone()).unwrap();
        assert_eq!(status_request.await.unwrap().unwrap().0, running);

        let start_request = tokio::spawn(start_wda_runner(
            State(state.clone()),
            Json(StartWdaRunnerRequest {
                bundle_id: "com.example.WDARunner.xctrunner".into(),
            }),
        ));
        let InputCmd::WdaRunner(devicehub_runtime::WdaRunnerCommand::Start { bundle_id, reply }) =
            input_rx.recv().await.unwrap()
        else {
            panic!("expected WDA runner start command");
        };
        assert_eq!(bundle_id, "com.example.WDARunner.xctrunner");
        reply.send(Ok(running.clone())).unwrap();
        assert_eq!(start_request.await.unwrap().unwrap().0, running);

        let stop_request = tokio::spawn(stop_wda_runner(State(state.clone())));
        let InputCmd::WdaRunner(devicehub_runtime::WdaRunnerCommand::Stop { reply }) =
            input_rx.recv().await.unwrap()
        else {
            panic!("expected WDA runner stop command");
        };
        reply
            .send(Ok(devicehub_runtime::WdaRunnerStatus::default()))
            .unwrap();
        assert_eq!(
            stop_request.await.unwrap().unwrap().0,
            devicehub_runtime::WdaRunnerStatus::default()
        );

        assert!(matches!(
            start_wda_runner(
                State(state),
                Json(StartWdaRunnerRequest {
                    bundle_id: "com.example.not-a-runner".into(),
                }),
            )
            .await,
            Err((StatusCode::BAD_REQUEST, _))
        ));
        assert!(input_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn device_queries_require_an_active_session() {
        let (state, _input_rx) = test_state();
        state.input.set(None);

        assert!(matches!(
            device_details(State(state.clone())).await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
        assert!(matches!(
            rename_device(
                State(state.clone()),
                Json(RenameDeviceRequest {
                    name: "Test iPhone".into(),
                }),
            )
            .await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
        assert!(matches!(
            device_screenshot(State(state.clone())).await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
        assert!(matches!(
            device_companions(State(state.clone())).await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
        assert!(matches!(
            device_home_screen(State(state.clone())).await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
        assert!(matches!(
            lock_device(State(state.clone())).await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
        assert!(matches!(
            restart_device(State(state.clone())).await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
        assert!(matches!(
            shutdown_device(State(state.clone())).await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
        assert!(matches!(
            device_provisioning_profiles(State(state.clone())).await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
        assert!(matches!(
            trust_provisioning_profile_signer(
                State(state.clone()),
                Path("00000000-aaaa-bbbb-cccc-111111111111".into()),
            )
            .await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
        assert!(matches!(
            crate::http_crash_reports::device_crash_reports(State(
                state.crash_reports_http.clone(),
            ))
            .await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
    }

    #[tokio::test]
    async fn device_rename_validates_and_dispatches_a_normalized_name() {
        let (state, mut input_rx) = test_state();
        let request_state = state.clone();
        let request = tokio::spawn(async move {
            rename_device(
                State(request_state),
                Json(RenameDeviceRequest {
                    name: "  测试 iPhone  ".into(),
                }),
            )
            .await
        });
        match input_rx.recv().await.unwrap() {
            InputCmd::RenameDevice { name, reply } => {
                assert_eq!(name, "测试 iPhone");
                reply.send(Ok(name)).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(request.await.unwrap().unwrap().0.name, "测试 iPhone");

        assert!(matches!(
            rename_device(
                State(state),
                Json(RenameDeviceRequest {
                    name: "bad\nname".into(),
                }),
            )
            .await,
            Err((StatusCode::BAD_REQUEST, _))
        ));
        assert!(input_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn developer_mode_reveal_dispatches_a_typed_amfi_command() {
        let (state, mut input_rx) = test_state();
        let request = tokio::spawn(reveal_developer_mode(State(state)));
        let InputCmd::DeveloperMode(devicehub_runtime::DeveloperModeCommand::RevealOption {
            reply,
        }) = input_rx.recv().await.unwrap()
        else {
            panic!("unexpected command");
        };
        reply
            .send(Ok(devicehub_runtime::DeveloperModePreparation {
                already_enabled: false,
            }))
            .unwrap();
        let response = request.await.unwrap().unwrap().0;
        assert!(!response.already_enabled);
    }

    #[tokio::test]
    async fn developer_image_endpoints_dispatch_mount_lifecycle() {
        use crate::developer_image::{
            DeveloperImageMountCommand, DeveloperImageMountRequest, DeveloperImageMountState,
        };

        let (state, mut input_rx) = test_state();
        assert_eq!(
            developer_image_status(State(state.clone())).await.0.state,
            DeveloperImageMountState::Idle
        );
        let mount_request = DeveloperImageMountRequest {
            image: PathBuf::from("/DeveloperDiskImage.dmg"),
            signature: None,
            trust_cache: Some(PathBuf::from("/DeveloperDiskImage.dmg.trustcache")),
            manifest: Some(PathBuf::from("/BuildManifest.plist")),
        };
        let start = tokio::spawn(start_developer_image_mount(
            State(state.clone()),
            Json(mount_request),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::DeveloperImageMount(DeveloperImageMountCommand::Start { request, reply }) => {
                assert_eq!(request.image, PathBuf::from("/DeveloperDiskImage.dmg"));
                assert!(request.signature.is_none());
                assert_eq!(
                    request.trust_cache,
                    Some(PathBuf::from("/DeveloperDiskImage.dmg.trustcache"))
                );
                reply.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(start.await.unwrap().unwrap(), StatusCode::NO_CONTENT);

        let stop = tokio::spawn(stop_developer_image_mount(State(state.clone())));
        match input_rx.recv().await.unwrap() {
            InputCmd::DeveloperImageMount(DeveloperImageMountCommand::Stop { reply }) => {
                reply.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(stop.await.unwrap().unwrap(), StatusCode::NO_CONTENT);

        let unmount = tokio::spawn(unmount_developer_image(State(state)));
        match input_rx.recv().await.unwrap() {
            InputCmd::DeveloperImageMount(DeveloperImageMountCommand::Unmount { reply }) => {
                reply.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(unmount.await.unwrap().unwrap(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn native_screenshot_endpoint_dispatches_and_disables_caching() {
        let (state, mut input_rx) = test_state();
        let request = tokio::spawn(device_screenshot(State(state)));
        match input_rx.recv().await.unwrap() {
            InputCmd::TakeScreenshot(reply) => {
                reply.send(Ok(vec![1, 2, 3])).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        let response = request.await.unwrap().unwrap().into_response();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers().get(CONTENT_TYPE).unwrap(), "image/png");
        assert_eq!(response.headers().get(CACHE_CONTROL).unwrap(), "no-store");
    }

    #[tokio::test]
    async fn device_power_endpoints_dispatch_only_fixed_commands() {
        let (state, mut input_rx) = test_state();
        let lock = tokio::spawn(lock_device(State(state.clone())));
        match input_rx.recv().await.unwrap() {
            InputCmd::LockDevice(reply) => reply.send(Ok(())).unwrap(),
            _ => panic!("unexpected command"),
        }
        assert_eq!(lock.await.unwrap().unwrap(), StatusCode::NO_CONTENT);

        let restart = tokio::spawn(restart_device(State(state.clone())));
        match input_rx.recv().await.unwrap() {
            InputCmd::RestartDevice(reply) => reply.send(Ok(())).unwrap(),
            _ => panic!("unexpected command"),
        }
        assert_eq!(restart.await.unwrap().unwrap(), StatusCode::NO_CONTENT);

        let shutdown = tokio::spawn(shutdown_device(State(state)));
        match input_rx.recv().await.unwrap() {
            InputCmd::ShutdownDevice(reply) => reply.send(Ok(())).unwrap(),
            _ => panic!("unexpected command"),
        }
        assert_eq!(shutdown.await.unwrap().unwrap(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn device_power_endpoint_reports_a_concurrent_command_as_conflict() {
        let (state, mut input_rx) = test_state();
        let request = tokio::spawn(restart_device(State(state)));
        match input_rx.recv().await.unwrap() {
            InputCmd::RestartDevice(reply) => reply
                .send(Err("another device power command is already running".into()))
                .unwrap(),
            _ => panic!("unexpected command"),
        }
        assert!(matches!(
            request.await.unwrap(),
            Err((StatusCode::CONFLICT, _))
        ));
    }

    #[tokio::test]
    async fn crash_report_list_export_and_delete_use_the_device_session() {
        let (state, mut input_rx) = test_state();
        let list_request = tokio::spawn(crate::http_crash_reports::device_crash_reports(State(
            state.crash_reports_http.clone(),
        )));
        match input_rx.recv().await.unwrap() {
            InputCmd::ListCrashReports(reply) => {
                reply
                    .send(Ok(devicehub_core::DeviceCrashReportList {
                        reports: vec![devicehub_core::DeviceCrashReport {
                            path: "/Report.ips".into(),
                            name: "Report.ips".into(),
                            size_bytes: 42,
                            modified: "2026-07-24T00:00:00Z".into(),
                        }],
                        truncated: false,
                    }))
                    .unwrap();
            }
            _ => panic!("unexpected command"),
        }
        let Json(list) = list_request.await.unwrap().unwrap();
        assert_eq!(list.reports.len(), 1);
        assert!(!list.truncated);

        let export_request = tokio::spawn(export_crash_report(
            State(state.crash_reports_http.clone()),
            Json(ExportCrashReportRequest {
                device_path: "/Report.ips".into(),
                destination: PathBuf::from("/tmp/Report.ips"),
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::ExportCrashReport {
                device_path,
                destination,
                reply,
            } => {
                assert_eq!(device_path, "/Report.ips");
                assert_eq!(destination, PathBuf::from("/tmp/Report.ips"));
                reply.send(Ok(42)).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        let Json(result) = export_request.await.unwrap().unwrap();
        assert_eq!(result, serde_json::json!({ "bytes_written": 42 }));

        let delete_request = tokio::spawn(delete_crash_report(
            State(state.crash_reports_http),
            Json(DeleteCrashReportRequest {
                device_path: "/Report.ips".into(),
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::DeleteCrashReport { device_path, reply } => {
                assert_eq!(device_path, "/Report.ips");
                reply.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        let Json(result) = delete_request.await.unwrap().unwrap();
        assert_eq!(result, serde_json::json!({ "deleted": true }));
    }

    #[tokio::test]
    async fn crash_report_summary_is_validated_bounded_and_summary_only() {
        let (state, mut input_rx) = test_state();
        let invalid = crash_report_summary(
            State(state.crash_reports_http.clone()),
            Query(CrashReportSummaryQuery {
                device_path: "/../private/report.ips".into(),
            }),
        )
        .await;
        assert!(matches!(invalid, Err((StatusCode::BAD_REQUEST, _))));
        assert!(input_rx.try_recv().is_err());

        let request = tokio::spawn(crash_report_summary(
            State(state.crash_reports_http),
            Query(CrashReportSummaryQuery {
                device_path: "/Report.ips".into(),
            }),
        ));
        let InputCmd::ReadCrashReport {
            device_path,
            max_bytes,
            reply,
        } = input_rx.recv().await.unwrap()
        else {
            panic!("expected crash report read command");
        };
        assert_eq!(device_path, "/Report.ips");
        assert_eq!(max_bytes, devicehub_runtime::MAX_CRASH_REPORT_READ_BYTES);
        reply
            .send(Ok(devicehub_core::DeviceCrashReportContent {
                device_path,
                size_bytes: 4_096,
                bytes_read: 128,
                truncated: false,
                lossy_utf8: false,
                summary: devicehub_core::DeviceCrashReportSummary {
                    format: devicehub_core::CrashReportFormat::IpsJson,
                    kind: devicehub_core::CrashReportKind::AppCrash,
                    process_name: Some("Game".into()),
                    bundle_id: Some("com.example.game".into()),
                    app_version: None,
                    build_version: None,
                    os_version: None,
                    timestamp: None,
                    bug_type: Some("309".into()),
                    exception_type: None,
                    exception_signal: None,
                    termination_namespace: None,
                    termination_code: None,
                    faulting_thread: None,
                    details_parsed: true,
                    source_truncated: false,
                },
                content: "PRIVATE RAW CRASH CONTENT".into(),
            }))
            .unwrap();
        let Json(summary) = request.await.unwrap().unwrap();
        let serialized = serde_json::to_string(&summary).unwrap();
        assert!(serialized.contains("com.example.game"));
        assert!(!serialized.contains("PRIVATE RAW CRASH CONTENT"));
    }
}
