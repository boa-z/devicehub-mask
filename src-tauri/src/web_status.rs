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
pub(crate) struct StatusView {
    status: String,
    active_udid: Option<String>,
    active_device_id: Option<String>,
    error: Option<String>,
    orientation: &'static str,
    devices: Vec<DeviceView>,
    location: LocationStatus,
}

pub(crate) fn snapshot(application: &RuntimeClient<std::path::PathBuf>) -> StatusView {
    StatusView {
        status: application.status.get(),
        active_udid: application.active.get(),
        active_device_id: application.active.selection_id(),
        error: application.error.get(),
        orientation: orientation_name(application.orientation.get()),
        devices: application
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
        location: application.location.get(),
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
