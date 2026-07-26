//! HTTP adapter for provisioning-profile management on the active device.
//!
//! Install sources remain opaque host paths. The runtime invokes a
//! host-provided loader to validate and read them before mutating the device.

use std::path::PathBuf;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, put};
use axum::{Json, Router};
use serde::Deserialize;
use tokio::sync::oneshot;

use devicehub_core::ProvisioningProfile;
use devicehub_runtime::{
    DeviceSessionCommand, ProvisioningCommand, ProvisioningFailure, SessionCommandSlot,
};

type InputCmd = DeviceSessionCommand<PathBuf>;
type InputSink = SessionCommandSlot<PathBuf>;

const COMMAND_DEADLINE: Duration = Duration::from_secs(20);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(22);

#[derive(Clone, Default)]
pub struct ProvisioningHttpState {
    input: InputSink,
}

impl ProvisioningHttpState {
    pub fn new(input: InputSink) -> Self {
        Self { input }
    }
}

pub fn router<S>(state: ProvisioningHttpState) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route(
            "/api/device/provisioning-profiles",
            get(list_profiles).put(install_profile),
        )
        .route(
            "/api/device/provisioning-profiles/{uuid}",
            delete(remove_profile),
        )
        .route(
            "/api/device/provisioning-profiles/{uuid}/trust",
            put(trust_profile_signer),
        )
        .with_state(state)
}

async fn list_profiles(
    State(state): State<ProvisioningHttpState>,
) -> Result<Json<Vec<ProvisioningProfile>>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    require_active_session(state.input.try_send(InputCmd::Provisioning(
        ProvisioningCommand::List {
            expires_at: tokio::time::Instant::now() + COMMAND_DEADLINE,
            reply,
        },
    )))?;
    let profiles = await_response(response, "provisioning profile request").await?;
    Ok(Json(profiles))
}

#[derive(Deserialize)]
struct InstallProfileRequest {
    path: PathBuf,
}

async fn install_profile(
    State(state): State<ProvisioningHttpState>,
    Json(request): Json<InstallProfileRequest>,
) -> Result<Json<ProvisioningProfile>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    require_active_session(state.input.try_send(InputCmd::Provisioning(
        ProvisioningCommand::Install {
            source: request.path,
            expires_at: tokio::time::Instant::now() + COMMAND_DEADLINE,
            reply,
        },
    )))?;
    let profile = await_response(response, "provisioning profile installation").await?;
    Ok(Json(profile))
}

async fn remove_profile(
    State(state): State<ProvisioningHttpState>,
    Path(uuid): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    validate_uuid(&uuid)?;
    let (reply, response) = oneshot::channel();
    require_active_session(state.input.try_send(InputCmd::Provisioning(
        ProvisioningCommand::Remove {
            uuid,
            expires_at: tokio::time::Instant::now() + COMMAND_DEADLINE,
            reply,
        },
    )))?;
    await_response(response, "provisioning profile removal").await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn trust_profile_signer(
    State(state): State<ProvisioningHttpState>,
    Path(uuid): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let uuid = normalized_uuid(uuid)?;
    let (reply, response) = oneshot::channel();
    require_active_session(state.input.try_send(InputCmd::Provisioning(
        ProvisioningCommand::TrustSigner {
            uuid,
            expires_at: tokio::time::Instant::now() + COMMAND_DEADLINE,
            reply,
        },
    )))?;
    await_response(response, "app signer trust request").await?;
    Ok(StatusCode::NO_CONTENT)
}

fn normalized_uuid(uuid: String) -> Result<String, (StatusCode, String)> {
    uuid::Uuid::parse_str(&uuid)
        .map(|uuid| uuid.to_string())
        .map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                "invalid provisioning profile UUID".into(),
            )
        })
}

fn validate_uuid(uuid: &str) -> Result<(), (StatusCode, String)> {
    uuid::Uuid::parse_str(uuid).map(|_| ()).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "invalid provisioning profile UUID".into(),
        )
    })
}

