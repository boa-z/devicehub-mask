//! Authenticated private API assembled from reusable server adapters.
//!
//! Hosts retain listener, address, TLS, CORS, and token-generation policy.
//! This module owns the stable route graph and token verification semantics.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Query, Request, State, WebSocketUpgrade};
use axum::http::header::{AUTHORIZATION, SEC_WEBSOCKET_PROTOCOL};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use crate::{http, status, websocket};

pub const DEVICE_ID_HEADER: &str = "x-devicehub-device";

#[derive(Clone)]
pub struct PrivateApiState {
    pub application: devicehub_runtime::RuntimeClient<PathBuf>,
    pub device_manager_http: http::DeviceManagerHttpState,
    pub device_http: http::DeviceHttpState,
    pub wda_http: http::WdaHttpState,
    pub developer_image_http: http::DeveloperImageHttpState,
    pub provisioning_http: http::ProvisioningHttpState,
    pub performance_http: http::PerformanceHttpState,
    pub profiles_http: http::ProfileHttpState,
    pub storage_http: http::StorageHttpState,
    pub diagnostics_http: http::DiagnosticsHttpState,
    pub apps_http: http::AppHttpState,
    pub crash_reports_http: http::CrashReportHttpState,
    pub host_http: http::HostHttpState,
    pub websocket_config: websocket::WebSocketConfig,
    pub browser_audio: Option<websocket::BrowserAudioSlot>,
}

#[derive(Clone)]
struct ApiToken(Arc<str>);

pub fn router(state: PrivateApiState, token: String) -> Router {
    let performance_routes = http::performance_router(state.performance_http.clone());
    let device_manager_routes = http::devices_router(state.device_manager_http.clone());
    let device_routes = http::device_router(state.device_http.clone());
    let wda_routes = http::wda_router(state.wda_http.clone());
    let developer_image_routes = http::developer_image_router(state.developer_image_http.clone());
    let provisioning_routes = http::provisioning_router(state.provisioning_http.clone());
    let profile_routes = http::profiles_router(state.profiles_http.clone());
    let storage_routes = http::storage_router(state.storage_http.clone());
    let diagnostics_routes = http::diagnostics_router(state.diagnostics_http.clone());
    let app_routes = http::apps_router(state.apps_http.clone());
    let crash_report_routes = http::crash_reports_router(state.crash_reports_http.clone());
    let host_routes = http::host_router(state.host_http.clone());

    let device_scoped_routes = Router::new()
        .merge(device_routes)
        .merge(wda_routes)
        .merge(developer_image_routes)
        .merge(provisioning_routes)
        .merge(performance_routes)
        .merge(storage_routes)
        .merge(diagnostics_routes)
        .merge(app_routes)
        .merge(crash_report_routes)
        .layer(from_fn_with_state(
            state.application.clone(),
            resolve_device_session,
        ));

    Router::new()
        .route("/api/status", get(api_status))
        .merge(device_manager_routes)
        .merge(device_scoped_routes)
        .merge(profile_routes)
        .merge(host_routes)
        .route("/api/ws", get(ws_upgrade))
        .layer(from_fn_with_state(
            ApiToken(Arc::from(token)),
            authorize_private_api,
        ))
        .with_state(state)
}

async fn resolve_device_session(
    State(application): State<devicehub_runtime::RuntimeClient<PathBuf>>,
    mut request: Request,
    next: Next,
) -> Response {
    let selection_id = match request
        .headers()
        .get(DEVICE_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 256)
    {
        Some(selection_id) => selection_id,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                format!("missing required {DEVICE_ID_HEADER} header"),
            )
                .into_response();
        }
    };
    let Some(session) = application.sessions.get(selection_id) else {
        return (StatusCode::NOT_FOUND, "device session is not available").into_response();
    };
    request.extensions_mut().insert(session);
    next.run(request).await
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

async fn api_status(State(state): State<PrivateApiState>) -> Json<status::StatusView> {
    Json(status::snapshot(&state.application))
}

#[derive(Deserialize)]
struct WebSocketQuery {
    device_id: String,
}

