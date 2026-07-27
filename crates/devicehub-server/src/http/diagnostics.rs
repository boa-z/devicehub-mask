//! HTTP adapter for long-running device diagnostic exports.
//!
//! The active session owns each export task. This module owns only request
//! validation, bounded command acknowledgement, and read-only status mapping.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use tokio::sync::oneshot;

use devicehub_core::{
    DeviceBackupSlot, DeviceBackupStatus, LogArchiveSlot, LogArchiveStatus, SysdiagnoseSlot,
    SysdiagnoseStatus,
};

type InputCmd = devicehub_runtime::DeviceSessionCommand<PathBuf>;
type InputSink = devicehub_runtime::SessionCommandSlot<PathBuf>;
type RequestSession = Option<Extension<devicehub_runtime::DeviceSessionClient<PathBuf>>>;
type DeviceBackupCommand = devicehub_runtime::DeviceBackupCommand<PathBuf>;
type SysdiagnoseCommand = devicehub_runtime::SysdiagnoseCommand<PathBuf>;
type LogArchiveCommand = devicehub_runtime::LogArchiveCommand<PathBuf>;
type DiagnosticDestinationFuture =
    Pin<Box<dyn Future<Output = Result<PathBuf, String>> + Send + 'static>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticDestinationKind {
    BackupDirectory,
    SysdiagnoseFile,
    LogArchiveFile,
}

/// Host capability for validating and normalizing diagnostic export targets.
#[derive(Clone)]
pub struct DiagnosticDestinationPreparer {
    prepare: Arc<
        dyn Fn(PathBuf, DiagnosticDestinationKind) -> DiagnosticDestinationFuture + Send + Sync,
    >,
}

impl DiagnosticDestinationPreparer {
    pub fn new<F, Fut>(prepare: F) -> Self
    where
        F: Fn(PathBuf, DiagnosticDestinationKind) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<PathBuf, String>> + Send + 'static,
    {
        Self {
            prepare: Arc::new(move |destination, kind| Box::pin(prepare(destination, kind))),
        }
    }

    async fn prepare(
        &self,
        destination: PathBuf,
        kind: DiagnosticDestinationKind,
    ) -> Result<PathBuf, String> {
        (self.prepare)(destination, kind).await
    }
}

#[derive(Clone)]
pub struct DiagnosticsHttpState {
    input: InputSink,
    device_backup: DeviceBackupSlot,
    sysdiagnose: SysdiagnoseSlot,
    log_archive: LogArchiveSlot,
    destinations: DiagnosticDestinationPreparer,
}

impl DiagnosticsHttpState {
    pub fn new(
        input: InputSink,
        device_backup: DeviceBackupSlot,
        sysdiagnose: SysdiagnoseSlot,
        log_archive: LogArchiveSlot,
        destinations: DiagnosticDestinationPreparer,
    ) -> Self {
        Self {
            input,
            device_backup,
            sysdiagnose,
            log_archive,
            destinations,
        }
    }

    fn input(&self, session: &RequestSession) -> InputSink {
        session
            .as_ref()
            .map(|session| session.commands.clone())
            .unwrap_or_else(|| self.input.clone())
    }

    fn device_backup(&self, session: &RequestSession) -> DeviceBackupSlot {
        session
            .as_ref()
            .map(|session| session.device_backup.clone())
            .unwrap_or_else(|| self.device_backup.clone())
    }

    fn sysdiagnose(&self, session: &RequestSession) -> SysdiagnoseSlot {
        session
            .as_ref()
            .map(|session| session.sysdiagnose.clone())
            .unwrap_or_else(|| self.sysdiagnose.clone())
    }

    fn log_archive(&self, session: &RequestSession) -> LogArchiveSlot {
        session
            .as_ref()
            .map(|session| session.log_archive.clone())
            .unwrap_or_else(|| self.log_archive.clone())
    }
}

