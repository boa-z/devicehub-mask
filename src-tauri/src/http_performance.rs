//! HTTP adapter for the device performance workbench.
//!
//! This module owns HTTP validation and response mapping only. Long-lived
//! sampling and capture resources remain owned by the active device session.

use std::path::PathBuf;
use std::time::Duration;

use crate::device_runtime::{InputCmd, InputSink};
use crate::supervisor::ServiceRegistry;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use devicehub_core::{AppActivityEvent, PerformanceSnapshot};
use devicehub_runtime::{PerformanceDemand, PerformanceSlot};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

/// Narrow capability set exposed to performance-workbench HTTP handlers.
/// Cloning it shares existing slots and demand counters; it does not start any
/// sampler, capture, or background task.
#[derive(Clone, Default)]
pub(crate) struct PerformanceHttpState {
    performance: PerformanceSlot,
    performance_demand: PerformanceDemand,
    device_logs: devicehub_runtime::DeviceLogSlot,
    device_log_demand: devicehub_runtime::DeviceLogDemand,
    device_conditions: devicehub_runtime::DeviceConditionSlot,
    network_capture: crate::network_capture::NetworkCaptureSlot,
    bluetooth_capture: crate::bluetooth_capture::BluetoothCaptureSlot,
    services: ServiceRegistry,
    input: InputSink,
}

impl PerformanceHttpState {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        performance: PerformanceSlot,
        performance_demand: PerformanceDemand,
        device_logs: devicehub_runtime::DeviceLogSlot,
        device_log_demand: devicehub_runtime::DeviceLogDemand,
        device_conditions: devicehub_runtime::DeviceConditionSlot,
        network_capture: crate::network_capture::NetworkCaptureSlot,
        bluetooth_capture: crate::bluetooth_capture::BluetoothCaptureSlot,
        services: ServiceRegistry,
        input: InputSink,
    ) -> Self {
        Self {
            performance,
            performance_demand,
            device_logs,
            device_log_demand,
            device_conditions,
            network_capture,
            bluetooth_capture,
            services,
            input,
        }
    }
}

/// Supplies this adapter's state before merging it into the application router,
/// so handlers cannot extract the much broader top-level HTTP state.
pub(crate) fn router<S>(state: PerformanceHttpState) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/performance", get(performance))
        .route("/api/performance/processes", get(running_processes))
        .route(
            "/api/performance/sampling",
            put(start_performance_sampling).delete(stop_performance_sampling),
        )
        .route(
            "/api/performance/network-capture",
            put(start_network_capture).delete(stop_network_capture),
        )
        .route(
            "/api/performance/bluetooth-capture",
            put(start_bluetooth_capture).delete(stop_bluetooth_capture),
        )
        .route(
            "/api/performance/device-condition",
            put(apply_device_condition).delete(clear_device_condition),
        )
        .route(
            "/api/device/logs",
            get(device_logs).delete(clear_device_logs),
        )
        .route(
            "/api/device/logs/streaming",
            put(start_device_logs).delete(stop_device_logs),
        )
        .with_state(state)
}

#[derive(Serialize)]
struct PerformanceView {
    sample: PerformanceSnapshot,
    app_activity: Vec<AppActivityEvent>,
    services: Vec<crate::supervisor::ServiceHealth>,
    sampling: bool,
    network_capture: crate::network_capture::NetworkCaptureStatus,
    bluetooth_capture: crate::bluetooth_capture::BluetoothCaptureStatus,
    device_conditions: devicehub_core::DeviceConditionStatus,
}

async fn performance(State(state): State<PerformanceHttpState>) -> Json<PerformanceView> {
    Json(PerformanceView {
        sample: state.performance.get(),
        app_activity: state.performance.app_activity(),
        services: state.services.snapshot(),
        sampling: state.performance_demand.enabled(),
        network_capture: state.network_capture.get(),
        bluetooth_capture: state.bluetooth_capture.get(),
        device_conditions: state.device_conditions.get(),
    })
}

async fn running_processes(
    State(state): State<PerformanceHttpState>,
) -> Result<Json<devicehub_core::RunningProcessList>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::RunningProcess(
        devicehub_runtime::RunningProcessCommand::List { reply },
    )) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    let processes = tokio::time::timeout(Duration::from_secs(12), response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "running process request timed out".into(),
            )
        })?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device session ended".into(),
            )
        })?
        .map_err(|error| (StatusCode::BAD_GATEWAY, error))?;
    Ok(Json(processes))
}

