//! Bonjour advertisement for DeviceHub LAN clients.
//!
//! The record deliberately contains no bearer credential. It only lets a
//! mobile client find a listener that the user has explicitly published with
//! `--allow-lan`; authentication still happens through the access token.

use mdns_sd::{ServiceDaemon, ServiceInfo};

pub const SERVICE_TYPE: &str = "_devicehub._tcp.local.";
const SERVICE_NAME: &str = "DeviceHub Mask";
const HOST_NAME: &str = "devicehub-mask.local.";

pub struct ServiceAdvertiser {
    daemon: ServiceDaemon,
    fullname: String,
}

impl ServiceAdvertiser {
    pub fn start(port: u16) -> Result<Self, String> {
        let daemon = ServiceDaemon::new().map_err(|error| {
            format!("cannot initialize DeviceHub Bonjour advertisement: {error}")
        })?;
        let properties = [
            ("protocol", "1"),
            ("targets", "ios"),
            ("transport", "http-ws"),
        ];
        let service = ServiceInfo::new(
            SERVICE_TYPE,
            SERVICE_NAME,
            HOST_NAME,
            (),
            port,
            &properties[..],
        )
        .map_err(|error| format!("cannot construct DeviceHub Bonjour record: {error}"))?
        .enable_addr_auto();
        let fullname = service.get_fullname().to_owned();
        daemon
            .register(service)
            .map_err(|error| format!("cannot publish DeviceHub Bonjour record: {error}"))?;
        tracing::info!(service_type = SERVICE_TYPE, %fullname, port, "DeviceHub LAN discovery published");
        Ok(Self { daemon, fullname })
    }
}

impl Drop for ServiceAdvertiser {
    fn drop(&mut self) {
        if let Err(error) = self.daemon.unregister(&self.fullname) {
            tracing::debug!(%error, fullname = %self.fullname, "unable to withdraw DeviceHub Bonjour record");
        }
        if let Err(error) = self.daemon.shutdown() {
            tracing::debug!(%error, "unable to stop DeviceHub Bonjour daemon");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SERVICE_TYPE;

    #[test]
    fn advertises_the_stable_client_service_type() {
        assert_eq!(SERVICE_TYPE, "_devicehub._tcp.local.");
    }
}
