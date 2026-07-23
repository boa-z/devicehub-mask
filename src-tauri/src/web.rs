use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, Query, Request, State, WebSocketUpgrade};
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, SEC_WEBSOCKET_PROTOCOL};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, put};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use tower_http::cors::CorsLayer;

use crate::hid::TouchContact;
use crate::protocol::{
    ActiveSlot, AppOperationSlot, AudioSlot, ClipboardSlot, ControlCmd, DeviceListSlot, ErrorSlot,
    Frame, FrameFormat, FrameSlot, InputCmd, InputSink, LocationStatus, LocationStatusSlot,
    Orientation, OrientationSlot, RotateDir, StatusSlot, VideoCounters, encode_audio_envelope,
    norm, unrotate_norm, validate_paste_text,
};
use crate::{
    performance::{PerformanceDemand, PerformanceSlot},
    supervisor::ServiceRegistry,
};

#[derive(Clone)]
pub struct AppState {
    pub frames: FrameSlot,
    pub audio: AudioSlot,
    pub clipboard: ClipboardSlot,
    pub device_events: crate::device_events::DeviceEventSlot,
    pub video_counters: VideoCounters,
    pub status: StatusSlot,
    pub orientation: OrientationSlot,
    pub devices: DeviceListSlot,
    pub active: ActiveSlot,
    pub error: ErrorSlot,
    pub app_operation: AppOperationSlot,
    pub location: LocationStatusSlot,
    pub performance: PerformanceSlot,
    pub performance_demand: PerformanceDemand,
    pub device_logs: crate::device_logs::DeviceLogSlot,
    pub device_log_demand: crate::device_logs::DeviceLogDemand,
    pub services: ServiceRegistry,
    pub input: InputSink,
    pub control: UnboundedSender<ControlCmd>,
    pub profile_dir: Arc<PathBuf>,
}

#[derive(Serialize)]
struct DeviceView {
    udid: String,
    name: String,
    connection: &'static str,
}

#[derive(Serialize)]
struct StatusView {
    status: String,
    active_udid: Option<String>,
    error: Option<String>,
    orientation: &'static str,
    devices: Vec<DeviceView>,
    location: LocationStatus,
}

#[derive(Serialize)]
struct StreamMetricsView {
    source_fps: f64,
    decoded_fps: f64,
    published_fps: f64,
    sent_fps: f64,
    backend_dropped_fps: f64,
    jpeg_encode_ms: f64,
    frame_age_ms: f64,
    websocket_send_ms: f64,
    presentation_ack_ms: f64,
    megabits_per_second: f64,
}

#[derive(Serialize)]
struct PerformanceView {
    sample: crate::performance::PerformanceSnapshot,
    services: Vec<crate::supervisor::ServiceHealth>,
    sampling: bool,
}

#[derive(Serialize, Deserialize)]
struct Profile {
    version: u8,
    name: String,
    mappings: Vec<serde_json::Value>,
    #[serde(default, rename = "bundleIdentifiers")]
    bundle_identifiers: Vec<String>,
    #[serde(default = "default_hardware_bindings", rename = "hardwareBindings")]
    hardware_bindings: BTreeMap<String, String>,
}

const HARDWARE_BUTTON_NAMES: [&str; 7] = [
    "home",
    "lock",
    "volume-up",
    "volume-down",
    "mute",
    "siri",
    "action",
];

fn default_hardware_bindings() -> BTreeMap<String, String> {
    HARDWARE_BUTTON_NAMES
        .into_iter()
        .map(|name| (name.to_string(), String::new()))
        .collect()
}

#[derive(Serialize)]
struct ProfileList {
    profiles: Vec<String>,
    active: String,
    app_bindings: BTreeMap<String, String>,
    binding_conflicts: Vec<String>,
}

#[derive(Clone)]
struct ApiToken(Arc<str>);

pub fn router(state: AppState, token: String) -> Router {
    Router::new()
        .route("/api/status", get(status))
        .route("/api/performance", get(performance))
        .route(
            "/api/performance/sampling",
            put(start_performance_sampling).delete(stop_performance_sampling),
        )
        .route(
            "/api/device/logs",
            get(device_logs).delete(clear_device_logs),
        )
        .route(
            "/api/device/logs/streaming",
            put(start_device_logs).delete(stop_device_logs),
        )
        .route("/api/devices/refresh", put(refresh_devices))
        .route("/api/devices/{udid}/connect", put(connect_device))
        .route("/api/devices/{udid}/reconnect", put(reconnect_device))
        .route("/api/device/details", get(device_details))
        .route("/api/device/screenshot", get(device_screenshot))
        .route("/api/device/text/paste", put(paste_device_text))
        .route("/api/device/restart", put(restart_device))
        .route("/api/device/shutdown", put(shutdown_device))
        .route(
            "/api/device/location",
            get(device_location)
                .put(set_device_location)
                .delete(clear_device_location),
        )
        .route("/api/device/apps", get(device_apps))
        .route("/api/device/apps/{bundle_id}/icon", get(device_app_icon))
        .route(
            "/api/device/apps/{bundle_id}/documents",
            get(app_documents).delete(delete_app_document),
        )
        .route(
            "/api/device/apps/{bundle_id}/documents/export",
            put(export_app_document),
        )
        .route(
            "/api/device/apps/{bundle_id}/documents/import",
            put(import_app_document),
        )
        .route(
            "/api/device/apps/{bundle_id}/documents/directory",
            put(create_app_document_directory),
        )
        .route(
            "/api/device/apps/{bundle_id}/documents/rename",
            put(rename_app_document),
        )
        .route("/api/device/apps/operation", get(app_operation))
        .route("/api/device/apps/install", put(install_app))
        .route("/api/device/apps/{bundle_id}", delete(uninstall_app))
        .route("/api/device/apps/{bundle_id}/launch", put(launch_app))
        .route("/api/device/apps/{bundle_id}/stop", put(stop_app))
        .route("/api/device/crash-reports", get(device_crash_reports))
        .route("/api/device/crash-reports/export", put(export_crash_report))
        .route(
            "/api/device/provisioning-profiles",
            get(device_provisioning_profiles),
        )
        .route("/api/profiles", get(list_profiles))
        .route("/api/profiles/{name}", get(load_profile).put(save_profile))
        .route("/api/profiles/{name}/activate", put(activate_profile))
        .route("/api/profiles/{name}/delete", put(delete_profile))
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

async fn status(State(state): State<AppState>) -> Json<StatusView> {
    Json(status_snapshot(&state))
}

async fn performance(State(state): State<AppState>) -> Json<PerformanceView> {
    Json(PerformanceView {
        sample: state.performance.get(),
        services: state.services.snapshot(),
        sampling: state.performance_demand.enabled(),
    })
}

async fn start_performance_sampling(State(state): State<AppState>) -> StatusCode {
    state.performance.reset();
    state.performance_demand.set(true);
    StatusCode::NO_CONTENT
}

async fn stop_performance_sampling(State(state): State<AppState>) -> StatusCode {
    state.performance_demand.set(false);
    state.performance.reset();
    StatusCode::NO_CONTENT
}

#[derive(Deserialize)]
struct DeviceLogQuery {
    after: Option<u64>,
    limit: Option<usize>,
}

async fn device_logs(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<DeviceLogQuery>,
) -> Json<DeviceLogsView> {
    let service = state
        .services
        .snapshot()
        .into_iter()
        .find(|service| service.name == "device.logs");
    Json(DeviceLogsView {
        batch: state.device_logs.snapshot(
            query.after,
            query.limit.unwrap_or(crate::device_logs::MAX_BATCH_ENTRIES),
            state.device_log_demand.enabled(),
        ),
        service,
    })
}

#[derive(Serialize)]
struct DeviceLogsView {
    #[serde(flatten)]
    batch: crate::device_logs::DeviceLogBatch,
    service: Option<crate::supervisor::ServiceHealth>,
}

async fn start_device_logs(State(state): State<AppState>) -> StatusCode {
    state.device_log_demand.set(true);
    StatusCode::NO_CONTENT
}

async fn stop_device_logs(State(state): State<AppState>) -> StatusCode {
    state.device_log_demand.set(false);
    StatusCode::NO_CONTENT
}

async fn clear_device_logs(State(state): State<AppState>) -> StatusCode {
    state.device_logs.clear();
    StatusCode::NO_CONTENT
}

fn status_snapshot(state: &AppState) -> StatusView {
    StatusView {
        status: state.status.get(),
        active_udid: state.active.get(),
        error: state.error.get(),
        orientation: orientation_name(state.orientation.get()),
        devices: state
            .devices
            .get()
            .into_iter()
            .map(|device| DeviceView {
                udid: device.udid,
                name: device.name,
                connection: device.connection.label(),
            })
            .collect(),
        location: state.location.get(),
    }
}

async fn refresh_devices(State(state): State<AppState>) -> StatusCode {
    let _ = state.control.send(ControlCmd::Refresh);
    StatusCode::ACCEPTED
}

async fn connect_device(State(state): State<AppState>, Path(udid): Path<String>) -> StatusCode {
    let _ = state.control.send(ControlCmd::Connect(udid));
    StatusCode::ACCEPTED
}

async fn reconnect_device(State(state): State<AppState>, Path(udid): Path<String>) -> StatusCode {
    let _ = state.control.send(ControlCmd::Reconnect(udid));
    StatusCode::ACCEPTED
}

const DEVICE_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const SCREENSHOT_REQUEST_TIMEOUT: Duration = Duration::from_secs(25);
const CRASH_REPORT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const APP_DOCUMENT_REQUEST_TIMEOUT: Duration = Duration::from_secs(11 * 60);

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
    Json(state.location.get())
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
) -> Result<Json<crate::protocol::DeviceDetails>, (StatusCode, String)> {
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

async fn device_screenshot(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::TakeScreenshot(reply)) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    let png = tokio::time::timeout(SCREENSHOT_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "device screenshot request timed out".into(),
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
        png,
    ))
}

