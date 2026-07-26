//! HTTP adapter for device discovery, selection, and trust management.
//!
//! These operations belong to the runtime manager rather than an active device
//! session. Keeping the adapter on that narrow client prevents HTTP hosts from
//! acquiring session internals and prepares the route surface for a later
//! multi-device runtime registry.

use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::put;
use axum::{Json, Router};
use tokio::sync::oneshot;

use devicehub_core::{ForgetDeviceResult, PairDeviceResult};
use devicehub_runtime::{RuntimeManagerClient, SessionControlCommand};

const PAIRING_REQUEST_TIMEOUT: Duration = Duration::from_secs(95);
const FORGET_DEVICE_REQUEST_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Clone)]
pub struct DeviceManagerHttpState {
    manager: RuntimeManagerClient,
}

impl DeviceManagerHttpState {
    pub fn new(manager: RuntimeManagerClient) -> Self {
        Self { manager }
    }
}

pub fn router<S>(state: DeviceManagerHttpState) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/devices/refresh", put(refresh_devices))
        .route("/api/devices/{selection_id}/connect", put(connect_device))
        .route(
            "/api/devices/{selection_id}/reconnect",
            put(reconnect_device),
        )
        .route(
            "/api/devices/{selection_id}/pair",
            put(pair_device).delete(forget_device),
        )
        .with_state(state)
}

async fn refresh_devices(State(state): State<DeviceManagerHttpState>) -> StatusCode {
    let _ = state.manager.control.send(SessionControlCommand::Refresh);
    StatusCode::ACCEPTED
}

async fn connect_device(
    State(state): State<DeviceManagerHttpState>,
    Path(selection_id): Path<String>,
) -> StatusCode {
    let _ = state
        .manager
        .control
        .send(SessionControlCommand::Connect(selection_id));
    StatusCode::ACCEPTED
}

async fn reconnect_device(
    State(state): State<DeviceManagerHttpState>,
    Path(selection_id): Path<String>,
) -> StatusCode {
    let _ = state
        .manager
        .control
        .send(SessionControlCommand::Reconnect(selection_id));
    StatusCode::ACCEPTED
}

async fn pair_device(
    State(state): State<DeviceManagerHttpState>,
    Path(selection_id): Path<String>,
) -> Result<Json<PairDeviceResult>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    state
        .manager
        .control
        .send(SessionControlCommand::Pair {
            selection_id,
            reply,
        })
        .map_err(|_| manager_unavailable())?;
    let result = tokio::time::timeout(PAIRING_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "device pairing request timed out".into(),
            )
        })?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device pairing request was interrupted".into(),
            )
        })?;
    Ok(Json(result))
}

async fn forget_device(
    State(state): State<DeviceManagerHttpState>,
    Path(selection_id): Path<String>,
) -> Result<Json<ForgetDeviceResult>, (StatusCode, String)> {
    let (reply, response) = oneshot::channel();
    state
        .manager
        .control
        .send(SessionControlCommand::Forget {
            selection_id,
            reply,
        })
        .map_err(|_| manager_unavailable())?;
    let result = tokio::time::timeout(FORGET_DEVICE_REQUEST_TIMEOUT, response)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "device trust removal request timed out".into(),
            )
        })?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "device trust removal request was interrupted".into(),
            )
        })?;
    Ok(Json(result))
}

fn manager_unavailable() -> (StatusCode, String) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "device session manager is not running".into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use devicehub_core::{ForgetDeviceOutcome, PairDeviceOutcome};

    fn test_state() -> (
        DeviceManagerHttpState,
        tokio::sync::mpsc::UnboundedReceiver<SessionControlCommand>,
    ) {
        let (client, control) =
            devicehub_runtime::RuntimeClientFixture::<std::path::PathBuf>::default().build();
        (DeviceManagerHttpState::new(client.manager), control)
    }

    #[tokio::test]
    async fn lifecycle_routes_dispatch_to_the_runtime_manager() {
        let (state, mut control) = test_state();

        assert_eq!(
            refresh_devices(State(state.clone())).await,
            StatusCode::ACCEPTED
        );
        assert!(matches!(
            control.recv().await,
            Some(SessionControlCommand::Refresh)
        ));

        assert_eq!(
            connect_device(State(state.clone()), Path("device-1::usb".into())).await,
            StatusCode::ACCEPTED
        );
        assert!(matches!(
            control.recv().await,
            Some(SessionControlCommand::Connect(id)) if id == "device-1::usb"
        ));

        assert_eq!(
            reconnect_device(State(state), Path("device-1::wifi".into())).await,
            StatusCode::ACCEPTED
        );
        assert!(matches!(
            control.recv().await,
            Some(SessionControlCommand::Reconnect(id)) if id == "device-1::wifi"
        ));
    }

    #[tokio::test]
    async fn pairing_route_waits_for_the_manager_result() {
        let (state, mut control) = test_state();
        let request = tokio::spawn(pair_device(State(state), Path("device-1::usb".into())));

        let Some(SessionControlCommand::Pair {
            selection_id,
            reply,
        }) = control.recv().await
        else {
            panic!("expected pairing command");
        };
        assert_eq!(selection_id, "device-1::usb");
        reply
            .send(PairDeviceResult {
                outcome: PairDeviceOutcome::Paired,
                error: None,
            })
            .unwrap();

        let response = request.await.unwrap().unwrap().0;
        assert_eq!(response.outcome, PairDeviceOutcome::Paired);
        assert!(response.error.is_none());
    }

    #[tokio::test]
    async fn forget_route_waits_for_the_manager_result() {
        let (state, mut control) = test_state();
        let request = tokio::spawn(forget_device(State(state), Path("device-1::usb".into())));

        let Some(SessionControlCommand::Forget {
            selection_id,
            reply,
        }) = control.recv().await
        else {
            panic!("expected forget command");
        };
        assert_eq!(selection_id, "device-1::usb");
        reply
            .send(ForgetDeviceResult {
                outcome: ForgetDeviceOutcome::Forgotten,
                error: None,
            })
            .unwrap();

        let response = request.await.unwrap().unwrap().0;
        assert_eq!(response.outcome, ForgetDeviceOutcome::Forgotten);
        assert!(response.error.is_none());
    }

    #[tokio::test]
    async fn manager_shutdown_is_reported_without_waiting_for_a_timeout() {
        let (state, control) = test_state();
        drop(control);

        let error = pair_device(State(state), Path("device-1::usb".into()))
            .await
            .unwrap_err();
        assert_eq!(error.0, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn router_constructs_without_a_device_session() {
        let (state, _control) = test_state();
        let _: Router = router(state);
    }
}
