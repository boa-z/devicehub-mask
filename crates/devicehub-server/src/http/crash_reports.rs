//! HTTP adapter for bounded crash-report discovery and export.
//!
//! Device services and transfer work remain owned by the active session. This
//! module owns only request validation, deadlines, and HTTP response mapping.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Extension, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::Deserialize;
use tokio::sync::oneshot;

use super::browser_transfers::{BrowserTransferStore, binary_download, validate_file_name};

type InputCmd = devicehub_runtime::DeviceSessionCommand<PathBuf>;
type InputSink = devicehub_runtime::SessionCommandSlot<PathBuf>;
type RequestSession = Option<Extension<devicehub_runtime::DeviceSessionClient<PathBuf>>>;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Default)]
pub struct CrashReportHttpState {
    input: InputSink,
    browser_transfers: Option<Arc<dyn BrowserTransferStore>>,
}

impl CrashReportHttpState {
    pub fn new(input: InputSink) -> Self {
        Self {
            input,
            browser_transfers: None,
        }
    }

    pub fn with_browser_transfers(mut self, store: impl BrowserTransferStore) -> Self {
        self.browser_transfers = Some(Arc::new(store));
        self
    }

    fn input(&self, session: &RequestSession) -> InputSink {
        session
            .as_ref()
            .map(|session| session.commands.clone())
            .unwrap_or_else(|| self.input.clone())
    }
}

pub fn router<S>(state: CrashReportHttpState) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route(
            "/api/device/crash-reports",
            get(device_crash_reports).delete(delete_crash_report),
        )
        .route(
            "/api/device/crash-reports/summary",
            get(crash_report_summary),
        )
        .route("/api/device/crash-reports/export", put(export_crash_report))
        .route(
            "/api/device/crash-reports/browser-export",
            get(browser_export_crash_report),
        )
        .with_state(state)
}

async fn device_crash_reports(
    State(state): State<CrashReportHttpState>,
    session: RequestSession,
) -> Result<Json<devicehub_core::DeviceCrashReportList>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    require_active_session(
        state
            .input(&session)
            .try_send(InputCmd::ListCrashReports(reply)),
    )?;
    let reports = await_session_result(response, "crash report list request").await?;
    Ok(Json(reports))
}

#[derive(Deserialize)]
struct CrashReportSummaryQuery {
    device_path: String,
}

async fn crash_report_summary(
    State(state): State<CrashReportHttpState>,
    session: RequestSession,
    Query(query): Query<CrashReportSummaryQuery>,
) -> Result<Json<devicehub_core::DeviceCrashReportSummary>, (StatusCode, String)> {
    devicehub_core::validate_crash_report_path(&query.device_path)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    let (reply, response) = oneshot::channel();
    require_active_session(state.input(&session).try_send(InputCmd::ReadCrashReport {
        device_path: query.device_path,
        max_bytes: devicehub_runtime::MAX_CRASH_REPORT_READ_BYTES,
        reply,
    }))?;
    let report = await_session_result(response, "crash report summary request").await?;
    Ok(Json(report.summary))
}

#[derive(Deserialize)]
struct ExportCrashReportRequest {
    device_path: String,
    destination: PathBuf,
}

async fn export_crash_report(
    State(state): State<CrashReportHttpState>,
    session: RequestSession,
    Json(request): Json<ExportCrashReportRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    require_active_session(state.input(&session).try_send(InputCmd::ExportCrashReport {
        device_path: request.device_path,
        destination: request.destination,
        reply,
    }))?;
    let bytes_written = await_session_result(response, "crash report export").await?;
    Ok(Json(serde_json::json!({ "bytes_written": bytes_written })))
}

#[derive(Deserialize)]
struct BrowserExportCrashReportQuery {
    device_path: String,
    name: String,
}

async fn browser_export_crash_report(
    State(state): State<CrashReportHttpState>,
    session: RequestSession,
    Query(query): Query<BrowserExportCrashReportQuery>,
) -> Result<Response, (StatusCode, String)> {
    devicehub_core::validate_crash_report_path(&query.device_path)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    validate_file_name(&query.name).map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    let store = state.browser_transfers.clone().ok_or((
        StatusCode::NOT_IMPLEMENTED,
        "browser file transfer is unavailable in this host".into(),
    ))?;
    let destination = store
        .prepare_download(query.name)
        .await
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;
    let cleanup = destination.clone();
    let (reply, response) = oneshot::channel();
    if let Err(error) =
        require_active_session(state.input(&session).try_send(InputCmd::ExportCrashReport {
            device_path: query.device_path,
            destination,
            reply,
        }))
    {
        let _ = store.remove(cleanup).await;
        return Err(error);
    }
    if let Err(error) = await_session_result(response, "browser crash report export").await {
        let _ = store.remove(cleanup).await;
        return Err(error);
    }
    let bytes = store
        .read_and_remove(cleanup)
        .await
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;
    Ok(binary_download(bytes))
}

#[derive(Deserialize)]
struct DeleteCrashReportRequest {
    device_path: String,
}

