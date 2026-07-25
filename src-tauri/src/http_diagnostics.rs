//! HTTP adapter for long-running device diagnostic exports.
//!
//! The active session owns each export task. This module owns only request
//! validation, bounded command acknowledgement, and read-only status mapping.

use std::path::PathBuf;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use tokio::sync::oneshot;

use crate::protocol::{InputCmd, InputSink};

#[derive(Clone, Default)]
pub(crate) struct DiagnosticsHttpState {
    input: InputSink,
    device_backup: crate::device_backup::DeviceBackupSlot,
    sysdiagnose: crate::sysdiagnose::SysdiagnoseSlot,
    log_archive: crate::log_archive::LogArchiveSlot,
}

impl DiagnosticsHttpState {
    pub(crate) fn new(
        input: InputSink,
        device_backup: crate::device_backup::DeviceBackupSlot,
        sysdiagnose: crate::sysdiagnose::SysdiagnoseSlot,
        log_archive: crate::log_archive::LogArchiveSlot,
    ) -> Self {
        Self {
            input,
            device_backup,
            sysdiagnose,
            log_archive,
        }
    }
}

pub(crate) fn router<S>(state: DiagnosticsHttpState) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route(
            "/api/device/backup",
            get(device_backup_status)
                .put(start_device_backup)
                .delete(stop_device_backup),
        )
        .route(
            "/api/device/sysdiagnose",
            get(sysdiagnose_status)
                .put(start_sysdiagnose)
                .delete(stop_sysdiagnose),
        )
        .route(
            "/api/device/log-archive",
            get(log_archive_status)
                .put(start_log_archive)
                .delete(stop_log_archive),
        )
        .with_state(state)
}

async fn device_backup_status(
    State(state): State<DiagnosticsHttpState>,
) -> Json<crate::device_backup::DeviceBackupStatus> {
    Json(state.device_backup.get())
}

#[derive(Deserialize)]
struct StartDeviceBackupRequest {
    destination: PathBuf,
    #[serde(default)]
    full: bool,
}