async fn restart_device(State(state): State<AppState>) -> Result<StatusCode, (StatusCode, String)> {
    dispatch_device_power_command(&state, true).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn shutdown_device(
    State(state): State<AppState>,
) -> Result<StatusCode, (StatusCode, String)> {
    dispatch_device_power_command(&state, false).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn dispatch_device_power_command(
    state: &AppState,
    restart: bool,
) -> Result<(), (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    let command = if restart {
        InputCmd::RestartDevice(reply)
    } else {
        InputCmd::ShutdownDevice(reply)
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

async fn device_apps(
    State(state): State<AppState>,
) -> Result<Json<Vec<crate::protocol::DeviceApp>>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::ListApps(reply)) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    let apps = tokio::time::timeout(DEVICE_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "app list request timed out".into(),
            )
        })?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device session ended".into(),
            )
        })?
        .map_err(|error| (StatusCode::BAD_GATEWAY, error))?;
    Ok(Json(apps))
}

async fn device_app_icon(
    State(state): State<AppState>,
    Path(bundle_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if !valid_bundle_identifier(&bundle_id) {
        return Err((StatusCode::BAD_REQUEST, "invalid bundle identifier".into()));
    }
    let (reply, response) = oneshot::channel();
    if !state
        .input
        .try_send(InputCmd::GetAppIcon { bundle_id, reply })
    {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    let icon = tokio::time::timeout(DEVICE_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "app icon request timed out".into(),
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
        [
            (CONTENT_TYPE, "image/png"),
            (CACHE_CONTROL, "private, max-age=300"),
        ],
        icon,
    ))
}

#[derive(Deserialize)]
struct AppDocumentQuery {
    #[serde(default = "app_document_root")]
    path: String,
}

fn app_document_root() -> String {
    "/".into()
}

#[derive(Deserialize)]
struct ExportAppDocumentRequest {
    path: String,
    destination: PathBuf,
}

#[derive(Deserialize)]
struct ImportAppDocumentRequest {
    directory: String,
    source: PathBuf,
}

#[derive(Deserialize)]
struct CreateAppDocumentDirectoryRequest {
    directory: String,
    name: String,
}

#[derive(Deserialize)]
struct RenameAppDocumentRequest {
    path: String,
    name: String,
}

async fn app_documents(
    State(state): State<AppState>,
    Path(bundle_id): Path<String>,
    Query(query): Query<AppDocumentQuery>,
) -> Result<Json<crate::app_documents::AppDocumentList>, (StatusCode, String)> {
    validate_app_document_bundle(&bundle_id)?;
    let (reply, response) = oneshot::channel();
    dispatch_app_document_command(
        &state,
        crate::app_documents::AppDocumentCommand::List {
            bundle_id,
            path: query.path,
            reply,
        },
    )?;
    Ok(Json(
        await_app_document_response(response, "application document listing").await?,
    ))
}

async fn export_app_document(
    State(state): State<AppState>,
    Path(bundle_id): Path<String>,
    Json(request): Json<ExportAppDocumentRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    validate_app_document_bundle(&bundle_id)?;
    let (reply, response) = oneshot::channel();
    dispatch_app_document_command(
        &state,
        crate::app_documents::AppDocumentCommand::Export {
            bundle_id,
            path: request.path,
            destination: request.destination,
            reply,
        },
    )?;
    let bytes_written =
        await_app_document_response(response, "application document export").await?;
    Ok(Json(json!({ "bytes_written": bytes_written })))
}

async fn import_app_document(
    State(state): State<AppState>,
    Path(bundle_id): Path<String>,
    Json(request): Json<ImportAppDocumentRequest>,
) -> Result<Json<crate::app_documents::AppDocumentEntry>, (StatusCode, String)> {
    validate_app_document_bundle(&bundle_id)?;
    let (reply, response) = oneshot::channel();
    dispatch_app_document_command(
        &state,
        crate::app_documents::AppDocumentCommand::Import {
            bundle_id,
            directory: request.directory,
            source: request.source,
            reply,
        },
    )?;
    Ok(Json(
        await_app_document_response(response, "application document upload").await?,
    ))
}

async fn create_app_document_directory(
    State(state): State<AppState>,
    Path(bundle_id): Path<String>,
    Json(request): Json<CreateAppDocumentDirectoryRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    validate_app_document_bundle(&bundle_id)?;
    let (reply, response) = oneshot::channel();
    dispatch_app_document_command(
        &state,
        crate::app_documents::AppDocumentCommand::CreateDirectory {
            bundle_id,
            directory: request.directory,
            name: request.name,
            reply,
        },
    )?;
    await_app_document_response(response, "application directory creation").await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn rename_app_document(
    State(state): State<AppState>,
    Path(bundle_id): Path<String>,
    Json(request): Json<RenameAppDocumentRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    validate_app_document_bundle(&bundle_id)?;
    let (reply, response) = oneshot::channel();
    dispatch_app_document_command(
        &state,
        crate::app_documents::AppDocumentCommand::Rename {
            bundle_id,
            path: request.path,
            name: request.name,
            reply,
        },
    )?;
    await_app_document_response(response, "application document rename").await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_app_document(
    State(state): State<AppState>,
    Path(bundle_id): Path<String>,
    Query(query): Query<AppDocumentQuery>,
) -> Result<StatusCode, (StatusCode, String)> {
    validate_app_document_bundle(&bundle_id)?;
    let (reply, response) = oneshot::channel();
    dispatch_app_document_command(
        &state,
        crate::app_documents::AppDocumentCommand::Delete {
            bundle_id,
            path: query.path,
            reply,
        },
    )?;
    await_app_document_response(response, "application document deletion").await?;
    Ok(StatusCode::NO_CONTENT)
}

fn validate_app_document_bundle(bundle_id: &str) -> Result<(), (StatusCode, String)> {
    if valid_bundle_identifier(bundle_id) {
        Ok(())
    } else {
        Err((StatusCode::BAD_REQUEST, "invalid bundle identifier".into()))
    }
}

fn dispatch_app_document_command(
    state: &AppState,
    command: crate::app_documents::AppDocumentCommand,
) -> Result<(), (StatusCode, String)> {
    if state.input.try_send(InputCmd::AppDocuments(command)) {
        Ok(())
    } else {
        Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ))
    }
}

async fn await_app_document_response<T>(
    response: oneshot::Receiver<Result<T, String>>,
    operation: &str,
) -> Result<T, (StatusCode, String)> {
    tokio::time::timeout(APP_DOCUMENT_REQUEST_TIMEOUT, response)
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
        .map_err(|error| {
            let status = if error == "an application document with this name already exists" {
                StatusCode::CONFLICT
            } else if error.starts_with("invalid ")
                || error.contains("root cannot be modified")
                || error.contains("must be a regular file")
                || error.contains("only regular application documents")
            {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::BAD_GATEWAY
            };
            (status, error)
        })
}

async fn app_operation(State(state): State<AppState>) -> Json<crate::protocol::AppOperationView> {
    Json(state.app_operation.get())
}

#[derive(Deserialize)]
struct InstallAppRequest {
    path: PathBuf,
}

async fn install_app(
    State(state): State<AppState>,
    Json(request): Json<InstallAppRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::InstallApp {
        path: request.path,
        reply,
    }) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    await_app_operation_acceptance(response, "app install").await?;
    Ok(StatusCode::ACCEPTED)
}

