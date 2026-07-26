//! Best-effort observation of the device's SpringBoard interface orientation.

use std::time::Duration;

use devicehub_core::{Orientation, OrientationSlot};
use idevice::{
    IdeviceError, RsdService,
    rsd::RsdHandshake,
    springboardservices::{InterfaceOrientation, SpringBoardServicesClient},
    tcp::handle::AdapterHandle,
};

const POLL_INTERVAL: Duration = Duration::from_millis(500);

pub(crate) struct OrientationWatcher(SpringBoardServicesClient);

impl OrientationWatcher {
    /// Connect and seed the current interface orientation. Screen streaming can
    /// continue when SpringBoard is unavailable, so absence is represented by
    /// `None` instead of failing the device session.
    pub(crate) async fn connect(
        adapter: &mut AdapterHandle,
        handshake: &mut RsdHandshake,
        view: &OrientationSlot,
    ) -> Option<Self> {
        match SpringBoardServicesClient::connect_rsd(adapter, handshake).await {
            Ok(mut client) => {
                if let Err(error) = refresh(&mut client, view).await {
                    tracing::warn!(
                        "could not read initial device interface orientation: {error:?}"
                    );
                }
                Some(Self(client))
            }
            Err(error) => {
                tracing::warn!(
                    "no SpringBoard orientation service; using rotation command state: {error:?}"
                );
                None
            }
        }
    }

    pub(crate) async fn run(mut self, view: OrientationSlot) {
        let mut reported_error = false;
        loop {
            match refresh(&mut self.0, &view).await {
                Ok(()) => reported_error = false,
                Err(error) if !reported_error => {
                    tracing::warn!("could not refresh device interface orientation: {error:?}");
                    reported_error = true;
                }
                Err(_) => {}
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

fn from_interface(orientation: InterfaceOrientation) -> Option<Orientation> {
    match orientation {
        InterfaceOrientation::Portrait => Some(Orientation::Portrait),
        InterfaceOrientation::PortraitUpsideDown => Some(Orientation::PortraitUpsideDown),
        // SpringBoard labels the opposite landscape edge from the screen stream.
        InterfaceOrientation::LandscapeLeft => Some(Orientation::LandscapeRight),
        InterfaceOrientation::LandscapeRight => Some(Orientation::LandscapeLeft),
        InterfaceOrientation::Unknown => None,
    }
}

async fn refresh(
    springboard: &mut SpringBoardServicesClient,
    view: &OrientationSlot,
) -> Result<(), IdeviceError> {
    let Some(orientation) = from_interface(springboard.get_interface_orientation().await?) else {
        return Ok(());
    };

    if view.get() != orientation {
        tracing::info!(?orientation, "device interface orientation changed");
        view.set(orientation);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use idevice::springboardservices::InterfaceOrientation;

    use super::from_interface;
    use devicehub_core::Orientation;

    #[test]
    fn maps_springboard_interface_orientations() {
        assert_eq!(
            from_interface(InterfaceOrientation::Portrait),
            Some(Orientation::Portrait)
        );
        assert_eq!(
            from_interface(InterfaceOrientation::PortraitUpsideDown),
            Some(Orientation::PortraitUpsideDown)
        );
        assert_eq!(
            from_interface(InterfaceOrientation::LandscapeLeft),
            Some(Orientation::LandscapeRight)
        );
        assert_eq!(
            from_interface(InterfaceOrientation::LandscapeRight),
            Some(Orientation::LandscapeLeft)
        );
        assert_eq!(from_interface(InterfaceOrientation::Unknown), None);
    }
}