async fn await_response<T>(
    response: oneshot::Receiver<Result<T, ProvisioningFailure>>,
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
        .map_err(|error| {
            let status = match error {
                ProvisioningFailure::Invalid(_) => StatusCode::BAD_REQUEST,
                ProvisioningFailure::NotFound(_) => StatusCode::NOT_FOUND,
                ProvisioningFailure::Conflict(_) => StatusCode::CONFLICT,
                ProvisioningFailure::Operation(_) | ProvisioningFailure::Unavailable(_) => {
                    StatusCode::BAD_GATEWAY
                }
                ProvisioningFailure::Deadline(_) | ProvisioningFailure::Timeout(_) => {
                    StatusCode::GATEWAY_TIMEOUT
                }
            };
            (status, error.to_string())
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

    fn test_state() -> (
        ProvisioningHttpState,
        tokio::sync::mpsc::UnboundedReceiver<InputCmd>,
    ) {
        let input = InputSink::default();
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        input.set(Some(sender));
        (ProvisioningHttpState::new(input), receiver)
    }

    fn profile() -> ProvisioningProfile {
        ProvisioningProfile {
            name: "Game Development".into(),
            uuid: "00000000-1111-2222-3333-444444444444".into(),
            team_identifiers: vec!["TEAM123".into()],
            application_identifier: Some("TEAM123.com.example.game".into()),
            creation_date: None,
            expiration_date: None,
            provisioned_devices: 1,
            is_expired: false,
            get_task_allow: true,
            removal_supported: true,
            parse_error: None,
        }
    }

    #[tokio::test]
    async fn lifecycle_routes_dispatch_typed_commands_and_opaque_sources() {
        let (state, mut commands) = test_state();
        let list = tokio::spawn(list_profiles(State(state.clone())));
        let InputCmd::Provisioning(ProvisioningCommand::List { reply, .. }) =
            commands.recv().await.unwrap()
        else {
            panic!("expected provisioning list command");
        };
        reply.send(Ok(Vec::new())).unwrap();
        assert!(list.await.unwrap().unwrap().0.is_empty());

        let install = tokio::spawn(install_profile(
            State(state.clone()),
            Json(InstallProfileRequest {
                path: PathBuf::from("/tmp/Game.mobileprovision"),
            }),
        ));
        let InputCmd::Provisioning(ProvisioningCommand::Install { source, reply, .. }) =
            commands.recv().await.unwrap()
        else {
            panic!("expected provisioning install command");
        };
        assert_eq!(source, PathBuf::from("/tmp/Game.mobileprovision"));
        let expected = profile();
        reply.send(Ok(expected.clone())).unwrap();
        assert_eq!(install.await.unwrap().unwrap().0.uuid, expected.uuid);

        let remove = tokio::spawn(remove_profile(
            State(state.clone()),
            Path("00000000-1111-2222-3333-444444444444".into()),
        ));
        let InputCmd::Provisioning(ProvisioningCommand::Remove { uuid, reply, .. }) =
            commands.recv().await.unwrap()
        else {
            panic!("expected provisioning remove command");
        };
        assert_eq!(uuid, "00000000-1111-2222-3333-444444444444");
        reply.send(Ok(())).unwrap();
        assert_eq!(remove.await.unwrap().unwrap(), StatusCode::NO_CONTENT);

        let trust = tokio::spawn(trust_profile_signer(
            State(state),
            Path("00000000-AAAA-BBBB-CCCC-111111111111".into()),
        ));
        let InputCmd::Provisioning(ProvisioningCommand::TrustSigner { uuid, reply, .. }) =
            commands.recv().await.unwrap()
        else {
            panic!("expected app signer trust command");
        };
        assert_eq!(uuid, "00000000-aaaa-bbbb-cccc-111111111111");
        reply.send(Ok(())).unwrap();
        assert_eq!(trust.await.unwrap().unwrap(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn invalid_uuid_is_rejected_before_dispatch() {
        let (state, mut commands) = test_state();
        assert_eq!(
            remove_profile(State(state.clone()), Path("not-a-uuid".into()))
                .await
                .unwrap_err()
                .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            trust_profile_signer(State(state), Path("still-not-a-uuid".into()))
                .await
                .unwrap_err()
                .0,
            StatusCode::BAD_REQUEST
        );
        assert!(commands.try_recv().is_err());
    }

    #[tokio::test]
    async fn failures_map_to_actionable_http_statuses() {
        let cases = [
            (
                ProvisioningFailure::Invalid("invalid".into()),
                StatusCode::BAD_REQUEST,
            ),
            (
                ProvisioningFailure::NotFound("missing".into()),
                StatusCode::NOT_FOUND,
            ),
            (
                ProvisioningFailure::Conflict("conflict".into()),
                StatusCode::CONFLICT,
            ),
            (
                ProvisioningFailure::Operation("failed".into()),
                StatusCode::BAD_GATEWAY,
            ),
            (
                ProvisioningFailure::Unavailable("closed".into()),
                StatusCode::BAD_GATEWAY,
            ),
            (
                ProvisioningFailure::Deadline("expired".into()),
                StatusCode::GATEWAY_TIMEOUT,
            ),
            (
                ProvisioningFailure::Timeout("slow".into()),
                StatusCode::GATEWAY_TIMEOUT,
            ),
        ];
        for (failure, expected) in cases {
            let (reply, response) = oneshot::channel();
            reply.send(Err(failure)).unwrap();
            assert_eq!(
                await_response::<()>(response, "test").await.unwrap_err().0,
                expected
            );
        }
    }

    #[tokio::test]
    async fn routes_require_an_active_session() {
        let state = ProvisioningHttpState::default();
        assert!(matches!(
            list_profiles(State(state.clone())).await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
        assert!(matches!(
            trust_profile_signer(
                State(state),
                Path("00000000-aaaa-bbbb-cccc-111111111111".into()),
            )
            .await,
            Err((StatusCode::SERVICE_UNAVAILABLE, _))
        ));
    }

    #[test]
    fn router_constructs_without_filesystem_or_runtime_owner() {
        let _: Router = router(ProvisioningHttpState::default());
    }
}