#[derive(Deserialize)]
struct ApplyDeviceConditionRequest {
    group_identifier: String,
    profile_identifier: String,
}

async fn apply_device_condition(
    State(state): State<PerformanceHttpState>,
    Json(request): Json<ApplyDeviceConditionRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    devicehub_runtime::validate_device_condition_identifiers(
        &request.group_identifier,
        &request.profile_identifier,
    )
    .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::DeviceCondition(
        devicehub_runtime::DeviceConditionCommand::Apply {
            group_identifier: request.group_identifier,
            profile_identifier: request.profile_identifier,
            expires_at: tokio::time::Instant::now() + Duration::from_secs(7),
            reply,
        },
    )) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    await_device_condition_command(response, "apply device condition").await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn clear_device_condition(
    State(state): State<PerformanceHttpState>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::DeviceCondition(
        devicehub_runtime::DeviceConditionCommand::Clear {
            expires_at: tokio::time::Instant::now() + Duration::from_secs(7),
            reply,
        },
    )) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    await_device_condition_command(response, "clear device condition").await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn await_device_condition_command(
    response: oneshot::Receiver<Result<(), String>>,
    operation: &str,
) -> Result<(), (StatusCode, String)> {
    let result = tokio::time::timeout(Duration::from_secs(8), response)
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
                format!("{operation} service stopped"),
            )
        })?;
    result.map_err(|error| (StatusCode::CONFLICT, error))
}

#[derive(Deserialize)]
struct StartNetworkCaptureRequest {
    destination: PathBuf,
    duration_seconds: u64,
    #[serde(default)]
    process_id: Option<u32>,
}

async fn start_network_capture(
    State(state): State<PerformanceHttpState>,
    Json(request): Json<StartNetworkCaptureRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    crate::network_capture::validate_request(&request.destination, request.duration_seconds)
        .await
        .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::NetworkCapture(
        crate::network_capture::NetworkCaptureCommand::Start {
            destination: request.destination,
            duration_seconds: request.duration_seconds,
            process_id: request.process_id,
            reply,
        },
    )) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    await_network_capture_command(response, "start packet capture").await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn stop_network_capture(
    State(state): State<PerformanceHttpState>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::NetworkCapture(
        crate::network_capture::NetworkCaptureCommand::Stop { reply },
    )) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    await_network_capture_command(response, "stop packet capture").await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn await_network_capture_command(
    response: oneshot::Receiver<Result<(), String>>,
    operation: &str,
) -> Result<(), (StatusCode, String)> {
    let result = tokio::time::timeout(Duration::from_secs(15), response)
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
        let status = if error.contains("already running") || error.contains("no packet capture") {
            StatusCode::CONFLICT
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        };
        (status, error)
    })
}

#[derive(Deserialize)]
struct StartBluetoothCaptureRequest {
    destination: PathBuf,
    duration_seconds: u64,
}

async fn start_bluetooth_capture(
    State(state): State<PerformanceHttpState>,
    Json(request): Json<StartBluetoothCaptureRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    crate::bluetooth_capture::validate_request(&request.destination, request.duration_seconds)
        .await
        .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::BluetoothCapture(
        crate::bluetooth_capture::BluetoothCaptureCommand::Start {
            destination: request.destination,
            duration_seconds: request.duration_seconds,
            reply,
        },
    )) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    await_bluetooth_capture_command(response, "start Bluetooth capture").await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn stop_bluetooth_capture(
    State(state): State<PerformanceHttpState>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::BluetoothCapture(
        crate::bluetooth_capture::BluetoothCaptureCommand::Stop { reply },
    )) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    await_bluetooth_capture_command(response, "stop Bluetooth capture").await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn await_bluetooth_capture_command(
    response: oneshot::Receiver<Result<(), String>>,
    operation: &str,
) -> Result<(), (StatusCode, String)> {
    let result = tokio::time::timeout(Duration::from_secs(15), response)
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
        let status = if error.contains("already running") || error.contains("no Bluetooth capture")
        {
            StatusCode::CONFLICT
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        };
        (status, error)
    })
}

