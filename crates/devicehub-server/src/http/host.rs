//! Host-scoped HTTP capabilities for browser clients.
//!
//! Device operations remain in the runtime-owned route modules. This adapter
//! exposes only bounded product metadata, host preferences, and frontend logs.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

const MAX_FRONTEND_LOG_FIELD_BYTES: usize = 2_048;

#[derive(Clone, Debug, Default, Serialize)]
pub struct HostCapabilities {
    pub always_on_top: bool,
    pub system_fullscreen: bool,
    pub native_file_dialogs: bool,
    pub browser_file_transfer: bool,
    pub device_audio: bool,
    pub clipboard_sync: bool,
    pub app_updates: bool,
    pub open_host_directories: bool,
    pub mutable_debug_logging: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostBuildInfo {
    pub version: String,
    pub build: String,
    pub commit: String,
    pub update_channel: String,
    pub host: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct HostSettingsStatus {
    pub audio_enabled: bool,
    pub audio_muted: bool,
    pub audio_volume: f32,
    pub clipboard_sync_enabled: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct HostSettingsPatch {
    pub audio_enabled: Option<bool>,
    pub audio_muted: Option<bool>,
    pub audio_volume: Option<f32>,
    pub clipboard_sync_enabled: Option<bool>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct HostDiagnosticsStatus {
    pub debug_enabled: bool,
    pub custom_filter: bool,
    pub filter: String,
    pub log_directory: String,
    pub file_logging: bool,
    pub run_id: String,
    pub dropped_log_lines: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FrontendLogEvent {
    level: String,
    component: String,
    operation: String,
    message: String,
}

pub trait HostControl: Send + Sync + 'static {
    fn settings(&self) -> HostSettingsStatus;
    fn update_settings(&self, patch: HostSettingsPatch) -> Result<HostSettingsStatus, String>;
    fn diagnostics(&self) -> HostDiagnosticsStatus;
    fn set_debug_logging(&self, _enabled: bool) -> Result<HostDiagnosticsStatus, String> {
        Err("debug log filtering is configured by the headless process environment".into())
    }
}

#[derive(Clone)]
pub struct HostHttpState {
    pub capabilities: HostCapabilities,
    pub build: HostBuildInfo,
    control: Arc<dyn HostControl>,
}

impl HostHttpState {
    pub fn new(
        capabilities: HostCapabilities,
        build: HostBuildInfo,
        control: impl HostControl,
    ) -> Self {
        Self {
            capabilities,
            build,
            control: Arc::new(control),
        }
    }

    pub fn unavailable() -> Self {
        Self::new(
            HostCapabilities::default(),
            HostBuildInfo {
                version: "unknown".into(),
                build: "unknown".into(),
                commit: "unknown".into(),
                update_channel: "nightly".into(),
                host: "unknown".into(),
            },
            UnavailableHostControl,
        )
    }
}

#[derive(Clone, Copy)]
struct UnavailableHostControl;

impl HostControl for UnavailableHostControl {
    fn settings(&self) -> HostSettingsStatus {
        HostSettingsStatus::default()
    }

    fn update_settings(&self, _patch: HostSettingsPatch) -> Result<HostSettingsStatus, String> {
        Err("host settings are unavailable".into())
    }

    fn diagnostics(&self) -> HostDiagnosticsStatus {
        HostDiagnosticsStatus::default()
    }
}

#[derive(Serialize)]
struct HostView {
    capabilities: HostCapabilities,
    build: HostBuildInfo,
}

pub fn router<S>(state: HostHttpState) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/host", get(host_status))
        .route("/api/host/settings", get(settings).put(update_settings))
        .route(
            "/api/host/diagnostics",
            get(diagnostics).put(set_debug_logging),
        )
        .route("/api/host/frontend-log", post(frontend_log))
        .with_state(state)
}

async fn host_status(State(state): State<HostHttpState>) -> Json<HostView> {
    Json(HostView {
        capabilities: state.capabilities,
        build: state.build,
    })
}

async fn settings(State(state): State<HostHttpState>) -> Json<HostSettingsStatus> {
    Json(state.control.settings())
}

async fn update_settings(
    State(state): State<HostHttpState>,
    Json(patch): Json<HostSettingsPatch>,
) -> Result<Json<HostSettingsStatus>, (StatusCode, String)> {
    if patch
        .audio_volume
        .is_some_and(|volume| !volume.is_finite() || !(0.0..=1.0).contains(&volume))
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "audio volume must be a finite value between 0 and 1".into(),
        ));
    }
    state
        .control
        .update_settings(patch)
        .map(Json)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}

async fn diagnostics(State(state): State<HostHttpState>) -> Json<HostDiagnosticsStatus> {
    Json(state.control.diagnostics())
}

#[derive(Deserialize)]
struct DebugLoggingPatch {
    enabled: bool,
}

async fn set_debug_logging(
    State(state): State<HostHttpState>,
    Json(patch): Json<DebugLoggingPatch>,
) -> Result<Json<HostDiagnosticsStatus>, (StatusCode, String)> {
    state
        .control
        .set_debug_logging(patch.enabled)
        .map(Json)
        .map_err(|error| (StatusCode::NOT_IMPLEMENTED, error))
}

async fn frontend_log(
    Json(event): Json<FrontendLogEvent>,
) -> Result<StatusCode, (StatusCode, String)> {
    let fields = [
        &event.level,
        &event.component,
        &event.operation,
        &event.message,
    ];
    if fields.iter().any(|field| {
        field.is_empty()
            || field.len() > MAX_FRONTEND_LOG_FIELD_BYTES
            || field.contains(['\r', '\n'])
    }) {
        return Err((StatusCode::BAD_REQUEST, "invalid frontend log event".into()));
    }
    match event.level.as_str() {
        "debug" => {
            tracing::debug!(component = %event.component, operation = %event.operation, message = %event.message, "frontend event")
        }
        "info" => {
            tracing::info!(component = %event.component, operation = %event.operation, message = %event.message, "frontend event")
        }
        "warn" => {
            tracing::warn!(component = %event.component, operation = %event.operation, message = %event.message, "frontend event")
        }
        "error" => {
            tracing::error!(component = %event.component, operation = %event.operation, message = %event.message, "frontend event")
        }
        _ => return Err((StatusCode::BAD_REQUEST, "invalid frontend log level".into())),
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn frontend_logs_are_bounded_and_single_line() {
        let app = router::<()>(HostHttpState::unavailable());
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/host/frontend-log")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"level":"info","component":"web","operation":"ready","message":"ok"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let response = app
            .oneshot(
                Request::post("/api/host/frontend-log")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"level":"info","component":"web","operation":"ready","message":"bad\nline"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