async fn uninstall_app(
    State(state): State<AppState>,
    Path(bundle_id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    if !valid_bundle_identifier(&bundle_id) {
        return Err((StatusCode::BAD_REQUEST, "invalid bundle identifier".into()));
    }
    let (reply, response) = oneshot::channel();
    if !state
        .input
        .try_send(InputCmd::UninstallApp { bundle_id, reply })
    {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
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
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device session ended".into(),
            )
        })?;
    result.map_err(|error| {
        let status = if error == "another app operation is already running" {
            StatusCode::CONFLICT
        } else {
            StatusCode::BAD_REQUEST
        };
        (status, error)
    })
}

async fn device_provisioning_profiles(
    State(state): State<AppState>,
) -> Result<Json<Vec<crate::protocol::ProvisioningProfile>>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state
        .input
        .try_send(InputCmd::ListProvisioningProfiles(reply))
    {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    let profiles = tokio::time::timeout(DEVICE_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "provisioning profile request timed out".into(),
            )
        })?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device session ended".into(),
            )
        })?
        .map_err(|error| (StatusCode::BAD_GATEWAY, error))?;
    Ok(Json(profiles))
}

async fn launch_app(
    State(state): State<AppState>,
    Path(bundle_id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    if !valid_bundle_identifier(&bundle_id) {
        return Err((StatusCode::BAD_REQUEST, "invalid bundle identifier".into()));
    }
    let (reply, response) = oneshot::channel();
    if !state
        .input
        .try_send(InputCmd::LaunchApp { bundle_id, reply })
    {
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
                "app launch request timed out".into(),
            )
        })?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device session ended".into(),
            )
        })?
        .map_err(|error| (StatusCode::BAD_GATEWAY, error))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn stop_app(
    State(state): State<AppState>,
    Path(bundle_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if !valid_bundle_identifier(&bundle_id) {
        return Err((StatusCode::BAD_REQUEST, "invalid bundle identifier".into()));
    }
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::StopApp { bundle_id, reply }) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    let was_running = tokio::time::timeout(DEVICE_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "app stop request timed out".into(),
            )
        })?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device session ended".into(),
            )
        })?
        .map_err(|error| (StatusCode::BAD_GATEWAY, error))?;
    Ok(Json(serde_json::json!({ "was_running": was_running })))
}

async fn device_crash_reports(
    State(state): State<AppState>,
) -> Result<Json<crate::protocol::DeviceCrashReportList>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::ListCrashReports(reply)) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    let reports = tokio::time::timeout(CRASH_REPORT_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "crash report list request timed out".into(),
            )
        })?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device session ended".into(),
            )
        })?
        .map_err(|error| (StatusCode::BAD_GATEWAY, error))?;
    Ok(Json(reports))
}

#[derive(Deserialize)]
struct ExportCrashReportRequest {
    device_path: String,
    destination: PathBuf,
}

async fn export_crash_report(
    State(state): State<AppState>,
    Json(request): Json<ExportCrashReportRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::ExportCrashReport {
        device_path: request.device_path,
        destination: request.destination,
        reply,
    }) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    let bytes_written = tokio::time::timeout(CRASH_REPORT_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "crash report export timed out".into(),
            )
        })?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device session ended".into(),
            )
        })?
        .map_err(|error| (StatusCode::BAD_GATEWAY, error))?;
    Ok(Json(serde_json::json!({ "bytes_written": bytes_written })))
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

fn profile_path(state: &AppState, name: &str) -> Result<PathBuf, StatusCode> {
    if name.is_empty()
        || name.len() > 80
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(state.profile_dir.join(format!("{name}.json")))
}

fn active_profile_path(state: &AppState) -> PathBuf {
    state.profile_dir.join(".active-profile")
}

async fn active_profile_name(state: &AppState) -> String {
    tokio::fs::read_to_string(active_profile_path(state))
        .await
        .ok()
        .map(|name| name.trim().to_string())
        .filter(|name| profile_path(state, name).is_ok())
        .unwrap_or_else(|| "default".into())
}

async fn list_profiles(State(state): State<AppState>) -> Result<Json<ProfileList>, StatusCode> {
    tokio::fs::create_dir_all(state.profile_dir.as_ref())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut entries = tokio::fs::read_dir(state.profile_dir.as_ref())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut profiles = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        if let Some(name) = path.file_stem().and_then(|name| name.to_str())
            && profile_path(&state, name).is_ok()
        {
            profiles.push(name.to_string());
        }
    }
    profiles.sort();
    let requested_active = active_profile_name(&state).await;
    let active = if profiles.contains(&requested_active) {
        requested_active
    } else {
        profiles
            .first()
            .cloned()
            .unwrap_or_else(|| "default".into())
    };
    let mut app_bindings = BTreeMap::new();
    let mut binding_conflicts = HashSet::new();
    for name in &profiles {
        let Ok(bytes) = tokio::fs::read(profile_path(&state, name)?).await else {
            continue;
        };
        let Ok(profile) = serde_json::from_slice::<Profile>(&bytes) else {
            continue;
        };
        if validate_profile(&profile).is_err() {
            continue;
        }
        for bundle_id in profile.bundle_identifiers {
            if binding_conflicts.contains(&bundle_id) {
                continue;
            }
            if app_bindings
                .insert(bundle_id.clone(), name.clone())
                .is_some()
            {
                app_bindings.remove(&bundle_id);
                binding_conflicts.insert(bundle_id);
            }
        }
    }
    let mut binding_conflicts = binding_conflicts.into_iter().collect::<Vec<_>>();
    binding_conflicts.sort();
    Ok(Json(ProfileList {
        profiles,
        active,
        app_bindings,
        binding_conflicts,
    }))
}

async fn load_profile(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Profile>, StatusCode> {
    let path = profile_path(&state, &name)?;
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        })?;
    let profile: Profile =
        serde_json::from_slice(&bytes).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    validate_profile(&profile)?;
    Ok(Json(profile))
}

async fn save_profile(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(profile): Json<Profile>,
) -> Result<StatusCode, StatusCode> {
    let path = profile_path(&state, &name)?;
    validate_profile(&profile)?;
    tokio::fs::create_dir_all(state.profile_dir.as_ref())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let bytes = serde_json::to_vec_pretty(&profile).map_err(|_| StatusCode::BAD_REQUEST)?;
    tokio::fs::write(path, bytes)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn activate_profile(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let path = profile_path(&state, &name)?;
    if !tokio::fs::try_exists(path)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        return Err(StatusCode::NOT_FOUND);
    }
    tokio::fs::write(active_profile_path(&state), name)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_profile(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let path = profile_path(&state, &name)?;
    if active_profile_name(&state).await == name {
        return Err(StatusCode::CONFLICT);
    }
    tokio::fs::remove_file(path)
        .await
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        })?;
    Ok(Json(json!({ "deleted": name })))
}