async fn start_performance_sampling(State(state): State<PerformanceHttpState>) -> StatusCode {
    state.performance.reset();
    state.performance_demand.set(true);
    StatusCode::NO_CONTENT
}

async fn stop_performance_sampling(State(state): State<PerformanceHttpState>) -> StatusCode {
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
    State(state): State<PerformanceHttpState>,
    Query(query): Query<DeviceLogQuery>,
) -> Json<DeviceLogsView> {
    let service = state
        .services
        .snapshot()
        .into_iter()
        .find(|service| service.name == "device.logs");
    Json(DeviceLogsView {
        batch: state.device_logs.snapshot(
            query.after,
            query
                .limit
                .unwrap_or(devicehub_core::MAX_DEVICE_LOG_BATCH_ENTRIES),
            state.device_log_demand.enabled(),
        ),
        service,
    })
}

#[derive(Serialize)]
struct DeviceLogsView {
    #[serde(flatten)]
    batch: devicehub_core::DeviceLogBatch,
    service: Option<crate::supervisor::ServiceHealth>,
}

async fn start_device_logs(State(state): State<PerformanceHttpState>) -> StatusCode {
    state.device_log_demand.set(true);
    StatusCode::NO_CONTENT
}

async fn stop_device_logs(State(state): State<PerformanceHttpState>) -> StatusCode {
    state.device_log_demand.set(false);
    StatusCode::NO_CONTENT
}

