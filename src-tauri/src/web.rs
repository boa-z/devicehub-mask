use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, Request, State, WebSocketUpgrade};
use axum::http::header::{AUTHORIZATION, SEC_WEBSOCKET_PROTOCOL};
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
    ActiveSlot, AppOperationSlot, ControlCmd, DeviceListSlot, ErrorSlot, Frame, FrameSlot,
    InputCmd, InputSink, Orientation, OrientationSlot, RotateDir, StatusSlot, norm, unrotate_norm,
};

#[derive(Clone)]
pub struct AppState {
    pub frames: FrameSlot,
    pub status: StatusSlot,
    pub orientation: OrientationSlot,
    pub devices: DeviceListSlot,
    pub active: ActiveSlot,
    pub error: ErrorSlot,
    pub app_operation: AppOperationSlot,
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
}

#[derive(Serialize)]
struct StreamMetricsView {
    decoded_fps: f64,
    sent_fps: f64,
    jpeg_encode_ms: f64,
    megabits_per_second: f64,
}

#[derive(Serialize, Deserialize)]
struct Profile {
    version: u8,
    name: String,
    mappings: Vec<serde_json::Value>,
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
}

#[derive(Clone)]
struct ApiToken(Arc<str>);

pub fn router(state: AppState, token: String) -> Router {
    Router::new()
        .route("/api/status", get(status))
        .route("/api/devices/refresh", put(refresh_devices))
        .route("/api/devices/{udid}/connect", put(connect_device))
        .route("/api/device/details", get(device_details))
        .route("/api/device/apps", get(device_apps))
        .route("/api/device/apps/operation", get(app_operation))
        .route("/api/device/apps/install", put(install_app))
        .route("/api/device/apps/{bundle_id}", delete(uninstall_app))
        .route("/api/device/apps/{bundle_id}/launch", put(launch_app))
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

const DEVICE_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

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

fn valid_bundle_identifier(bundle_id: &str) -> bool {
    !bundle_id.is_empty()
        && bundle_id.len() <= 255
        && bundle_id.contains('.')
        && bundle_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
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
    Ok(Json(ProfileList { profiles, active }))
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
        || HARDWARE_BUTTON_NAMES
            .iter()
            .any(|name| !profile.hardware_bindings.contains_key(*name))
    {
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
    MultiTouch { contacts: Vec<WebContact> },
    Button { name: String },
    ButtonDown { name: String },
    ButtonUp { name: String },
    KeyboardDown { usage: u64 },
    KeyboardUp { usage: u64 },
    Rotate { direction: RotateRequest },
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
    let send_task = tokio::spawn(async move {
        let mut last_frame_version = 0;
        let mut last_status = String::new();
        // CoreDevice can deliver 60 FPS. Poll the latest-frame slot at that rate;
        // lagging clients still drop intermediate versions instead of queueing.
        let frame_period = Duration::from_micros(16_667);
        let mut next_frame_at = tokio::time::Instant::now();
        let mut status_tick = tokio::time::interval(Duration::from_millis(250));
        status_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut metrics_tick = tokio::time::interval(Duration::from_secs(1));
        metrics_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut metrics_started = Instant::now();
        let mut metrics_frame_version = send_state.frames.version();
        let mut sent_frames = 0_u64;
        let mut sent_bytes = 0_u64;
        let mut encoded_frames = 0_u64;
        let mut encoding_time = Duration::ZERO;
        loop {
            let frame_sleep = tokio::time::sleep_until(next_frame_at);
            tokio::pin!(frame_sleep);
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
                _ = &mut frame_sleep => {
                    next_frame_at = tokio::time::Instant::now() + frame_period;
                    let Some((version, frame)) = send_state.frames.latest() else {
                        continue;
                    };
                    if version == last_frame_version {
                        continue;
                    }
                    last_frame_version = version;
                    let cached = frame.jpeg.get().is_some();
                    let encode_started = Instant::now();
                    let encoded = tokio::task::spawn_blocking(move || encode_jpeg(&frame)).await;
                    let Ok(Ok(jpeg)) = encoded else {
                        continue;
                    };
                    if !cached {
                        encoded_frames += 1;
                        encoding_time += encode_started.elapsed();
                    }
                    sent_frames += 1;
                    sent_bytes += jpeg.len() as u64;
                    if sender.send(Message::Binary(jpeg)).await.is_err() {
                        break;
                    }
                }
                _ = metrics_tick.tick() => {
                    let elapsed = metrics_started.elapsed().as_secs_f64().max(f64::EPSILON);
                    let version = send_state.frames.version();
                    let metrics = StreamMetricsView {
                        decoded_fps: version.wrapping_sub(metrics_frame_version) as f64 / elapsed,
                        sent_fps: sent_frames as f64 / elapsed,
                        jpeg_encode_ms: if encoded_frames == 0 {
                            0.0
                        } else {
                            encoding_time.as_secs_f64() * 1000.0 / encoded_frames as f64
                        },
                        megabits_per_second: sent_bytes as f64 * 8.0 / elapsed / 1_000_000.0,
                    };
                    let Ok(text) = serde_json::to_string(
                        &json!({"type": "metrics", "payload": metrics}),
                    ) else {
                        continue;
                    };
                    if sender.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                    metrics_started = Instant::now();
                    metrics_frame_version = version;
                    sent_frames = 0;
                    sent_bytes = 0;
                    encoded_frames = 0;
                    encoding_time = Duration::ZERO;
                }
            }
        }
    });

    let mut pressed_keyboard = HashSet::new();
    while let Some(Ok(message)) = receiver.next().await {
        match message {
            Message::Text(text) => handle_client_message(&state, &text, &mut pressed_keyboard),
            Message::Close(_) => break,
            _ => {}
        }
    }
    send_task.abort();
    send_all_up(&state, &pressed_keyboard);
}

fn encode_jpeg(frame: &Frame) -> Result<bytes::Bytes, String> {
    frame
        .jpeg
        .get_or_init(|| {
            let image = turbojpeg::Image {
                pixels: frame.rgba.as_slice(),
                width: frame.width,
                pitch: frame.width * 4,
                height: frame.height,
                format: turbojpeg::PixelFormat::RGBA,
            };
            let encoded = turbojpeg::compress(image, 80, turbojpeg::Subsamp::Sub2x2)
                .map_err(|error| error.to_string())?;
            Ok(bytes::Bytes::copy_from_slice(&encoded))
        })
        .clone()
}

fn handle_client_message(state: &AppState, text: &str, pressed_keyboard: &mut HashSet<u64>) {
    let Ok(message) = serde_json::from_str::<ClientMessage>(text) else {
        return;
    };
    match message {
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
        ClientMessage::Rotate { direction } => {
            state.input.send(InputCmd::Rotate(match direction {
                RotateRequest::Left => RotateDir::Left,
                RotateRequest::Right => RotateDir::Right,
            }))
        }
    }
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
        let input = InputSink::default();
        let (input_tx, input_rx) = unbounded_channel();
        input.set(Some(input_tx));
        let (control, _control_rx) = unbounded_channel();
        (
            AppState {
                frames: FrameSlot::default(),
                status: StatusSlot::default(),
                orientation: OrientationSlot::default(),
                devices: DeviceListSlot::default(),
                active: ActiveSlot::default(),
                error: ErrorSlot::default(),
                app_operation: AppOperationSlot::default(),
                input,
                control,
                profile_dir: Arc::new(PathBuf::new()),
            },
            input_rx,
        )
    }

    fn test_frame() -> Frame {
        Frame {
            width: 2,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 255, 0, 255],
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
    fn frame_slot_version_advances_on_publish() {
        let slot = FrameSlot::default();
        assert_eq!(slot.version(), 0);
        slot.publish(Arc::new(test_frame()));
        assert_eq!(slot.version(), 1);
        slot.publish(Arc::new(test_frame()));
        assert_eq!(slot.version(), 2);
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
            device_apps(State(state.clone())).await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
        assert!(matches!(
            device_provisioning_profiles(State(state.clone())).await,
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