pub fn router<S>(state: DiagnosticsHttpState) -> Router<S>
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
    session: RequestSession,
) -> Json<DeviceBackupStatus> {
    Json(state.device_backup(&session).get())
}

#[derive(Deserialize)]
struct StartDeviceBackupRequest {
    destination: PathBuf,
    #[serde(default)]
    full: bool,
}

async fn start_device_backup(
    State(state): State<DiagnosticsHttpState>,
    session: RequestSession,
    Json(request): Json<StartDeviceBackupRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let destination = state
        .destinations
        .prepare(
            request.destination,
            DiagnosticDestinationKind::BackupDirectory,
        )
        .await
        .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    let (reply, response) = oneshot::channel();
    if !state
        .input(&session)
        .try_send(InputCmd::DeviceBackup(DeviceBackupCommand::Start {
            destination,
            full: request.full,
            reply,
        }))
    {
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
    session: RequestSession,
) -> Result<StatusCode, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state
        .input(&session)
        .try_send(InputCmd::DeviceBackup(DeviceBackupCommand::Stop { reply }))
    {
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
    session: RequestSession,
) -> Json<SysdiagnoseStatus> {
    Json(state.sysdiagnose(&session).get())
}

#[derive(Deserialize)]
struct StartSysdiagnoseRequest {
    destination: PathBuf,
}

async fn start_sysdiagnose(
    State(state): State<DiagnosticsHttpState>,
    session: RequestSession,
    Json(request): Json<StartSysdiagnoseRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let destination = state
        .destinations
        .prepare(
            request.destination,
            DiagnosticDestinationKind::SysdiagnoseFile,
        )
        .await
        .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    let (reply, response) = oneshot::channel();
    if !state
        .input(&session)
        .try_send(InputCmd::Sysdiagnose(SysdiagnoseCommand::Start {
            destination,
            reply,
        }))
    {
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
    session: RequestSession,
) -> Result<StatusCode, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state
        .input(&session)
        .try_send(InputCmd::Sysdiagnose(SysdiagnoseCommand::Stop { reply }))
    {
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
    session: RequestSession,
) -> Json<LogArchiveStatus> {
    Json(state.log_archive(&session).get())
}

#[derive(Deserialize)]
struct StartLogArchiveRequest {
    destination: PathBuf,
    age_limit_hours: u16,
}

async fn start_log_archive(
    State(state): State<DiagnosticsHttpState>,
    session: RequestSession,
    Json(request): Json<StartLogArchiveRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let destination = state
        .destinations
        .prepare(
            request.destination,
            DiagnosticDestinationKind::LogArchiveFile,
        )
        .await
        .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    let age_limit_hours =
        devicehub_runtime::validate_log_archive_age_limit_hours(request.age_limit_hours)
            .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    let (reply, response) = oneshot::channel();
    if !state
        .input(&session)
        .try_send(InputCmd::LogArchive(LogArchiveCommand::Start {
            destination,
            age_limit_hours,
            reply,
        }))
    {
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
    session: RequestSession,
) -> Result<StatusCode, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    if !state
        .input(&session)
        .try_send(InputCmd::LogArchive(LogArchiveCommand::Stop { reply }))
    {
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
                DeviceBackupSlot::default(),
                SysdiagnoseSlot::default(),
                LogArchiveSlot::default(),
                DiagnosticDestinationPreparer::new(|destination, _| async move {
                    if destination.is_absolute() {
                        Ok(destination)
                    } else {
                        Err("diagnostic destination must be absolute".into())
                    }
                }),
            ),
            input_rx,
        )
    }

    #[tokio::test]
    async fn host_destination_policy_rejects_before_device_dispatch() {
        let (mut state, mut input_rx) = test_state();
        state.destinations = DiagnosticDestinationPreparer::new(|destination, kind| async move {
            assert_eq!(kind, DiagnosticDestinationKind::SysdiagnoseFile);
            assert_eq!(
                destination.file_name().and_then(|name| name.to_str()),
                Some("denied.tar.gz")
            );
            Err("diagnostic destination denied by host".into())
        });

        let error = start_sysdiagnose(
            State(state),
            None,
            Json(StartSysdiagnoseRequest {
                destination: std::env::temp_dir().join("denied.tar.gz"),
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(error.0, StatusCode::BAD_REQUEST);
        assert_eq!(error.1, "diagnostic destination denied by host");
        assert!(input_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn device_backup_endpoints_validate_and_dispatch_commands() {
        let (state, mut input_rx) = test_state();
        let invalid = start_device_backup(
            State(state.clone()),
            None,
            Json(StartDeviceBackupRequest {
                destination: PathBuf::from("relative-backup"),
                full: false,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(invalid.0, StatusCode::BAD_REQUEST);
        assert!(input_rx.try_recv().is_err());

        let destination = std::env::temp_dir();
        let expected = destination.clone();
        let start = tokio::spawn(start_device_backup(
            State(state.clone()),
            None,
            Json(StartDeviceBackupRequest {
                destination,
                full: true,
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::DeviceBackup(DeviceBackupCommand::Start {
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
            device_backup_status(State(state.clone()), None)
                .await
                .0
                .state,
            devicehub_core::DeviceBackupState::Idle
        );

        let stop = tokio::spawn(stop_device_backup(State(state), None));
        match input_rx.recv().await.unwrap() {
            InputCmd::DeviceBackup(DeviceBackupCommand::Stop { reply }) => {
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
            None,
            Json(StartSysdiagnoseRequest {
                destination: PathBuf::from("relative.tar.gz"),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(invalid.0, StatusCode::BAD_REQUEST);
        assert!(input_rx.try_recv().is_err());

        let destination = std::env::temp_dir().join("devicehub-mask-web-sysdiagnose.tar.gz");
        let expected = destination.clone();
        let start = tokio::spawn(start_sysdiagnose(
            State(state.clone()),
            None,
            Json(StartSysdiagnoseRequest { destination }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::Sysdiagnose(SysdiagnoseCommand::Start { destination, reply }) => {
                assert_eq!(destination, expected);
                reply.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(start.await.unwrap().unwrap(), StatusCode::NO_CONTENT);
        assert_eq!(
            sysdiagnose_status(State(state.clone()), None).await.0.state,
            devicehub_core::SysdiagnoseState::Idle
        );

        let stop = tokio::spawn(stop_sysdiagnose(State(state), None));
        match input_rx.recv().await.unwrap() {
            InputCmd::Sysdiagnose(SysdiagnoseCommand::Stop { reply }) => {
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
            None,
            Json(StartLogArchiveRequest {
                destination: PathBuf::from("relative.tar"),
                age_limit_hours: 2,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(invalid.0, StatusCode::BAD_REQUEST);
        assert!(input_rx.try_recv().is_err());

        let destination = std::env::temp_dir().join("devicehub-mask-web-log-archive.tar");
        let invalid_age = start_log_archive(
            State(state.clone()),
            None,
            Json(StartLogArchiveRequest {
                destination: destination.clone(),
                age_limit_hours: 2,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(invalid_age.0, StatusCode::BAD_REQUEST);
        assert!(input_rx.try_recv().is_err());

        let expected = destination.clone();
        let start = tokio::spawn(start_log_archive(
            State(state.clone()),
            None,
            Json(StartLogArchiveRequest {
                destination,
                age_limit_hours: 6,
            }),
        ));
        match input_rx.recv().await.unwrap() {
            InputCmd::LogArchive(LogArchiveCommand::Start {
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
            log_archive_status(State(state.clone()), None).await.0.state,
            devicehub_core::LogArchiveState::Idle
        );

        let stop = tokio::spawn(stop_log_archive(State(state), None));
        match input_rx.recv().await.unwrap() {
            InputCmd::LogArchive(LogArchiveCommand::Stop { reply }) => {
                reply.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected command"),
        }
        assert_eq!(stop.await.unwrap().unwrap(), StatusCode::NO_CONTENT);
    }
}