async fn clear_device_logs(State(state): State<PerformanceHttpState>) -> StatusCode {
    state.device_logs.clear();
    StatusCode::NO_CONTENT
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    fn test_state() -> (PerformanceHttpState, UnboundedReceiver<InputCmd>) {
        let input = InputSink::default();
        let (input_tx, input_rx) = unbounded_channel();
        input.set(Some(input_tx));
        (
            PerformanceHttpState::new(
                PerformanceSlot::default(),
                PerformanceDemand::default(),
                devicehub_runtime::DeviceLogSlot::default(),
                devicehub_runtime::DeviceLogDemand::default(),
                devicehub_runtime::DeviceConditionSlot::default(),
                crate::network_capture::NetworkCaptureSlot::default(),
                crate::bluetooth_capture::BluetoothCaptureSlot::default(),
                ServiceRegistry::default(),
                input,
            ),
            input_rx,
        )
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
        assert!(view.app_activity.is_empty());
        assert_eq!(
            stop_performance_sampling(State(state.clone())).await,
            StatusCode::NO_CONTENT
        );
        assert!(!state.performance_demand.enabled());
    }

    #[tokio::test]
    async fn device_condition_endpoints_dispatch_to_the_supervised_service() {
        let (state, mut input_rx) = test_state();
        let apply = tokio::spawn(apply_device_condition(
            State(state.clone()),
            Json(ApplyDeviceConditionRequest {
                group_identifier: "Network".into(),
                profile_identifier: "Lossy LTE".into(),
            }),
        ));
        let InputCmd::DeviceCondition(devicehub_runtime::DeviceConditionCommand::Apply {
            group_identifier,
            profile_identifier,
            reply,
            ..
        }) = input_rx.recv().await.unwrap()
        else {
            panic!("expected apply device condition command");
        };
        assert_eq!(group_identifier, "Network");
        assert_eq!(profile_identifier, "Lossy LTE");
        reply.send(Ok(())).unwrap();
        assert_eq!(apply.await.unwrap().unwrap(), StatusCode::NO_CONTENT);

        let clear = tokio::spawn(clear_device_condition(State(state)));
        let InputCmd::DeviceCondition(devicehub_runtime::DeviceConditionCommand::Clear {
            reply,
            ..
        }) = input_rx.recv().await.unwrap()
        else {
            panic!("expected clear device condition command");
        };
        reply.send(Ok(())).unwrap();
        assert_eq!(clear.await.unwrap().unwrap(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn device_condition_rejects_invalid_identifiers_before_dispatch() {
        let (state, mut input_rx) = test_state();
        let error = apply_device_condition(
            State(state),
            Json(ApplyDeviceConditionRequest {
                group_identifier: "Network\nInjected".into(),
                profile_identifier: "LTE".into(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(error.0, StatusCode::BAD_REQUEST);
        assert!(input_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn network_capture_endpoints_validate_and_dispatch_commands() {
        let (state, mut input_rx) = test_state();
        let destination = std::env::temp_dir().join(format!(
            "devicehub-mask-http-performance-{}.pcap",
            uuid::Uuid::new_v4()
        ));
        let invalid = start_network_capture(
            State(state.clone()),
            Json(StartNetworkCaptureRequest {
                destination: destination.clone(),
                duration_seconds: 0,
                process_id: None,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(invalid.0, StatusCode::BAD_REQUEST);
        assert!(input_rx.try_recv().is_err());

        let start = tokio::spawn(start_network_capture(
            State(state.clone()),
            Json(StartNetworkCaptureRequest {
                destination: destination.clone(),
                duration_seconds: 30,
                process_id: Some(42),
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::NetworkCapture(crate::network_capture::NetworkCaptureCommand::Start {
                destination: actual,
                duration_seconds,
                process_id,
                reply,
            }) => {
                assert_eq!(actual, destination);
                assert_eq!(duration_seconds, 30);
                assert_eq!(process_id, Some(42));
                reply.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(start.await.unwrap().unwrap(), StatusCode::NO_CONTENT);

        let stop = tokio::spawn(stop_network_capture(State(state)));
        match input_rx.recv().await.unwrap() {
            InputCmd::NetworkCapture(crate::network_capture::NetworkCaptureCommand::Stop {
                reply,
            }) => reply.send(Ok(())).unwrap(),
            _ => panic!("unexpected command"),
        }
        assert_eq!(stop.await.unwrap().unwrap(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn bluetooth_capture_endpoints_validate_and_dispatch_commands() {
        let (state, mut input_rx) = test_state();
        let destination = std::env::temp_dir().join(format!(
            "devicehub-mask-bluetooth-http-performance-{}.pcap",
            uuid::Uuid::new_v4()
        ));
        let invalid = start_bluetooth_capture(
            State(state.clone()),
            Json(StartBluetoothCaptureRequest {
                destination: destination.clone(),
                duration_seconds: 0,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(invalid.0, StatusCode::BAD_REQUEST);
        assert!(input_rx.try_recv().is_err());

        let start = tokio::spawn(start_bluetooth_capture(
            State(state.clone()),
            Json(StartBluetoothCaptureRequest {
                destination: destination.clone(),
                duration_seconds: 30,
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::BluetoothCapture(
                crate::bluetooth_capture::BluetoothCaptureCommand::Start {
                    destination: actual,
                    duration_seconds,
                    reply,
                },
            ) => {
                assert_eq!(actual, destination);
                assert_eq!(duration_seconds, 30);
                reply.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(start.await.unwrap().unwrap(), StatusCode::NO_CONTENT);

        let stop = tokio::spawn(stop_bluetooth_capture(State(state)));
        match input_rx.recv().await.unwrap() {
            InputCmd::BluetoothCapture(
                crate::bluetooth_capture::BluetoothCaptureCommand::Stop { reply },
            ) => reply.send(Ok(())).unwrap(),
            _ => panic!("unexpected command"),
        }
        assert_eq!(stop.await.unwrap().unwrap(), StatusCode::NO_CONTENT);
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
            Query(DeviceLogQuery {
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
    async fn running_process_endpoint_dispatches_a_bounded_read_only_query() {
        let (state, mut input_rx) = test_state();
        let request = tokio::spawn(running_processes(State(state)));
        let InputCmd::RunningProcess(devicehub_runtime::RunningProcessCommand::List { reply }) =
            input_rx.recv().await.unwrap()
        else {
            panic!("expected running process query");
        };
        reply
            .send(Ok(devicehub_core::RunningProcessList {
                processes: vec![devicehub_core::RunningProcess {
                    pid: 42,
                    name: "Example".into(),
                    app_name: Some("Example App".into()),
                    is_application: true,
                }],
                truncated: false,
            }))
            .unwrap();
        let response = request.await.unwrap().unwrap();
        assert_eq!(response.0.processes[0].pid, 42);
        assert_eq!(
            response.0.processes[0].app_name.as_deref(),
            Some("Example App")
        );
    }
}