fn validate_profile(profile: &Profile) -> Result<(), StatusCode> {
    if profile.version != 1
        || profile.name.is_empty()
        || profile.mappings.len() > 512
        || profile.hardware_bindings.len() != HARDWARE_BUTTON_NAMES.len()
        || profile.bundle_identifiers.len() > 32
        || HARDWARE_BUTTON_NAMES
            .iter()
            .any(|name| !profile.hardware_bindings.contains_key(*name))
    {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let mut bundle_identifiers = HashSet::new();
    if profile.bundle_identifiers.iter().any(|bundle_id| {
        !valid_bundle_identifier(bundle_id) || !bundle_identifiers.insert(bundle_id.as_str())
    }) {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let mut ids = HashSet::new();
    for mapping in &profile.mappings {
        let Some(mapping) = mapping.as_object() else {
            return Err(StatusCode::UNPROCESSABLE_ENTITY);
        };
        let id = mapping.get("id").and_then(serde_json::Value::as_str);
        let mapping_type = mapping.get("type").and_then(serde_json::Value::as_str);
        if id.is_none_or(str::is_empty)
            || !ids.insert(id.unwrap())
            || !mapping_type.is_some_and(valid_mapping_type)
            || !valid_mapping_positions(mapping)
        {
            return Err(StatusCode::UNPROCESSABLE_ENTITY);
        }
    }
    let mut mapping_keys = HashSet::new();
    for mapping in &profile.mappings {
        collect_mapping_keys(mapping, &mut mapping_keys);
    }
    let mut hardware_keys = HashSet::new();
    for key in profile.hardware_bindings.values() {
        if key.len() > 64
            || !key
                .chars()
                .all(|character| character.is_ascii_alphanumeric())
            || (!key.is_empty() && mapping_keys.contains(key.as_str()))
            || (!key.is_empty() && !hardware_keys.insert(key))
        {
            return Err(StatusCode::UNPROCESSABLE_ENTITY);
        }
    }
    Ok(())
}

fn valid_mapping_type(mapping_type: &str) -> bool {
    matches!(
        mapping_type,
        "touch"
            | "dpad"
            | "SingleTap"
            | "RepeatTap"
            | "MultipleTap"
            | "Swipe"
            | "DirectionPad"
            | "MouseCastSpell"
            | "PadCastSpell"
            | "CancelCast"
            | "Observation"
            | "Fps"
            | "Fire"
            | "RawInput"
            | "Script"
    )
}

fn valid_mapping_positions(mapping: &serde_json::Map<String, serde_json::Value>) -> bool {
    fn valid_position(value: &serde_json::Value) -> bool {
        let Some(point) = value.as_object() else {
            return false;
        };
        let Some(x) = point.get("x").and_then(serde_json::Value::as_f64) else {
            return false;
        };
        let Some(y) = point.get("y").and_then(serde_json::Value::as_f64) else {
            return false;
        };
        x.is_finite() && y.is_finite() && (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y)
    }
    let primary = if mapping.contains_key("position") {
        mapping.get("position").is_some_and(valid_position)
    } else {
        mapping
            .get("x")
            .and_then(serde_json::Value::as_f64)
            .is_some_and(|x| (0.0..=1.0).contains(&x))
            && mapping
                .get("y")
                .and_then(serde_json::Value::as_f64)
                .is_some_and(|y| (0.0..=1.0).contains(&y))
    };
    primary
        && mapping.get("center").is_none_or(valid_position)
        && mapping.get("positions").is_none_or(|values| {
            values
                .as_array()
                .is_some_and(|values| values.iter().all(valid_position))
        })
        && mapping.get("items").is_none_or(|values| {
            values.as_array().is_some_and(|values| {
                values
                    .iter()
                    .all(|item| item.get("position").is_some_and(valid_position))
            })
        })
}

fn collect_mapping_keys<'a>(value: &'a serde_json::Value, keys: &mut HashSet<&'a str>) {
    match value {
        serde_json::Value::Array(values) => values
            .iter()
            .for_each(|value| collect_mapping_keys(value, keys)),
        serde_json::Value::Object(values) => {
            for (name, value) in values {
                if name == "key"
                    || name == "bind"
                    || name.ends_with("_bind")
                    || matches!(name.as_str(), "up" | "down" | "left" | "right")
                {
                    collect_mapping_keys(value, keys);
                }
            }
        }
        serde_json::Value::String(value) if !value.is_empty() => {
            keys.insert(value);
        }
        _ => {}
    }
}

async fn ws_upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.protocols(["devicehub-mask"])
        .on_upgrade(move |socket| websocket(socket, state))
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    MultiTouch {
        contacts: Vec<WebContact>,
    },
    Button {
        name: String,
    },
    ButtonDown {
        name: String,
    },
    ButtonUp {
        name: String,
    },
    KeyboardDown {
        usage: u64,
    },
    KeyboardUp {
        usage: u64,
    },
    Text {
        text: String,
    },
    Rotate {
        direction: RotateRequest,
    },
    FramePresented,
    FrontendMetrics {
        window_ms: f64,
        received_frames: u64,
        replaced_frames: u64,
        presented_frames: u64,
        jpeg_decode_ms: f64,
        canvas_draw_ms: f64,
        decode_errors: u64,
    },
}