async fn start_device_backup(
    State(state): State<DiagnosticsHttpState>,
    Json(request): Json<StartDeviceBackupRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let destination = crate::device_backup::prepare_destination(&request.destination)
        .await
        .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::DeviceBackup(
        crate::device_backup::DeviceBackupCommand::Start {
            destination,
            full: request.full,
            reply,
        },
    )) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    await_export_command(
        response,
        "start device backup",
        Duration::from_secs(45),
        |error| error.contains("already running") || error.contains("no device backup"),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn stop_device_backup(
    State(state): State<DiagnosticsHttpState>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::DeviceBackup(
        crate::device_backup::DeviceBackupCommand::Stop { reply },
    )) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    await_export_command(
        response,
        "stop device backup",
        Duration::from_secs(45),
        |error| error.contains("already running") || error.contains("no device backup"),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn sysdiagnose_status(
    State(state): State<DiagnosticsHttpState>,
) -> Json<crate::sysdiagnose::SysdiagnoseStatus> {
    Json(state.sysdiagnose.get())
}

#[derive(Deserialize)]
struct StartSysdiagnoseRequest {
    destination: PathBuf,
}

async fn start_sysdiagnose(
    State(state): State<DiagnosticsHttpState>,
    Json(request): Json<StartSysdiagnoseRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let destination = crate::sysdiagnose::prepare_destination(&request.destination)
        .await
        .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::Sysdiagnose(
        crate::sysdiagnose::SysdiagnoseCommand::Start { destination, reply },
    )) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    await_export_command(
        response,
        "start sysdiagnose export",
        Duration::from_secs(10),
        |error| error.contains("already running") || error.contains("no sysdiagnose"),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn stop_sysdiagnose(
    State(state): State<DiagnosticsHttpState>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::Sysdiagnose(
        crate::sysdiagnose::SysdiagnoseCommand::Stop { reply },
    )) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    await_export_command(
        response,
        "stop sysdiagnose export",
        Duration::from_secs(10),
        |error| error.contains("already running") || error.contains("no sysdiagnose"),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn log_archive_status(
    State(state): State<DiagnosticsHttpState>,
) -> Json<crate::log_archive::LogArchiveStatus> {
    Json(state.log_archive.get())
}

#[derive(Deserialize)]
struct StartLogArchiveRequest {
    destination: PathBuf,
    age_limit_hours: u16,
}

async fn start_log_archive(
    State(state): State<DiagnosticsHttpState>,
    Json(request): Json<StartLogArchiveRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let destination = crate::log_archive::prepare_destination(&request.destination)
        .await
        .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    let age_limit_hours = crate::log_archive::validate_age_limit_hours(request.age_limit_hours)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::LogArchive(
        crate::log_archive::LogArchiveCommand::Start {
            destination,
            age_limit_hours,
            reply,
        },
    )) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    await_export_command(
        response,
        "start log archive export",
        Duration::from_secs(10),
        |error| error.contains("already running") || error.contains("no log archive"),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn stop_log_archive(
    State(state): State<DiagnosticsHttpState>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state.input.try_send(InputCmd::LogArchive(
        crate::log_archive::LogArchiveCommand::Stop { reply },
    )) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        ));
    }
    await_export_command(
        response,
        "stop log archive export",
        Duration::from_secs(10),
        |error| error.contains("already running") || error.contains("no log archive"),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn await_export_command(
    response: oneshot::Receiver<Result<(), String>>,
    operation: &str,
    timeout: Duration,
    is_conflict: impl FnOnce(&str) -> bool,
) -> Result<(), (StatusCode, String)> {
    let result = tokio::time::timeout(timeout, response)
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
        let status = if is_conflict(&error) {
            StatusCode::CONFLICT
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        };
        (status, error)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    fn test_state() -> (DiagnosticsHttpState, UnboundedReceiver<InputCmd>) {
        let input = InputSink::default();
        let (input_tx, input_rx) = unbounded_channel();
        input.set(Some(input_tx));
        (
            DiagnosticsHttpState::new(
                input,
                crate::device_backup::DeviceBackupSlot::default(),
                crate::sysdiagnose::SysdiagnoseSlot::default(),
                crate::log_archive::LogArchiveSlot::default(),
            ),
            input_rx,
        )
    }

    #[tokio::test]
    async fn device_backup_endpoints_validate_and_dispatch_commands() {
        let (state, mut input_rx) = test_state();
        let missing = std::env::temp_dir().join(format!(
            "devicehub-mask-missing-web-backup-{}",
            uuid::Uuid::new_v4()
        ));
        let invalid = start_device_backup(
            State(state.clone()),
            Json(StartDeviceBackupRequest {
                destination: missing,
                full: false,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(invalid.0, StatusCode::BAD_REQUEST);
        assert!(input_rx.try_recv().is_err());

        let destination = std::env::temp_dir();
        let expected = tokio::fs::canonicalize(&destination).await.unwrap();
        let start = tokio::spawn(start_device_backup(
            State(state.clone()),
            Json(StartDeviceBackupRequest {
                destination,
                full: true,
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::DeviceBackup(crate::device_backup::DeviceBackupCommand::Start {
                destination,
                full,
                reply,
            }) => {
                assert_eq!(destination, expected);
                assert!(full);
                reply.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(start.await.unwrap().unwrap(), StatusCode::NO_CONTENT);
        assert_eq!(
            device_backup_status(State(state.clone())).await.0.state,
            crate::device_backup::DeviceBackupState::Idle
        );

        let stop = tokio::spawn(stop_device_backup(State(state)));
        match input_rx.recv().await.unwrap() {
            InputCmd::DeviceBackup(crate::device_backup::DeviceBackupCommand::Stop { reply }) => {
                reply.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(stop.await.unwrap().unwrap(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn sysdiagnose_endpoints_validate_and_dispatch_commands() {
        let (state, mut input_rx) = test_state();
        let invalid = start_sysdiagnose(
            State(state.clone()),
            Json(StartSysdiagnoseRequest {
                destination: PathBuf::from("relative.tar.gz"),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(invalid.0, StatusCode::BAD_REQUEST);
        assert!(input_rx.try_recv().is_err());

        let destination = std::env::temp_dir().join(format!(
            "devicehub-mask-web-sysdiagnose-{}.tar.gz",
            uuid::Uuid::new_v4()
        ));
        let expected = crate::sysdiagnose::prepare_destination(&destination)
            .await
            .unwrap();
        let start = tokio::spawn(start_sysdiagnose(
            State(state.clone()),
            Json(StartSysdiagnoseRequest { destination }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::Sysdiagnose(crate::sysdiagnose::SysdiagnoseCommand::Start {
                destination,
                reply,
            }) => {
                assert_eq!(destination, expected);
                reply.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(start.await.unwrap().unwrap(), StatusCode::NO_CONTENT);
        assert_eq!(
            sysdiagnose_status(State(state.clone())).await.0.state,
            crate::sysdiagnose::SysdiagnoseState::Idle
        );

        let stop = tokio::spawn(stop_sysdiagnose(State(state)));
        match input_rx.recv().await.unwrap() {
            InputCmd::Sysdiagnose(crate::sysdiagnose::SysdiagnoseCommand::Stop { reply }) => {
                reply.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(stop.await.unwrap().unwrap(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn log_archive_endpoints_validate_and_dispatch_commands() {
        let (state, mut input_rx) = test_state();
        let invalid = start_log_archive(
            State(state.clone()),
            Json(StartLogArchiveRequest {
                destination: PathBuf::from("relative.tar"),
                age_limit_hours: 2,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(invalid.0, StatusCode::BAD_REQUEST);
        assert!(input_rx.try_recv().is_err());

        let destination = std::env::temp_dir().join(format!(
            "devicehub-mask-web-log-archive-{}.tar",
            uuid::Uuid::new_v4()
        ));
        let invalid_age = start_log_archive(
            State(state.clone()),
            Json(StartLogArchiveRequest {
                destination: destination.clone(),
                age_limit_hours: 2,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(invalid_age.0, StatusCode::BAD_REQUEST);
        assert!(input_rx.try_recv().is_err());

        let expected = crate::log_archive::prepare_destination(&destination)
            .await
            .unwrap();
        let start = tokio::spawn(start_log_archive(
            State(state.clone()),
            Json(StartLogArchiveRequest {
                destination,
                age_limit_hours: 6,
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::LogArchive(crate::log_archive::LogArchiveCommand::Start {
                destination,
                age_limit_hours,
                reply,
            }) => {
                assert_eq!(destination, expected);
                assert_eq!(age_limit_hours, 6);
                reply.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(start.await.unwrap().unwrap(), StatusCode::NO_CONTENT);
        assert_eq!(
            log_archive_status(State(state.clone())).await.0.state,
            crate::log_archive::LogArchiveState::Idle
        );

        let stop = tokio::spawn(stop_log_archive(State(state)));
        match input_rx.recv().await.unwrap() {
            InputCmd::LogArchive(crate::log_archive::LogArchiveCommand::Stop { reply }) => {
                reply.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(stop.await.unwrap().unwrap(), StatusCode::NO_CONTENT);
    }
}