async fn delete_crash_report(
    State(state): State<CrashReportHttpState>,
    session: RequestSession,
    Json(request): Json<DeleteCrashReportRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    require_active_session(state.input(&session).try_send(InputCmd::DeleteCrashReport {
        device_path: request.device_path,
        reply,
    }))?;
    await_session_result(response, "crash report delete").await?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn await_session_result<T>(
    response: oneshot::Receiver<Result<T, String>>,
    operation: &str,
) -> Result<T, (StatusCode, String)> {
    tokio::time::timeout(REQUEST_TIMEOUT, response)
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
        .map_err(|error| (StatusCode::BAD_GATEWAY, error))
}

fn require_active_session(sent: bool) -> Result<(), (StatusCode, String)> {
    sent.then_some(()).ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "no active device session".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Bytes, to_bytes};
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    #[derive(Clone)]
    struct TestBrowserTransferStore {
        removed: Arc<AtomicBool>,
    }

    impl BrowserTransferStore for TestBrowserTransferStore {
        fn stage_upload(
            &self,
            _name: String,
            _bytes: Bytes,
        ) -> crate::http::BrowserTransferFuture<PathBuf> {
            unreachable!("crash reports never stage browser uploads")
        }

        fn prepare_download(&self, name: String) -> crate::http::BrowserTransferFuture<PathBuf> {
            Box::pin(async move { Ok(PathBuf::from("staging").join(name)) })
        }

        fn read_and_remove(&self, path: PathBuf) -> crate::http::BrowserTransferFuture<Bytes> {
            let removed = self.removed.clone();
            Box::pin(async move {
                assert_eq!(path, PathBuf::from("staging/Report.ips"));
                removed.store(true, Ordering::Relaxed);
                Ok(Bytes::from_static(b"report"))
            })
        }

        fn remove(&self, _path: PathBuf) -> crate::http::BrowserTransferFuture<()> {
            let removed = self.removed.clone();
            Box::pin(async move {
                removed.store(true, Ordering::Relaxed);
                Ok(())
            })
        }
    }

    fn test_state() -> (CrashReportHttpState, UnboundedReceiver<InputCmd>) {
        let input = InputSink::default();
        let (input_tx, input_rx) = unbounded_channel();
        input.set(Some(input_tx));
        (CrashReportHttpState::new(input), input_rx)
    }

    #[tokio::test]
    async fn list_export_and_delete_use_the_device_session() {
        let (state, mut input_rx) = test_state();
        let list_request = tokio::spawn(device_crash_reports(State(state.clone()), None));
        match input_rx.recv().await.unwrap() {
            InputCmd::ListCrashReports(reply) => reply
                .send(Ok(devicehub_core::DeviceCrashReportList {
                    reports: vec![devicehub_core::DeviceCrashReport {
                        path: "/Report.ips".into(),
                        name: "Report.ips".into(),
                        size_bytes: 42,
                        modified: "2026-07-24T00:00:00Z".into(),
                    }],
                    truncated: false,
                }))
                .unwrap(),
            _ => panic!("unexpected command"),
        }
        let Json(list) = list_request.await.unwrap().unwrap();
        assert_eq!(list.reports.len(), 1);
        assert!(!list.truncated);

        let export_request = tokio::spawn(export_crash_report(
            State(state.clone()),
            None,
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
        assert_eq!(
            export_request.await.unwrap().unwrap().0,
            serde_json::json!({ "bytes_written": 42 })
        );

        let delete_request = tokio::spawn(delete_crash_report(
            State(state),
            None,
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
        assert_eq!(
            delete_request.await.unwrap().unwrap().0,
            serde_json::json!({ "deleted": true })
        );
    }

    #[tokio::test]
    async fn browser_export_returns_the_staged_report_and_removes_it() {
        let (state, mut input_rx) = test_state();
        let removed = Arc::new(AtomicBool::new(false));
        let state = state.with_browser_transfers(TestBrowserTransferStore {
            removed: removed.clone(),
        });
        let export = tokio::spawn(browser_export_crash_report(
            State(state),
            None,
            Query(BrowserExportCrashReportQuery {
                device_path: "/Report.ips".into(),
                name: "Report.ips".into(),
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::ExportCrashReport {
                device_path,
                destination,
                reply,
            } => {
                assert_eq!(device_path, "/Report.ips");
                assert_eq!(destination, PathBuf::from("staging/Report.ips"));
                reply.send(Ok(6)).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        let response = export.await.unwrap().unwrap();
        assert_eq!(
            to_bytes(response.into_body(), 16).await.unwrap(),
            Bytes::from_static(b"report")
        );
        assert!(removed.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn summary_is_validated_bounded_and_summary_only() {
        let (state, mut input_rx) = test_state();
        let invalid = crash_report_summary(
            State(state.clone()),
            None,
            Query(CrashReportSummaryQuery {
                device_path: "/../private/report.ips".into(),
            }),
        )
        .await;
        assert!(matches!(invalid, Err((StatusCode::BAD_REQUEST, _))));
        assert!(input_rx.try_recv().is_err());

        let request = tokio::spawn(crash_report_summary(
            State(state),
            None,
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

    #[tokio::test]
    async fn routes_require_an_active_session() {
        let (state, _input_rx) = test_state();
        state.input.set(None);
        assert!(matches!(
            device_crash_reports(State(state), None).await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
    }
}