#[derive(Deserialize)]
struct WebContact {
    identity: u8,
    touching: bool,
    x: f32,
    y: f32,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RotateRequest {
    Left,
    Right,
}

async fn websocket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let send_state = state.clone();
    let max_in_flight_frames = configured_in_flight_frames(
        std::env::var_os("DEVICEHUB_VIDEO_IN_FLIGHT_FRAMES").as_deref(),
    );
    tracing::debug!(max_in_flight_frames, "configured video frame pipeline");
    let frame_pacer = Arc::new(FramePacer::new(max_in_flight_frames));
    let send_pacer = frame_pacer.clone();
    let send_task = tokio::spawn(async move {
        let mut last_status = String::new();
        let mut frame_rx = send_state.frames.subscribe();
        let mut audio_rx = send_state.audio.subscribe();
        let mut clipboard_rx = send_state.clipboard.subscribe();
        let mut device_event_rx = send_state.device_events.subscribe();
        let mut status_tick = tokio::time::interval(Duration::from_millis(250));
        status_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut metrics_tick = tokio::time::interval(Duration::from_secs(1));
        metrics_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut metrics_started = Instant::now();
        let mut metrics_counters = send_state.video_counters.snapshot();
        let mut metrics_frame_version = send_state.frames.version();
        let mut sent_frames = 0_u64;
        let mut sent_bytes = 0_u64;
        let mut encoded_frames = 0_u64;
        let mut encoding_time = Duration::ZERO;
        let mut frame_age = Duration::ZERO;
        let mut websocket_send_time = Duration::ZERO;
        let mut skipped_for_backpressure = 0_u64;
        let mut metrics_log_windows = 0_u8;
        loop {
            tokio::select! {
                _ = status_tick.tick() => {
                    let snapshot = status_snapshot(&send_state);
                    if let Ok(text) = serde_json::to_string(
                        &json!({"type": "status", "payload": snapshot}),
                    ) && text != last_status {
                        last_status = text.clone();
                        if sender.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                }
                changed = frame_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let Some(frame) = frame_rx.borrow_and_update().clone() else {
                        continue;
                    };
                    if !send_pacer.try_acquire() {
                        skipped_for_backpressure += 1;
                        continue;
                    }
                    let cached = frame.jpeg.get().is_some();
                    let encode_started = Instant::now();
                    frame_age += encode_started.saturating_duration_since(frame.decoded_at);
                    let encoded = tokio::task::spawn_blocking(move || encode_jpeg(&frame)).await;
                    let Ok(Ok(jpeg)) = encoded else {
                        send_pacer.release();
                        continue;
                    };
                    if !cached {
                        encoded_frames += 1;
                        encoding_time += encode_started.elapsed();
                    }
                    sent_frames += 1;
                    sent_bytes += jpeg.len() as u64;
                    let send_started = Instant::now();
                    if sender.send(Message::Binary(jpeg)).await.is_err() {
                        break;
                    }
                    websocket_send_time += send_started.elapsed();
                }
                audio = audio_rx.recv() => {
                    match audio {
                        Ok(pcm) => {
                            let framed = encode_audio_envelope(pcm);
                            match tokio::time::timeout(
                                Duration::from_millis(50),
                                sender.send(Message::Binary(framed)),
                            ).await {
                                Ok(Ok(())) => {}
                                Ok(Err(_)) => break,
                                Err(_) => tracing::debug!("dropping audio chunk due to WebSocket backpressure"),
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::debug!(skipped, "WebSocket audio receiver skipped stale chunks");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                clipboard = clipboard_rx.recv() => {
                    match clipboard {
                        Ok(event) => {
                            let Ok(text) = serde_json::to_string(
                                &json!({"type": "clipboard", "payload": event}),
                            ) else {
                                continue;
                            };
                            if sender.send(Message::Text(text.into())).await.is_err() {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::debug!(skipped, "WebSocket clipboard receiver skipped stale events");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                device_event = device_event_rx.recv() => {
                    match device_event {
                        Ok(event) => {
                            let Ok(text) = serde_json::to_string(
                                &json!({"type": "device_event", "payload": event}),
                            ) else {
                                continue;
                            };
                            if sender.send(Message::Text(text.into())).await.is_err() {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::debug!(skipped, "WebSocket device event receiver skipped stale events");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = metrics_tick.tick() => {
                    let elapsed = metrics_started.elapsed().as_secs_f64().max(f64::EPSILON);
                    let counters = send_state.video_counters.snapshot();
                    let version = send_state.frames.version();
                    let source_frames = counters.source_frames.wrapping_sub(metrics_counters.source_frames);
                    let decoded_frames = counters.decoded_frames.wrapping_sub(metrics_counters.decoded_frames);
                    let published_frames = version.wrapping_sub(metrics_frame_version);
                    let pacer = send_pacer.take_metrics();
                    let metrics = StreamMetricsView {
                        source_fps: source_frames as f64 / elapsed,
                        decoded_fps: decoded_frames as f64 / elapsed,
                        published_fps: published_frames as f64 / elapsed,
                        sent_fps: sent_frames as f64 / elapsed,
                        backend_dropped_fps: published_frames.saturating_sub(sent_frames) as f64 / elapsed,
                        jpeg_encode_ms: if encoded_frames == 0 {
                            0.0
                        } else {
                            encoding_time.as_secs_f64() * 1000.0 / encoded_frames as f64
                        },
                        frame_age_ms: duration_average_ms(frame_age, sent_frames),
                        websocket_send_ms: duration_average_ms(websocket_send_time, sent_frames),
                        presentation_ack_ms: pacer.average_ack_ms,
                        megabits_per_second: sent_bytes as f64 * 8.0 / elapsed / 1_000_000.0,
                    };
                    metrics_log_windows += 1;
                    if metrics_log_windows >= 5 {
                        tracing::debug!(
                            target: "devicehub_mask::perf",
                            decoded_fps = metrics.decoded_fps,
                            source_fps = metrics.source_fps,
                            published_fps = metrics.published_fps,
                            sent_fps = metrics.sent_fps,
                            backend_dropped_fps = metrics.backend_dropped_fps,
                            duplicate_fps = counters.duplicate_frames.wrapping_sub(metrics_counters.duplicate_frames) as f64 / elapsed,
                            skipped_for_backpressure,
                            jpeg_encode_ms = metrics.jpeg_encode_ms,
                            frame_age_ms = metrics.frame_age_ms,
                            websocket_send_ms = metrics.websocket_send_ms,
                            presentation_ack_ms = metrics.presentation_ack_ms,
                            presentation_ack_max_ms = pacer.max_ack_ms,
                            expired_frame_credits = pacer.expired_credits,
                            megabits_per_second = metrics.megabits_per_second,
                            "video output performance"
                        );
                        metrics_log_windows = 0;
                    }
                    let Ok(text) = serde_json::to_string(
                        &json!({"type": "metrics", "payload": metrics}),
                    ) else {
                        continue;
                    };
                    if sender.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                    metrics_started = Instant::now();
                    metrics_counters = counters;
                    metrics_frame_version = version;
                    sent_frames = 0;
                    sent_bytes = 0;
                    encoded_frames = 0;
                    encoding_time = Duration::ZERO;
                    frame_age = Duration::ZERO;
                    websocket_send_time = Duration::ZERO;
                    skipped_for_backpressure = 0;
                }
            }
        }
    });

    let mut pressed_keyboard = HashSet::new();
    while let Some(Ok(message)) = receiver.next().await {
        match message {
            Message::Text(text) => {
                if handle_client_message(&state, &text, &mut pressed_keyboard) {
                    frame_pacer.release();
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    send_task.abort();
    send_all_up(&state, &pressed_keyboard);
}

const FRAME_CREDIT_LEASE: Duration = Duration::from_millis(500);
const DEFAULT_IN_FLIGHT_FRAMES: usize = 2;

fn configured_in_flight_frames(value: Option<&std::ffi::OsStr>) -> usize {
    match value.and_then(|value| value.to_str()) {
        None | Some("") | Some("2") => DEFAULT_IN_FLIGHT_FRAMES,
        Some("1") => 1,
        Some(value) => {
            tracing::warn!(value, "ignoring invalid DEVICEHUB_VIDEO_IN_FLIGHT_FRAMES");
            DEFAULT_IN_FLIGHT_FRAMES
        }
    }
}

fn duration_average_ms(total: Duration, samples: u64) -> f64 {
    if samples == 0 {
        0.0
    } else {
        total.as_secs_f64() * 1000.0 / samples as f64
    }
}

#[derive(Default)]
struct FramePacerState {
    acquired_at: VecDeque<Instant>,
    acknowledgements: u64,
    total_ack_time: Duration,
    max_ack_time: Duration,
    expired_credits: u64,
}

#[derive(Debug, Clone, Copy)]
struct FramePacerMetrics {
    average_ack_ms: f64,
    max_ack_ms: f64,
    expired_credits: u64,
}

struct FramePacer {
    max_in_flight: usize,
    state: Mutex<FramePacerState>,
}

impl FramePacer {
    fn new(max_in_flight: usize) -> Self {
        Self {
            max_in_flight,
            state: Mutex::new(FramePacerState::default()),
        }
    }

    fn try_acquire(&self) -> bool {
        let mut state = self.state.lock().expect("frame pacer lock poisoned");
        while state
            .acquired_at
            .front()
            .is_some_and(|acquired_at| acquired_at.elapsed() >= FRAME_CREDIT_LEASE)
        {
            state.acquired_at.pop_front();
            state.expired_credits = state.expired_credits.saturating_add(1);
        }
        if state.acquired_at.len() >= self.max_in_flight {
            return false;
        }
        state.acquired_at.push_back(Instant::now());
        true
    }

    fn release(&self) {
        let mut state = self.state.lock().expect("frame pacer lock poisoned");
        if let Some(acquired_at) = state.acquired_at.pop_front() {
            let elapsed = acquired_at.elapsed();
            state.acknowledgements = state.acknowledgements.saturating_add(1);
            state.total_ack_time += elapsed;
            state.max_ack_time = state.max_ack_time.max(elapsed);
        }
    }

    fn take_metrics(&self) -> FramePacerMetrics {
        let mut state = self.state.lock().expect("frame pacer lock poisoned");
        let metrics = FramePacerMetrics {
            average_ack_ms: duration_average_ms(state.total_ack_time, state.acknowledgements),
            max_ack_ms: state.max_ack_time.as_secs_f64() * 1000.0,
            expired_credits: state.expired_credits,
        };
        state.acknowledgements = 0;
        state.total_ack_time = Duration::ZERO;
        state.max_ack_time = Duration::ZERO;
        state.expired_credits = 0;
        metrics
    }
}

thread_local! {
    static JPEG_COMPRESSOR: RefCell<Option<turbojpeg::Compressor>> = const { RefCell::new(None) };
}

fn encode_jpeg(frame: &Frame) -> Result<bytes::Bytes, String> {
    frame
        .jpeg
        .get_or_init(|| {
            JPEG_COMPRESSOR.with(|slot| {
                let mut slot = slot.borrow_mut();
                if slot.is_none() {
                    let mut compressor =
                        turbojpeg::Compressor::new().map_err(|error| error.to_string())?;
                    compressor
                        .set_quality(80)
                        .map_err(|error| error.to_string())?;
                    compressor
                        .set_subsamp(turbojpeg::Subsamp::Sub2x2)
                        .map_err(|error| error.to_string())?;
                    *slot = Some(compressor);
                }
                let compressor = slot.as_mut().expect("JPEG compressor initialized");
                let encoded = match frame.format {
                    FrameFormat::Rgb24 => compressor.compress_to_vec(turbojpeg::Image {
                        pixels: frame.pixels.as_slice(),
                        width: frame.width,
                        pitch: frame.width * 3,
                        height: frame.height,
                        format: turbojpeg::PixelFormat::RGB,
                    }),
                    FrameFormat::Yuv420p => compressor.compress_yuv_to_vec(turbojpeg::YuvImage {
                        pixels: frame.pixels.as_slice(),
                        width: frame.width,
                        align: 1,
                        height: frame.height,
                        subsamp: turbojpeg::Subsamp::Sub2x2,
                    }),
                };
                encoded
                    .map(bytes::Bytes::from)
                    .map_err(|error| error.to_string())
            })
        })
        .clone()
}

/// Returns true when the WebView has consumed its outstanding video frame.
fn handle_client_message(
    state: &AppState,
    text: &str,
    pressed_keyboard: &mut HashSet<u64>,
) -> bool {
    let Ok(message) = serde_json::from_str::<ClientMessage>(text) else {
        return false;
    };
    match message {
        ClientMessage::FramePresented => return true,
        ClientMessage::FrontendMetrics {
            window_ms,
            received_frames,
            replaced_frames,
            presented_frames,
            jpeg_decode_ms,
            canvas_draw_ms,
            decode_errors,
        } => {
            if valid_frontend_metrics(
                window_ms,
                received_frames,
                replaced_frames,
                presented_frames,
                jpeg_decode_ms,
                canvas_draw_ms,
                decode_errors,
            ) {
                let elapsed = (window_ms / 1000.0).max(f64::EPSILON);
                tracing::debug!(
                    target: "devicehub_mask::perf",
                    received_fps = received_frames as f64 / elapsed,
                    presented_fps = presented_frames as f64 / elapsed,
                    received_frames,
                    replaced_frames,
                    presented_frames,
                    jpeg_decode_ms = jpeg_decode_ms / received_frames.max(1) as f64,
                    canvas_draw_ms = canvas_draw_ms / presented_frames.max(1) as f64,
                    decode_errors,
                    "frontend video performance"
                );
            }
        }
        ClientMessage::MultiTouch { contacts } => {
            if let Some(contacts) = validate_contacts(contacts, state.orientation.get()) {
                state.input.send(InputCmd::MultiTouchFrame(contacts));
            }
        }
        ClientMessage::Button { name } => {
            if let Some(name) = hardware_button_name(&name) {
                state.input.send(InputCmd::Button(name));
            }
        }
        ClientMessage::ButtonDown { name } => {
            if let Some(name) = hardware_button_name(&name) {
                state.input.send(InputCmd::ButtonDown(name));
            }
        }
        ClientMessage::ButtonUp { name } => {
            if let Some(name) = hardware_button_name(&name) {
                state.input.send(InputCmd::ButtonUp(name));
            }
        }
        ClientMessage::KeyboardDown { usage } => {
            if valid_keyboard_usage(usage) && pressed_keyboard.insert(usage) {
                state.input.send(InputCmd::KeyboardDown(usage));
            }
        }
        ClientMessage::KeyboardUp { usage } => {
            if valid_keyboard_usage(usage) && pressed_keyboard.remove(&usage) {
                state.input.send(InputCmd::KeyboardUp(usage));
            }
        }
        ClientMessage::Text { text } => {
            if !text.is_empty() && text.len() <= 512 && text.chars().count() <= 128 {
                state.input.send(InputCmd::Text(text));
            }
        }
        ClientMessage::Rotate { direction } => {
            state.input.send(InputCmd::Rotate(match direction {
                RotateRequest::Left => RotateDir::Left,
                RotateRequest::Right => RotateDir::Right,
            }))
        }
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn valid_frontend_metrics(
    window_ms: f64,
    received_frames: u64,
    replaced_frames: u64,
    presented_frames: u64,
    jpeg_decode_ms: f64,
    canvas_draw_ms: f64,
    decode_errors: u64,
) -> bool {
    (500.0..=60_000.0).contains(&window_ms)
        && jpeg_decode_ms.is_finite()
        && canvas_draw_ms.is_finite()
        && (0.0..=window_ms * 10.0).contains(&jpeg_decode_ms)
        && (0.0..=window_ms * 10.0).contains(&canvas_draw_ms)
        && received_frames <= 10_000
        && replaced_frames <= received_frames
        && presented_frames <= received_frames
        && decode_errors <= received_frames
}

fn validate_contacts(
    contacts: Vec<WebContact>,
    orientation: Orientation,
) -> Option<Vec<TouchContact>> {
    if contacts.len() > 5 {
        return None;
    }
    let mut identities = HashSet::new();
    let turns = orientation.quarter_turns_cw();
    contacts
        .into_iter()
        .map(|contact| {
            if contact.identity >= 5
                || !identities.insert(contact.identity)
                || !contact.x.is_finite()
                || !contact.y.is_finite()
                || !(0.0..=1.0).contains(&contact.x)
                || !(0.0..=1.0).contains(&contact.y)
            {
                return None;
            }
            let (x, y) = unrotate_norm(contact.x, contact.y, turns);
            Some(TouchContact {
                identity: contact.identity,
                touching: contact.touching,
                x: norm(x),
                y: norm(y),
            })
        })
        .collect()
}

fn send_all_up(state: &AppState, pressed_keyboard: &HashSet<u64>) {
    state.input.send(InputCmd::MultiTouchFrame(
        (0..5)
            .map(|identity| TouchContact {
                identity,
                touching: false,
                x: 0,
                y: 0,
            })
            .collect(),
    ));
    for name in HARDWARE_BUTTON_NAMES {
        state.input.send(InputCmd::ButtonUp(name));
    }
    for usage in pressed_keyboard {
        state.input.send(InputCmd::KeyboardUp(*usage));
    }
}

fn valid_keyboard_usage(usage: u64) -> bool {
    matches!(usage, 0x04..=0x73 | 0x85 | 0x87 | 0x89 | 0xe0..=0xe7)
}

fn hardware_button_name(name: &str) -> Option<&'static str> {
    HARDWARE_BUTTON_NAMES
        .into_iter()
        .find(|candidate| *candidate == name)
}

fn orientation_name(orientation: Orientation) -> &'static str {
    match orientation {
        Orientation::Portrait => "portrait",
        Orientation::PortraitUpsideDown => "portrait_upside_down",
        Orientation::LandscapeLeft => "landscape_left",
        Orientation::LandscapeRight => "landscape_right",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    fn test_state() -> (AppState, UnboundedReceiver<InputCmd>) {
        let (state, input_rx, _control_rx) = test_state_with_control();
        (state, input_rx)
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
        (
            AppState {
                frames: FrameSlot::default(),
                audio: AudioSlot::default(),
                clipboard: ClipboardSlot::default(),
                device_events: crate::device_events::DeviceEventSlot::default(),
                video_counters: VideoCounters::default(),
                status: StatusSlot::default(),
                orientation: OrientationSlot::default(),
                devices: DeviceListSlot::default(),
                active: ActiveSlot::default(),
                error: ErrorSlot::default(),
                app_operation: AppOperationSlot::default(),
                location: LocationStatusSlot::default(),
                performance: PerformanceSlot::default(),
                performance_demand: PerformanceDemand::default(),
                device_logs: crate::device_logs::DeviceLogSlot::default(),
                device_log_demand: crate::device_logs::DeviceLogDemand::default(),
                services: ServiceRegistry::default(),
                input,
                control,
                profile_dir: Arc::new(PathBuf::new()),
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
    async fn performance_sampling_endpoint_controls_demand() {
        let (state, _) = test_state();
        assert!(!state.performance_demand.enabled());
        assert_eq!(
            start_performance_sampling(State(state.clone())).await,
            StatusCode::NO_CONTENT
        );
        assert!(state.performance_demand.enabled());
        let view = performance(State(state.clone())).await.0;
        assert!(view.sampling);
        assert_eq!(
            stop_performance_sampling(State(state.clone())).await,
            StatusCode::NO_CONTENT
        );
        assert!(!state.performance_demand.enabled());
    }

    #[tokio::test]
    async fn device_log_endpoints_bound_batches_and_control_demand() {
        let (state, _) = test_state();
        for index in 0..3 {
            state.device_logs.publish(format!("line {index}"));
        }
        assert_eq!(
            start_device_logs(State(state.clone())).await,
            StatusCode::NO_CONTENT
        );
        let view = device_logs(
            State(state.clone()),
            axum::extract::Query(DeviceLogQuery {
                after: Some(1),
                limit: Some(1),
            }),
        )
        .await
        .0;
        assert!(view.batch.streaming);
        assert_eq!(view.batch.entries.len(), 1);
        assert_eq!(view.batch.entries[0].sequence, 2);
        assert!(!view.batch.cursor_lagged);
        assert!(view.batch.has_more);

        assert_eq!(
            clear_device_logs(State(state.clone())).await,
            StatusCode::NO_CONTENT
        );
        assert!(
            state
                .device_logs
                .snapshot(None, 10, true)
                .entries
                .is_empty()
        );
        assert_eq!(
            stop_device_logs(State(state.clone())).await,
            StatusCode::NO_CONTENT
        );
        assert!(!state.device_log_demand.enabled());
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

    fn test_frame() -> Frame {
        Frame {
            width: 2,
            height: 1,
            format: FrameFormat::Rgb24,
            pixels: vec![255, 0, 0, 0, 255, 0],
            decoded_at: Instant::now(),
            jpeg: std::sync::OnceLock::new(),
        }
    }

    #[test]
    fn jpeg_encoding_is_valid_and_cached() {
        let frame = test_frame();
        let first = encode_jpeg(&frame).unwrap();
        let second = encode_jpeg(&frame).unwrap();

        assert_eq!(first.as_ptr(), second.as_ptr());
        let decoded =
            image::load_from_memory_with_format(&first, image::ImageFormat::Jpeg).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (2, 1));
    }

    #[test]
    fn jpeg_encoding_accepts_yuv420p_without_rgb_conversion() {
        let frame = Frame {
            width: 2,
            height: 2,
            format: FrameFormat::Yuv420p,
            pixels: vec![76, 76, 76, 76, 85, 255],
            decoded_at: Instant::now(),
            jpeg: std::sync::OnceLock::new(),
        };
        let encoded = encode_jpeg(&frame).unwrap();
        let decoded =
            image::load_from_memory_with_format(&encoded, image::ImageFormat::Jpeg).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (2, 2));
    }

    #[test]
    fn frame_slot_version_advances_on_publish() {
        let slot = FrameSlot::default();
        assert_eq!(slot.version(), 0);
        slot.publish(Arc::new(test_frame()));
        assert_eq!(slot.version(), 1);
        slot.publish(Arc::new(test_frame()));
        assert_eq!(slot.version(), 2);
    }

    #[tokio::test]
    async fn frame_slot_notifies_with_only_the_latest_frame() {
        let slot = FrameSlot::default();
        let mut receiver = slot.subscribe();
        slot.publish(Arc::new(test_frame()));
        let latest = Arc::new(test_frame());
        slot.publish(latest.clone());

        receiver.changed().await.unwrap();
        let received = receiver.borrow_and_update().clone().unwrap();
        assert!(Arc::ptr_eq(&received, &latest));
    }

    #[test]
    fn frame_pacer_bounds_pipeline_until_a_frame_is_presented() {
        let pacer = FramePacer::new(2);
        assert!(pacer.try_acquire());
        assert!(pacer.try_acquire());
        assert!(!pacer.try_acquire());

        pacer.release();
        assert!(pacer.try_acquire());

        pacer.release();
        pacer.release();
        pacer.release();
        assert!(pacer.try_acquire());
        assert!(pacer.try_acquire());
        assert!(!pacer.try_acquire());
    }

    #[test]
    fn frame_pipeline_depth_accepts_only_bounded_diagnostic_values() {
        assert_eq!(configured_in_flight_frames(None), 2);
        assert_eq!(
            configured_in_flight_frames(Some(std::ffi::OsStr::new("1"))),
            1
        );
        assert_eq!(
            configured_in_flight_frames(Some(std::ffi::OsStr::new("2"))),
            2
        );
        assert_eq!(
            configured_in_flight_frames(Some(std::ffi::OsStr::new("16"))),
            2
        );
    }

    #[test]
    fn expired_frame_credit_does_not_stall_stream() {
        let pacer = FramePacer {
            max_in_flight: 2,
            state: Mutex::new(FramePacerState {
                acquired_at: VecDeque::from([Instant::now() - FRAME_CREDIT_LEASE]),
                ..FramePacerState::default()
            }),
        };
        assert!(pacer.try_acquire());
        assert!(pacer.try_acquire());
        assert!(!pacer.try_acquire());
    }

    #[test]
    fn frame_presented_message_releases_video_credit() {
        let (state, _input_rx) = test_state();
        assert!(handle_client_message(
            &state,
            r#"{"type":"frame_presented"}"#,
            &mut HashSet::new(),
        ));
    }

    #[test]
    fn frontend_metrics_reject_impossible_or_unbounded_values() {
        assert!(valid_frontend_metrics(
            5_000.0, 300, 0, 299, 600.0, 100.0, 1
        ));
        assert!(!valid_frontend_metrics(
            5_000.0, 300, 301, 299, 600.0, 100.0, 1,
        ));
        assert!(!valid_frontend_metrics(f64::NAN, 0, 0, 0, 0.0, 0.0, 0,));
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
    fn legacy_profile_gets_empty_hardware_bindings() {
        let profile: Profile = serde_json::from_value(json!({
            "version": 1,
            "name": "legacy",
            "mappings": []
        }))
        .unwrap();

        assert_eq!(profile.hardware_bindings, default_hardware_bindings());
        assert!(validate_profile(&profile).is_ok());
    }

    #[test]
    fn profile_rejects_duplicate_hardware_shortcuts() {
        let mut profile = Profile {
            version: 1,
            name: "duplicate".into(),
            mappings: Vec::new(),
            bundle_identifiers: Vec::new(),
            hardware_bindings: default_hardware_bindings(),
        };
        profile
            .hardware_bindings
            .insert("home".into(), "KeyH".into());
        profile
            .hardware_bindings
            .insert("lock".into(), "KeyH".into());

        assert!(validate_profile(&profile).is_err());
    }

    #[test]
    fn profile_rejects_hardware_and_touch_shortcut_conflict() {
        let mut profile = Profile {
            version: 1,
            name: "conflict".into(),
            mappings: vec![json!({
                "id": "touch", "type": "touch", "label": "Touch",
                "contactId": 0, "x": 0.5, "y": 0.5, "key": "KeyH"
            })],
            bundle_identifiers: Vec::new(),
            hardware_bindings: default_hardware_bindings(),
        };
        profile
            .hardware_bindings
            .insert("home".into(), "KeyH".into());

        assert!(validate_profile(&profile).is_err());
    }

    #[test]
    fn keyboard_messages_validate_and_track_pressed_usages() {
        let (state, mut input_rx) = test_state();
        let mut pressed = HashSet::new();

        handle_client_message(
            &state,
            r#"{"type":"keyboard_down","usage":4}"#,
            &mut pressed,
        );
        handle_client_message(
            &state,
            r#"{"type":"keyboard_down","usage":4}"#,
            &mut pressed,
        );
        handle_client_message(
            &state,
            r#"{"type":"keyboard_down","usage":65535}"#,
            &mut pressed,
        );

        assert!(matches!(input_rx.try_recv(), Ok(InputCmd::KeyboardDown(4))));
        assert!(input_rx.try_recv().is_err());
        assert_eq!(pressed, HashSet::from([4]));

        handle_client_message(&state, r#"{"type":"keyboard_up","usage":4}"#, &mut pressed);
        assert!(matches!(input_rx.try_recv(), Ok(InputCmd::KeyboardUp(4))));
        assert!(pressed.is_empty());
    }

    #[test]
    fn text_messages_are_bounded_before_dispatch() {
        let (state, mut input_rx) = test_state();
        let mut pressed = HashSet::new();

        handle_client_message(
            &state,
            r#"{"type":"text","text":"Hello, iPhone!"}"#,
            &mut pressed,
        );
        handle_client_message(&state, r#"{"type":"text","text":""}"#, &mut pressed);
        let oversized =
            serde_json::to_string(&json!({ "type": "text", "text": "x".repeat(129) })).unwrap();
        handle_client_message(&state, &oversized, &mut pressed);

        assert!(matches!(
            input_rx.try_recv(),
            Ok(InputCmd::Text(text)) if text == "Hello, iPhone!"
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
        send_all_up(&state, &HashSet::from([0x04, 0xe1]));

        let commands: Vec<_> = std::iter::from_fn(|| input_rx.try_recv().ok()).collect();
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, InputCmd::KeyboardUp(0x04)))
        );
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, InputCmd::KeyboardUp(0xe1)))
        );
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
    async fn device_queries_require_an_active_session() {
        let (state, _input_rx) = test_state();
        state.input.set(None);

        assert!(matches!(
            device_details(State(state.clone())).await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
        assert!(matches!(
            device_screenshot(State(state.clone())).await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
        assert!(matches!(
            device_apps(State(state.clone())).await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
        assert!(matches!(
            device_app_icon(State(state.clone()), Path("com.example.game".into())).await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
        assert!(matches!(
            app_documents(
                State(state.clone()),
                Path("com.example.game".into()),
                Query(AppDocumentQuery { path: "/".into() }),
            )
            .await,
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
            device_crash_reports(State(state.clone())).await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
        assert!(matches!(
            install_app(
                State(state.clone()),
                Json(InstallAppRequest {
                    path: PathBuf::from("Example.ipa"),
                }),
            )
            .await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
        assert!(matches!(
            uninstall_app(State(state), Path("com.example.app".into())).await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
    }

    #[tokio::test]
    async fn app_launch_rejects_invalid_bundle_identifiers_before_dispatch() {
        let (state, mut input_rx) = test_state();

        for bundle_id in ["", "no-domain", "com.example.bad value", "com/example/app"] {
            assert!(matches!(
                launch_app(State(state.clone()), Path(bundle_id.into())).await,
                Err((StatusCode::BAD_REQUEST, _))
            ));
        }
        assert!(input_rx.try_recv().is_err());
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
    async fn app_document_endpoints_dispatch_typed_commands() {
        use crate::app_documents::{
            AppDocumentCommand, AppDocumentEntry, AppDocumentKind, AppDocumentList,
        };

        let (state, mut input_rx) = test_state();
        let list = tokio::spawn(app_documents(
            State(state.clone()),
            Path("com.example.game".into()),
            Query(AppDocumentQuery {
                path: "/Saves".into(),
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::AppDocuments(AppDocumentCommand::List {
                bundle_id,
                path,
                reply,
            }) => {
                assert_eq!(bundle_id, "com.example.game");
                assert_eq!(path, "/Saves");
                reply
                    .send(Ok(AppDocumentList {
                        path,
                        entries: Vec::new(),
                        truncated: false,
                    }))
                    .unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(list.await.unwrap().unwrap().0.path, "/Saves");

        let upload = tokio::spawn(import_app_document(
            State(state.clone()),
            Path("com.example.game".into()),
            Json(ImportAppDocumentRequest {
                directory: "/Saves".into(),
                source: PathBuf::from("slot.dat"),
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::AppDocuments(AppDocumentCommand::Import {
                directory,
                source,
                reply,
                ..
            }) => {
                assert_eq!(directory, "/Saves");
                assert_eq!(source, PathBuf::from("slot.dat"));
                reply
                    .send(Ok(AppDocumentEntry {
                        name: "slot.dat".into(),
                        path: "/Saves/slot.dat".into(),
                        kind: AppDocumentKind::File,
                        size_bytes: 42,
                        modified: "2026-07-24T00:00:00Z".into(),
                    }))
                    .unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(upload.await.unwrap().unwrap().0.size_bytes, 42);

        let create = tokio::spawn(create_app_document_directory(
            State(state.clone()),
            Path("com.example.game".into()),
            Json(CreateAppDocumentDirectoryRequest {
                directory: "/".into(),
                name: "Saves".into(),
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::AppDocuments(AppDocumentCommand::CreateDirectory { name, reply, .. }) => {
                assert_eq!(name, "Saves");
                reply.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(create.await.unwrap().unwrap(), StatusCode::NO_CONTENT);

        let rename = tokio::spawn(rename_app_document(
            State(state.clone()),
            Path("com.example.game".into()),
            Json(RenameAppDocumentRequest {
                path: "/Saves/slot.dat".into(),
                name: "slot-2.dat".into(),
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::AppDocuments(AppDocumentCommand::Rename { name, reply, .. }) => {
                assert_eq!(name, "slot-2.dat");
                reply.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(rename.await.unwrap().unwrap(), StatusCode::NO_CONTENT);

        let delete = tokio::spawn(delete_app_document(
            State(state.clone()),
            Path("com.example.game".into()),
            Query(AppDocumentQuery {
                path: "/Saves/slot-2.dat".into(),
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::AppDocuments(AppDocumentCommand::Delete { path, reply, .. }) => {
                assert_eq!(path, "/Saves/slot-2.dat");
                reply.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(delete.await.unwrap().unwrap(), StatusCode::NO_CONTENT);

        let export = tokio::spawn(export_app_document(
            State(state),
            Path("com.example.game".into()),
            Json(ExportAppDocumentRequest {
                path: "/Saves/slot-2.dat".into(),
                destination: PathBuf::from("slot-2.dat"),
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::AppDocuments(AppDocumentCommand::Export {
                destination, reply, ..
            }) => {
                assert_eq!(destination, PathBuf::from("slot-2.dat"));
                reply.send(Ok(84)).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(export.await.unwrap().unwrap().0["bytes_written"], 84);
    }

    #[tokio::test]
    async fn app_document_conflicts_are_reported_as_http_conflicts() {
        let (reply, response) = oneshot::channel::<Result<(), String>>();
        reply
            .send(Err(
                "an application document with this name already exists".into()
            ))
            .unwrap();
        assert!(matches!(
            await_app_document_response(response, "upload").await,
            Err((StatusCode::CONFLICT, _))
        ));
    }

    #[tokio::test]
    async fn device_power_endpoints_dispatch_only_fixed_commands() {
        let (state, mut input_rx) = test_state();
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
    async fn app_stop_rejects_invalid_bundle_identifiers_before_dispatch() {
        let (state, mut input_rx) = test_state();

        for bundle_id in ["", "no-domain", "com.example.bad value", "com/example/app"] {
            assert!(matches!(
                stop_app(State(state.clone()), Path(bundle_id.into())).await,
                Err((StatusCode::BAD_REQUEST, _))
            ));
        }
        assert!(input_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn app_stop_dispatches_only_a_validated_bundle_identifier() {
        let (state, mut input_rx) = test_state();
        let request = tokio::spawn(stop_app(State(state), Path("com.example.game".into())));

        match input_rx.recv().await.unwrap() {
            InputCmd::StopApp { bundle_id, reply } => {
                assert_eq!(bundle_id, "com.example.game");
                reply.send(Ok(true)).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        let Json(result) = request.await.unwrap().unwrap();
        assert_eq!(result, serde_json::json!({ "was_running": true }));
    }

    #[tokio::test]
    async fn crash_report_list_and_export_use_the_device_session() {
        let (state, mut input_rx) = test_state();
        let list_request = tokio::spawn(device_crash_reports(State(state.clone())));
        match input_rx.recv().await.unwrap() {
            InputCmd::ListCrashReports(reply) => {
                reply
                    .send(Ok(crate::protocol::DeviceCrashReportList {
                        reports: vec![crate::protocol::DeviceCrashReport {
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
            State(state),
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
    }

    #[tokio::test]
    async fn app_uninstall_rejects_invalid_bundle_identifiers_before_dispatch() {
        let (state, mut input_rx) = test_state();

        for bundle_id in ["", "no-domain", "com.example.bad value", "com/example/app"] {
            assert!(matches!(
                uninstall_app(State(state.clone()), Path(bundle_id.into())).await,
                Err((StatusCode::BAD_REQUEST, _))
            ));
        }
        assert!(input_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn app_install_reports_operation_conflicts() {
        let (state, mut input_rx) = test_state();
        let request = install_app(
            State(state),
            Json(InstallAppRequest {
                path: PathBuf::from("Example.ipa"),
            }),
        );
        let respond = async move {
            let command = input_rx.recv().await.unwrap();
            let InputCmd::InstallApp { path, reply } = command else {
                panic!("expected install command");
            };
            assert_eq!(path, PathBuf::from("Example.ipa"));
            let _ = reply.send(Err("another app operation is already running".into()));
        };

        let (result, ()) = tokio::join!(request, respond);
        assert!(matches!(result, Err((StatusCode::CONFLICT, _))));
    }

    #[tokio::test]
    async fn app_operation_endpoint_returns_shared_state() {
        let (state, _input_rx) = test_state();
        let id = state
            .app_operation
            .start(
                crate::protocol::AppOperationKind::Install,
                "Example.ipa".into(),
            )
            .unwrap();
        state.app_operation.update(id, "installing", Some(42));

        let view = app_operation(State(state)).await.0;
        assert_eq!(view.id, id);
        assert_eq!(view.progress, Some(42));
    }

    #[tokio::test]
    async fn profile_management_tracks_active_and_protects_it_from_delete() {
        let (mut state, _input_rx) = test_state();
        let directory = std::env::temp_dir().join(format!(
            "devicehub-mask-profile-test-{}",
            uuid::Uuid::new_v4()
        ));
        state.profile_dir = Arc::new(directory.clone());
        let profile = |name: &str| Profile {
            version: 1,
            name: name.into(),
            mappings: Vec::new(),
            bundle_identifiers: if name == "game" {
                vec!["com.example.game".into()]
            } else {
                Vec::new()
            },
            hardware_bindings: default_hardware_bindings(),
        };

        save_profile(
            State(state.clone()),
            Path("default".into()),
            Json(profile("default")),
        )
        .await
        .unwrap();
        save_profile(
            State(state.clone()),
            Path("game".into()),
            Json(profile("game")),
        )
        .await
        .unwrap();
        activate_profile(State(state.clone()), Path("game".into()))
            .await
            .unwrap();

        let list = list_profiles(State(state.clone())).await.unwrap().0;
        assert_eq!(list.profiles, vec!["default", "game"]);
        assert_eq!(list.active, "game");
        assert_eq!(
            list.app_bindings
                .get("com.example.game")
                .map(String::as_str),
            Some("game")
        );
        assert!(list.binding_conflicts.is_empty());

        let mut duplicate = profile("duplicate");
        duplicate.bundle_identifiers = vec!["com.example.game".into()];
        save_profile(
            State(state.clone()),
            Path("duplicate".into()),
            Json(duplicate),
        )
        .await
        .unwrap();
        let conflicted = list_profiles(State(state.clone())).await.unwrap().0;
        assert!(!conflicted.app_bindings.contains_key("com.example.game"));
        assert_eq!(conflicted.binding_conflicts, vec!["com.example.game"]);
        let _ = delete_profile(State(state.clone()), Path("duplicate".into()))
            .await
            .unwrap();
        assert!(matches!(
            delete_profile(State(state.clone()), Path("game".into())).await,
            Err(StatusCode::CONFLICT)
        ));

        activate_profile(State(state.clone()), Path("default".into()))
            .await
            .unwrap();
        let deleted = delete_profile(State(state.clone()), Path("game".into()))
            .await
            .unwrap();
        assert_eq!(deleted.0["deleted"], "game");
        let _ = tokio::fs::remove_dir_all(directory).await;
    }
}
