//! HTTP adapter for the runtime-owned Developer Disk Image lifecycle.
//!
//! Host paths remain opaque command values. The host-injected runtime asset
//! loader performs all filesystem validation and reads outside this adapter.

use std::path::PathBuf;
use std::time::Duration;

use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use tokio::sync::oneshot;

use devicehub_core::{DeveloperImageMountSlot, DeveloperImageMountStatus};
use devicehub_runtime::{
    DeveloperImageMountCommand, DeveloperImageMountRequest, DeviceSessionCommand,
    SessionCommandSlot,
};

type InputCmd = DeviceSessionCommand<PathBuf>;
type InputSink = SessionCommandSlot<PathBuf>;
type RequestSession = Option<Extension<devicehub_runtime::DeviceSessionClient<PathBuf>>>;

const DEVELOPER_IMAGE_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Default)]
pub struct DeveloperImageHttpState {
    input: InputSink,
    status: DeveloperImageMountSlot,
}

impl DeveloperImageHttpState {
    pub fn new(input: InputSink, status: DeveloperImageMountSlot) -> Self {
        Self { input, status }
    }

    fn input(&self, session: &RequestSession) -> InputSink {
        session
            .as_ref()
            .map(|session| session.commands.clone())
            .unwrap_or_else(|| self.input.clone())
    }

    fn status(&self, session: &RequestSession) -> DeveloperImageMountSlot {
        session
            .as_ref()
            .map(|session| session.developer_image.clone())
            .unwrap_or_else(|| self.status.clone())
    }
}

pub fn router<S>(state: DeveloperImageHttpState) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route(
            "/api/device/developer-image",
            get(developer_image_status)
                .put(start_developer_image_mount)
                .delete(stop_developer_image_mount),
        )
        .route(
            "/api/device/developer-image/unmount",
            axum::routing::put(unmount_developer_image),
        )
        .with_state(state)
}

async fn developer_image_status(
    State(state): State<DeveloperImageHttpState>,
    session: RequestSession,
) -> Json<DeveloperImageMountStatus> {
    Json(state.status(&session).get())
}

async fn start_developer_image_mount(
    State(state): State<DeveloperImageHttpState>,
    session: RequestSession,
    Json(request): Json<DeveloperImageMountRequest<PathBuf>>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    require_active_session(
        state
            .input(&session)
            .try_send(InputCmd::DeveloperImageMount(
                DeveloperImageMountCommand::Start { request, reply },
            )),
    )?;
    await_developer_image_command(response, "start developer image mount").await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn stop_developer_image_mount(
    State(state): State<DeveloperImageHttpState>,
    session: RequestSession,
) -> Result<StatusCode, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    require_active_session(
        state
            .input(&session)
            .try_send(InputCmd::DeveloperImageMount(
                DeveloperImageMountCommand::Stop { reply },
            )),
    )?;
    await_developer_image_command(response, "stop developer image mount").await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn unmount_developer_image(
    State(state): State<DeveloperImageHttpState>,
    session: RequestSession,
) -> Result<StatusCode, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    require_active_session(
        state
            .input(&session)
            .try_send(InputCmd::DeveloperImageMount(
                DeveloperImageMountCommand::Unmount { reply },
            )),
    )?;
    await_developer_image_command(response, "unmount developer image").await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn await_developer_image_command(
    response: oneshot::Receiver<Result<(), String>>,
    operation: &str,
) -> Result<(), (StatusCode, String)> {
    let result = tokio::time::timeout(DEVELOPER_IMAGE_REQUEST_TIMEOUT, response)
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

fn require_active_session(sent: bool) -> Result<(), (StatusCode, String)> {
    sent.then_some(()).ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "no active device session".into(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use devicehub_core::DeveloperImageMountState;

    fn test_state() -> (
        DeveloperImageHttpState,
        tokio::sync::mpsc::UnboundedReceiver<InputCmd>,
    ) {
        let input = InputSink::default();
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        input.set(Some(sender));
        (
            DeveloperImageHttpState::new(input, DeveloperImageMountSlot::default()),
            receiver,
        )
    }

    #[tokio::test]
    async fn lifecycle_routes_dispatch_opaque_host_sources() {
        let (state, mut commands) = test_state();
        assert_eq!(
            developer_image_status(State(state.clone()), None)
                .await
                .0
                .state,
            DeveloperImageMountState::Idle
        );
        let request = DeveloperImageMountRequest {
            image: PathBuf::from("/DeveloperDiskImage.dmg"),
            signature: None,
            trust_cache: Some(PathBuf::from("/DeveloperDiskImage.dmg.trustcache")),
            manifest: Some(PathBuf::from("/BuildManifest.plist")),
        };

        let start = tokio::spawn(start_developer_image_mount(
            State(state.clone()),
            None,
            Json(request),
        ));
        let InputCmd::DeveloperImageMount(DeveloperImageMountCommand::Start { request, reply }) =
            commands.recv().await.unwrap()
        else {
            panic!("expected start command");
        };
        assert_eq!(request.image, PathBuf::from("/DeveloperDiskImage.dmg"));
        assert_eq!(
            request.trust_cache,
            Some(PathBuf::from("/DeveloperDiskImage.dmg.trustcache"))
        );
        reply.send(Ok(())).unwrap();
        assert_eq!(start.await.unwrap().unwrap(), StatusCode::NO_CONTENT);

        let stop = tokio::spawn(stop_developer_image_mount(State(state.clone()), None));
        let InputCmd::DeveloperImageMount(DeveloperImageMountCommand::Stop { reply }) =
            commands.recv().await.unwrap()
        else {
            panic!("expected stop command");
        };
        reply.send(Ok(())).unwrap();
        assert_eq!(stop.await.unwrap().unwrap(), StatusCode::NO_CONTENT);

        let unmount = tokio::spawn(unmount_developer_image(State(state), None));
        let InputCmd::DeveloperImageMount(DeveloperImageMountCommand::Unmount { reply }) =
            commands.recv().await.unwrap()
        else {
            panic!("expected unmount command");
        };
        reply.send(Ok(())).unwrap();
        assert_eq!(unmount.await.unwrap().unwrap(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn lifecycle_routes_require_an_active_session() {
        let state = DeveloperImageHttpState::default();
        let request = DeveloperImageMountRequest {
            image: PathBuf::from("/DeveloperDiskImage.dmg"),
            signature: None,
            trust_cache: None,
            manifest: None,
        };
        assert!(matches!(
            start_developer_image_mount(State(state), None, Json(request)).await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
    }

    #[test]
    fn router_constructs_without_filesystem_or_runtime_owner() {
        let _: Router = router(DeveloperImageHttpState::default());
    }
}
