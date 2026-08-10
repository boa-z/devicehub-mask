//! Normalized connection status shared by private HTTP and WebSocket adapters.

use serde::Serialize;

use devicehub_core::{LocationStatus, Orientation, SessionPhase, SessionStatus};
use devicehub_runtime::{DeviceSessionClient, RuntimeClient};

#[derive(Serialize)]
struct DeviceView {
    id: String,
    udid: String,
    name: String,
    connection: &'static str,
    pairing: devicehub_core::DevicePairingState,
    session_status: Option<String>,
    session_phase: Option<SessionPhase>,
    session_updated_at_ms: Option<u64>,
    session_error: Option<String>,
    resources: Option<SessionResourcesView>,
}

#[derive(Serialize)]
struct SessionResourcesView {
    video: bool,
    audio: bool,
    performance: bool,
    device_logs: bool,
}

#[derive(Serialize)]
pub struct StatusView {
    status: String,
    phase: SessionPhase,
    updated_at_ms: u64,
    active_udid: Option<String>,
    active_device_id: Option<String>,
    error: Option<String>,
    orientation: &'static str,
    devices: Vec<DeviceView>,
    location: LocationStatus,
}

#[derive(Serialize)]
pub struct DeviceInventoryView {
    active_device_id: Option<String>,
    devices: Vec<DeviceView>,
}

pub fn inventory<HostPath>(application: &RuntimeClient<HostPath>) -> DeviceInventoryView {
    DeviceInventoryView {
        active_device_id: application.manager.active.selection_id(),
        devices: device_views(application),
    }
}

pub fn snapshot<HostPath>(application: &RuntimeClient<HostPath>) -> StatusView {
    let selection_id = application.manager.active.selection_id();
    let session = selection_id
        .as_deref()
        .and_then(|selection_id| application.sessions.get(selection_id));
    snapshot_with_session(application, selection_id, session.as_ref())
}

pub fn snapshot_for_session<HostPath>(
    application: &RuntimeClient<HostPath>,
    selection_id: &str,
    session: &DeviceSessionClient<HostPath>,
) -> StatusView {
    snapshot_with_session(application, Some(selection_id.to_string()), Some(session))
}

fn snapshot_with_session<HostPath>(
    application: &RuntimeClient<HostPath>,
    selection_id: Option<String>,
    session: Option<&DeviceSessionClient<HostPath>>,
) -> StatusView {
    let legacy = &application.device;
    let session = session.unwrap_or(legacy);
    let active_udid = selection_id.as_deref().and_then(|selection_id| {
        application
            .manager
            .devices
            .get()
            .into_iter()
            .find(|device| device.id == selection_id)
            .map(|device| device.udid)
    });
    let SessionStatus {
        mut phase,
        message,
        updated_at_ms,
    } = session.status.snapshot();
    let error = session.error.get();
    if error.is_some() && phase == SessionPhase::Disconnected {
        phase = SessionPhase::Failed;
    }
    StatusView {
        status: message,
        phase,
        updated_at_ms,
        active_udid,
        active_device_id: selection_id,
        error,
        orientation: orientation_name(session.orientation.get()),
        devices: device_views(application),
        location: session.location.get(),
    }
}

fn device_views<HostPath>(application: &RuntimeClient<HostPath>) -> Vec<DeviceView> {
    application
        .manager
        .devices
        .get()
        .into_iter()
        .map(|device| {
            let device_session = application.sessions.get(&device.id);
            let session_status = device_session
                .as_ref()
                .map(|session| session.status.snapshot());
            let session_error = device_session
                .as_ref()
                .and_then(|session| session.error.get());
            let resources = device_session.as_ref().map(|session| SessionResourcesView {
                video: session.media_demand.video.enabled(),
                audio: session.media_demand.audio.enabled(),
                performance: session.performance_demand.enabled(),
                device_logs: session.device_log_demand.enabled(),
            });
            DeviceView {
                id: device.id,
                udid: device.udid,
                name: device.name,
                connection: device.connection.label(),
                pairing: device.pairing,
                session_status: session_status.as_ref().map(|status| status.message.clone()),
                session_phase: session_status.as_ref().map(|status| {
                    if session_error.is_some() && status.phase == SessionPhase::Disconnected {
                        SessionPhase::Failed
                    } else {
                        status.phase
                    }
                }),
                session_updated_at_ms: session_status.map(|status| status.updated_at_ms),
                session_error,
                resources,
            }
        })
        .collect()
}

fn orientation_name(orientation: Orientation) -> &'static str {
    match orientation {
        Orientation::Portrait => "portrait",
        Orientation::PortraitUpsideDown => "portrait_upside_down",
        Orientation::LandscapeLeft => "landscape_left",
        Orientation::LandscapeRight => "landscape_right",
    }
}

#[cfg(test)]
mod tests {
    use super::{inventory, snapshot};
    use devicehub_core::{ConnKind, DeviceInfo, SessionPhase, StatusSlot};

    #[test]
    fn snapshot_accepts_an_opaque_host_path_type() {
        let (client, _control) =
            devicehub_runtime::RuntimeClientFixture::<String>::default().build();
        client
            .device
            .status
            .set_phase(devicehub_core::SessionPhase::Connected, "connected");

        let status = snapshot(&client);

        assert_eq!(status.status, "connected");
        assert!(status.active_device_id.is_none());
    }

    #[test]
    fn snapshot_projects_structured_status_for_each_device() {
        let session_status = StatusSlot::default();
        session_status.set_phase(SessionPhase::Recovering, "retrying connection");
        let (session_client, _session_control) =
            devicehub_runtime::RuntimeClientFixture::<String>::default()
                .with_status(session_status)
                .build();
        let (client, _control) = devicehub_runtime::RuntimeClientFixture::<String>::default()
            .with_session("device-1::usb", session_client.device)
            .build();
        client.manager.devices.set(vec![DeviceInfo {
            id: "device-1::usb".into(),
            udid: "device-1".into(),
            name: "Test iPhone".into(),
            connection: ConnKind::Usb,
            pairing: devicehub_core::DevicePairingState::Paired,
        }]);

        let status = snapshot(&client);
        let device = status.devices.first().expect("device status");
        assert_eq!(device.session_phase, Some(SessionPhase::Recovering));
        assert_eq!(
            device.session_status.as_deref(),
            Some("retrying connection")
        );
        assert!(device.session_updated_at_ms.is_some());
        let resources = device.resources.as_ref().expect("session resources");
        assert!(!resources.video);
        assert!(!resources.audio);

        let inventory = inventory(&client);
        assert_eq!(inventory.devices.len(), 1);
        assert_eq!(
            inventory.devices[0].session_phase,
            Some(SessionPhase::Recovering)
        );
    }
}