async fn ws_upgrade(
    ws: WebSocketUpgrade,
    Query(query): Query<WebSocketQuery>,
    State(state): State<PrivateApiState>,
) -> Response {
    let selection_id = query.device_id.trim().to_string();
    let Some(session) = state.application.sessions.get(&selection_id) else {
        return (StatusCode::NOT_FOUND, "device session is not available").into_response();
    };
    websocket::upgrade(
        ws,
        websocket::WebSocketState::new(
            state.application,
            selection_id,
            session,
            state.websocket_config,
            state.browser_audio,
        ),
    )
    .await
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::extract::Extension;
    use axum::http::Request;
    use tower::ServiceExt;

    #[derive(Clone, Copy)]
    struct EmptyProfileRepository;

    impl http::ProfileRepository for EmptyProfileRepository {
        fn snapshot(&self) -> http::ProfileRepositoryFuture<http::ProfileRepositorySnapshot> {
            Box::pin(async { Ok(http::ProfileRepositorySnapshot::default()) })
        }

        fn read(&self, _name: String) -> http::ProfileRepositoryFuture<Vec<u8>> {
            Box::pin(async { Err(http::ProfileRepositoryError::NotFound) })
        }

        fn write(&self, _name: String, _bytes: Vec<u8>) -> http::ProfileRepositoryFuture<()> {
            Box::pin(async { Ok(()) })
        }

        fn exists(&self, _name: String) -> http::ProfileRepositoryFuture<bool> {
            Box::pin(async { Ok(false) })
        }

        fn active(&self) -> http::ProfileRepositoryFuture<Option<String>> {
            Box::pin(async { Ok(None) })
        }

        fn set_active(&self, _name: String) -> http::ProfileRepositoryFuture<()> {
            Box::pin(async { Ok(()) })
        }

        fn delete(&self, _name: String) -> http::ProfileRepositoryFuture<()> {
            Box::pin(async { Ok(()) })
        }
    }

    fn test_state() -> PrivateApiState {
        let commands = devicehub_runtime::SessionCommandSlot::default();
        let (application, _control) = devicehub_runtime::RuntimeClientFixture::<PathBuf>::default()
            .with_commands(commands.clone())
            .build();
        PrivateApiState {
            application: application.clone(),
            device_manager_http: http::DeviceManagerHttpState::new(application.manager.clone()),
            device_http: http::DeviceHttpState::new(
                commands.clone(),
                application.device.location.clone(),
                application.device.device_control.clone(),
            ),
            wda_http: http::WdaHttpState::new(commands.clone()),
            developer_image_http: http::DeveloperImageHttpState::new(
                commands.clone(),
                application.device.developer_image.clone(),
            ),
            provisioning_http: http::ProvisioningHttpState::new(commands.clone()),
            performance_http: http::PerformanceHttpState::new(
                application.device.performance.clone(),
                application.device.performance_demand.clone(),
                application.device.device_logs.clone(),
                application.device.device_log_demand.clone(),
                application.device.device_conditions.clone(),
                application.device.network_capture.clone(),
                application.device.bluetooth_capture.clone(),
                application.device.service_registry.clone(),
                commands.clone(),
                http::CaptureDestinationValidator::new(|_, _| async { Ok(()) }),
            ),
            profiles_http: http::ProfileHttpState::new(EmptyProfileRepository),
            storage_http: http::StorageHttpState::new(
                commands.clone(),
                application.device.app_documents.clone(),
                application.device.device_files.clone(),
            ),
            diagnostics_http: http::DiagnosticsHttpState::new(
                commands.clone(),
                application.device.device_backup.clone(),
                application.device.sysdiagnose.clone(),
                application.device.log_archive.clone(),
                http::DiagnosticDestinationPreparer::new(|destination, _| async move {
                    Ok(destination)
                }),
            ),
            apps_http: http::AppHttpState::new(
                commands.clone(),
                application.device.app_operation.clone(),
            ),
            crash_reports_http: http::CrashReportHttpState::new(commands),
            host_http: http::HostHttpState::unavailable(),
            websocket_config: websocket::WebSocketConfig::default(),
            browser_audio: None,
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
    async fn complete_route_graph_applies_shared_authentication() {
        let unauthorized = router(test_state(), "secret".into())
            .oneshot(
                Request::builder()
                    .uri("/api/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        for (header, value) in [
            (AUTHORIZATION, "Bearer secret"),
            (SEC_WEBSOCKET_PROTOCOL, "devicehub-mask, secret"),
        ] {
            let authorized = router(test_state(), "secret".into())
                .oneshot(
                    Request::builder()
                        .uri("/api/status")
                        .header(header, value)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(authorized.status(), StatusCode::OK);
        }
    }

    #[tokio::test]
    async fn device_scope_resolves_exact_session_without_using_legacy_state() {
        let (phone, _) = devicehub_runtime::RuntimeClientFixture::<PathBuf>::default().build();
        phone.device.status.set("phone");
        let (tablet, _) = devicehub_runtime::RuntimeClientFixture::<PathBuf>::default().build();
        tablet.device.status.set("tablet");
        let (application, _) = devicehub_runtime::RuntimeClientFixture::<PathBuf>::default()
            .with_session("phone::usb", phone.device)
            .with_session("tablet::usb", tablet.device)
            .build();

        async fn session_status(
            Extension(session): Extension<devicehub_runtime::DeviceSessionClient<PathBuf>>,
        ) -> String {
            session.status.get()
        }

        let scoped = Router::new()
            .route("/", get(session_status))
            .layer(from_fn_with_state(application, resolve_device_session));
        for (selection_id, expected) in [("phone::usb", "phone"), ("tablet::usb", "tablet")] {
            let response = scoped
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/")
                        .header(DEVICE_ID_HEADER, selection_id)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = axum::body::to_bytes(response.into_body(), 32)
                .await
                .unwrap();
            assert_eq!(body, expected);
        }

        let missing = scoped
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::BAD_REQUEST);
    }
}
