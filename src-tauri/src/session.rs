//! Desktop composition for device-session discovery and host adapters.
//!
//! Connected-session protocol and lifecycle ownership live in
//! `devicehub-runtime`; this module retains only outer discovery/trust work that
//! has not yet migrated and physical-device diagnostics.

mod clipboard;
mod diagnostics;
mod manager;
mod services;

pub(crate) use manager::manage;

use devicehub_runtime::SystemUsbmuxdConfig;
#[cfg(test)]
use devicehub_runtime::read_device_developer_mode_status;

#[derive(Clone, Debug)]
pub(crate) struct DeviceTransportConfig {
    pub(crate) netmuxd: crate::netmuxd::NetmuxdConfig,
    pub(crate) system_usbmuxd: SystemUsbmuxdConfig,
}

impl DeviceTransportConfig {
    pub(crate) fn from_host(
        netmuxd: crate::netmuxd::NetmuxdConfig,
        system_usbmuxd: Option<String>,
    ) -> Self {
        Self {
            netmuxd,
            system_usbmuxd: SystemUsbmuxdConfig::from_host(system_usbmuxd),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devicehub_runtime::{
        SessionEndpoint, UsbmuxdEndpoint, connect_provider, select_preferred_usbmuxd_device,
    };
    use idevice::usbmuxd::{UsbmuxdAddr, UsbmuxdConnection};

    #[tokio::test]
    #[ignore = "requires a connected physical device"]
    async fn reads_developer_mode_status_from_hardware() {
        let mut usbmuxd = UsbmuxdConnection::default().await.expect("connect usbmuxd");
        let devices = usbmuxd.get_devices().await.expect("list devices");
        let endpoint = SessionEndpoint::Usbmuxd(Box::new(UsbmuxdEndpoint {
            device: select_preferred_usbmuxd_device(devices, None).expect("connected device"),
            address: UsbmuxdAddr::default(),
        }));
        let (provider, _) = connect_provider(endpoint)
            .await
            .expect("connect device provider");
        let enabled = read_device_developer_mode_status(provider.as_ref())
            .await
            .expect("query developer mode");
        eprintln!("developer mode enabled: {enabled}");
    }
}
