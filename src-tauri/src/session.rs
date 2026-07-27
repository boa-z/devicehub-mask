//! Desktop composition for runtime-owned device sessions and host adapters.

mod clipboard;
mod manager;

pub(crate) use manager::start as start_manager;

use devicehub_runtime::SystemUsbmuxdConfig;

#[derive(Clone, Debug)]
pub(crate) struct DeviceTransportConfig {
    pub(crate) netmuxd: devicehub_host::netmuxd::NetmuxdConfig,
    pub(crate) system_usbmuxd: SystemUsbmuxdConfig,
}

impl DeviceTransportConfig {
    pub(crate) fn from_host(
        netmuxd: devicehub_host::netmuxd::NetmuxdConfig,
        system_usbmuxd: Option<String>,
    ) -> Self {
        Self {
            netmuxd,
            system_usbmuxd: SystemUsbmuxdConfig::from_host(system_usbmuxd),
        }
    }
}
