//! Device management actions that must run outside a connected session.

use std::collections::{HashMap, HashSet};

use devicehub_core::{
    ForgetDeviceResult, ManagedOperationError, ManagedOperationKind, OperationErrorCode,
    PairDeviceOutcome, PairDeviceResult,
};
use tokio::sync::oneshot;

use super::{ManagedSessionViews, SessionManagerViews};
use crate::device::unmount_image;
use crate::session::{ConnectedSessionViews, forget_device, pair_device};
use crate::transport::DeviceDiscovery;
use crate::{MuxSidecar, PairingStore, SessionEndpoint, connect_provider};

pub(super) enum PendingManagementAction {
    Pair(oneshot::Sender<PairDeviceResult>),
    Forget(oneshot::Sender<ForgetDeviceResult>),
    UnmountDeveloperImage {
        status: devicehub_core::DeveloperImageMountSlot,
        reply: oneshot::Sender<Result<(), String>>,
    },
}

pub(super) enum ManagementOutcome {
    None,
    Connect(String),
    Remove(String),
}

async fn pair_request(
    selection_id: String,
    reply: oneshot::Sender<PairDeviceResult>,
    endpoints: &HashMap<String, SessionEndpoint>,
    views: &ConnectedSessionViews,
) -> bool {
    let result = pair_device(&selection_id, endpoints, &views.status).await;
    let paired = result.outcome == PairDeviceOutcome::Paired;
    let _ = reply.send(result);
    paired
}

async fn forget_request<Sidecar, Store>(
    selection_id: String,
    reply: oneshot::Sender<ForgetDeviceResult>,
    endpoints: &HashMap<String, SessionEndpoint>,
    views: &ConnectedSessionViews,
    discovery: &mut DeviceDiscovery<Sidecar, Store>,
) where
    Sidecar: MuxSidecar,
    Store: PairingStore,
{
    let result = forget_device(&selection_id, endpoints, &views.status, discovery).await;
    let _ = reply.send(result);
}

pub(super) async fn perform_management_action<Sidecar, Store, HostPath>(
    selection_id: String,
    action: PendingManagementAction,
    endpoints: &HashMap<String, SessionEndpoint>,
    views: &ManagedSessionViews<HostPath>,
    discovery: &mut DeviceDiscovery<Sidecar, Store>,
) -> ManagementOutcome
where
    Sidecar: MuxSidecar,
    Store: PairingStore,
{
    match action {
        PendingManagementAction::Pair(reply) => {
            let requested = selection_id.clone();
            let paired = pair_request(selection_id, reply, endpoints, &views.connected).await;
            discovery.invalidate();
            if paired {
                ManagementOutcome::Connect(requested)
            } else {
                ManagementOutcome::None
            }
        }
        PendingManagementAction::Forget(reply) => {
            forget_request(
                selection_id.clone(),
                reply,
                endpoints,
                &views.connected,
                discovery,
            )
            .await;
            discovery.invalidate();
            ManagementOutcome::Remove(selection_id)
        }
        PendingManagementAction::UnmountDeveloperImage { status, reply } => {
            unmount_developer_image(selection_id, status, reply, endpoints, views).await;
            ManagementOutcome::None
        }
    }
}

async fn unmount_developer_image<HostPath>(
    selection_id: String,
    status: devicehub_core::DeveloperImageMountSlot,
    reply: oneshot::Sender<Result<(), String>>,
    endpoints: &HashMap<String, SessionEndpoint>,
    views: &ManagedSessionViews<HostPath>,
) {
    let managed_id = match views.connected.operations.begin(
        ManagedOperationKind::DeveloperImageUnmount,
        None,
        false,
    ) {
        Ok(id) => id,
        Err(error) => {
            let message = error.message;
            status.update(|current| {
                current.state = devicehub_core::DeveloperImageMountState::Failed;
                current.operation = Some(devicehub_core::DeveloperImageOperation::Unmount);
                current.error = Some(message.clone());
            });
            let _ = reply.send(Err(message));
            return;
        }
    };
    let result: Result<(), String> = async {
        let endpoint = endpoints
            .get(&selection_id)
            .cloned()
            .ok_or_else(|| "selected device transport is no longer available".to_string())?;
        let (provider, _) = connect_provider(endpoint).await?;
        unmount_image(provider.as_ref(), status.clone()).await?;
        Ok(())
    }
    .await;
    match &result {
        Ok(()) => {
            status.update(|current| {
                current.state = devicehub_core::DeveloperImageMountState::Unmounted;
                current.operation = None;
                current.progress_percent = Some(100.0);
                current.error = None;
            });
            views.connected.operations.succeed(managed_id);
        }
        Err(error) => {
            status.update(|current| {
                current.state = devicehub_core::DeveloperImageMountState::Failed;
                current.operation = Some(devicehub_core::DeveloperImageOperation::Unmount);
                current.error = Some(error.clone());
            });
            views.connected.operations.fail(
                managed_id,
                ManagedOperationError::new(OperationErrorCode::Internal, error.clone()),
            );
        }
    }
    let _ = reply.send(result);
}

pub(super) fn apply_management_outcome<HostPath>(
    outcome: ManagementOutcome,
    views: &SessionManagerViews<HostPath>,
    sessions: &mut HashMap<String, ManagedSessionViews<HostPath>>,
    pending_connect: &mut HashSet<String>,
) {
    match outcome {
        ManagementOutcome::None => {}
        ManagementOutcome::Connect(selection_id) => {
            pending_connect.insert(selection_id);
        }
        ManagementOutcome::Remove(selection_id) => {
            sessions.remove(&selection_id);
            views.sessions.remove(&selection_id);
            if views.active.selection_id().as_deref() == Some(selection_id.as_str()) {
                views.active.set(None);
            }
        }
    }
}
