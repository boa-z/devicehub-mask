//! Normalized connection status shared by private HTTP and WebSocket adapters.

use serde::Serialize;

use devicehub_core::{LocationStatus, Orientation};
use devicehub_runtime::RuntimeClient;

#[derive(Serialize)]
struct DeviceView {
    id: String,
    udid: String,
    name: String,
    connection: &'static str,
    pairing: devicehub_core::DevicePairingState,
}

#[derive(Serialize)]
pub struct StatusView {
    status: String,
    active_udid: Option<String>,
    active_device_id: Option<String>,
    error: Option<String>,
    orientation: &'static str,
    devices: Vec<DeviceView>,
    location: LocationStatus,
}

pub fn snapshot<HostPath>(application: &RuntimeClient<HostPath>) -> StatusView {
    StatusView {
        status: application.device.status.get(),
        active_udid: application.manager.active.get(),
        active_device_id: application.manager.active.selection_id(),
        error: application.device.error.get(),
        orientation: orientation_name(application.device.orientation.get()),
        devices: application
            .manager
            .devices
            .get()
            .into_iter()
            .map(|device| DeviceView {
                id: device.id,
                udid: device.udid,
                name: device.name,
                connection: device.connection.label(),
                pairing: device.pairing,
            })
            .collect(),
        location: application.device.location.get(),
    }
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
    use super::snapshot;

    #[test]
    fn snapshot_accepts_an_opaque_host_path_type() {
        let (client, _control) =
            devicehub_runtime::RuntimeClientFixture::<String>::default().build();
        client.device.status.set("connected");

        let status = snapshot(&client);

        assert_eq!(status.status, "connected");
        assert!(status.active_device_id.is_none());
    }
}
