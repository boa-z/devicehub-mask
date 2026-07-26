//! HTTP adapter for bounded crash-report discovery and export.
//!
//! Device services and transfer work remain owned by the active session. This
//! module owns only request validation, deadlines, and HTTP response mapping.

use std::path::PathBuf;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::Deserialize;
use tokio::sync::oneshot;

use crate::protocol::{InputCmd, InputSink};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Default)]
pub(crate) struct CrashReportHttpState {
    input: InputSink,
}

impl CrashReportHttpState {
    pub(crate) fn new(input: InputSink) -> Self {
        Self { input }
    }
}

pub(crate) fn router<S>(state: CrashReportHttpState) -> Router<S>
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
        .with_state(state)
}

pub(crate) async fn device_crash_reports(
    State(state): State<CrashReportHttpState>,
) -> Result<Json<crate::protocol::DeviceCrashReportList>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    require_active_session(state.input.try_send(InputCmd::ListCrashReports(reply)))?;
    let reports = await_session_result(response, "crash report list request").await?;
    Ok(Json(reports))
}

#[derive(Deserialize)]
pub(crate) struct CrashReportSummaryQuery {
    pub(crate) device_path: String,
}

pub(crate) async fn crash_report_summary(
    State(state): State<CrashReportHttpState>,
    Query(query): Query<CrashReportSummaryQuery>,
) -> Result<Json<crate::protocol::DeviceCrashReportSummary>, (StatusCode, String)> {
    devicehub_runtime::validate_crash_report_path(&query.device_path)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    let (reply, response) = oneshot::channel();
    require_active_session(state.input.try_send(InputCmd::ReadCrashReport {
        device_path: query.device_path,
        max_bytes: devicehub_runtime::MAX_CRASH_REPORT_READ_BYTES,
        reply,
    }))?;
    let report = await_session_result(response, "crash report summary request").await?;
    Ok(Json(report.summary))
}

#[derive(Deserialize)]
pub(crate) struct ExportCrashReportRequest {
    pub(crate) device_path: String,
    pub(crate) destination: PathBuf,
}

pub(crate) async fn export_crash_report(
    State(state): State<CrashReportHttpState>,
    Json(request): Json<ExportCrashReportRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    require_active_session(state.input.try_send(InputCmd::ExportCrashReport {
        device_path: request.device_path,
        destination: request.destination,
        reply,
    }))?;
    let bytes_written = await_session_result(response, "crash report export").await?;
    Ok(Json(serde_json::json!({ "bytes_written": bytes_written })))
}

#[derive(Deserialize)]
pub(crate) struct DeleteCrashReportRequest {
    pub(crate) device_path: String,
}

pub(crate) async fn delete_crash_report(
    State(state): State<CrashReportHttpState>,
    Json(request): Json<DeleteCrashReportRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    require_active_session(state.input.try_send(InputCmd::DeleteCrashReport {
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
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    fn test_state() -> (CrashReportHttpState, UnboundedReceiver<InputCmd>) {
        let input = InputSink::default();
        let (input_tx, input_rx) = unbounded_channel();
        input.set(Some(input_tx));
        (CrashReportHttpState::new(input), input_rx)
    }

    #[tokio::test]
    async fn list_export_and_delete_use_the_device_session() {
        let (state, mut input_rx) = test_state();
        let list_request = tokio::spawn(device_crash_reports(State(state.clone())));
        match input_rx.recv().await.unwrap() {
            InputCmd::ListCrashReports(reply) => reply
                .send(Ok(crate::protocol::DeviceCrashReportList {
                    reports: vec![crate::protocol::DeviceCrashReport {
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
    async fn summary_is_validated_bounded_and_summary_only() {
        let (state, mut input_rx) = test_state();
        let invalid = crash_report_summary(
            State(state.clone()),
            Query(CrashReportSummaryQuery {
                device_path: "/../private/report.ips".into(),
            }),
        )
        .await;
        assert!(matches!(invalid, Err((StatusCode::BAD_REQUEST, _))));
        assert!(input_rx.try_recv().is_err());

        let request = tokio::spawn(crash_report_summary(
            State(state),
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
            .send(Ok(crate::protocol::DeviceCrashReportContent {
                device_path,
                size_bytes: 4_096,
                bytes_read: 128,
                truncated: false,
                lossy_utf8: false,
                summary: crate::protocol::DeviceCrashReportSummary {
                    format: crate::protocol::CrashReportFormat::IpsJson,
                    kind: crate::protocol::CrashReportKind::AppCrash,
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
            device_crash_reports(State(state)).await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
    }
}
