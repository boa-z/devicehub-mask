//! On-demand, bounded device syslog collection.

use std::time::Duration;

use idevice::RsdService;
use idevice::os_trace_relay::{LogLevel, OsTraceRelayClient, OsTraceRelayReceiver};
use idevice::rsd::RsdHandshake;
use idevice::syslog_relay::SyslogRelayClient;
use idevice::tcp::handle::AdapterHandle;
use tokio::sync::watch;

use devicehub_core::{DeviceLogLevel, DeviceLogMetadata, DeviceLogSlot, DeviceLogSource};

use crate::supervisor::{ServiceReporter, reconnect_backoff, wait_for_retry};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
#[derive(Clone, Default)]
pub struct DeviceLogDemand(crate::demand::Demand);

impl DeviceLogDemand {
    pub fn set(&self, enabled: bool) {
        self.0.set(enabled);
    }

    pub fn enabled(&self) -> bool {
        self.0.enabled()
    }

    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.0.subscribe()
    }

    pub fn acquire(&self) -> crate::demand::DemandLease {
        self.0.acquire()
    }
}

pub(crate) async fn supervise_device_logs(
    mut adapter: AdapterHandle,
    mut handshake: RsdHandshake,
    slot: DeviceLogSlot,
    reporter: ServiceReporter,
    mut enabled: watch::Receiver<bool>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut attempt = 0;
    loop {
        if *shutdown.borrow() {
            break;
        }
        if !wait_until_enabled(&mut enabled, &mut shutdown, &reporter, attempt).await {
            break;
        }
        attempt += 1;
        reporter.connecting(attempt);
        let result = run_once(
            &mut adapter,
            &mut handshake,
            slot.clone(),
            &reporter,
            attempt,
            &mut enabled,
            &mut shutdown,
        )
        .await;
        if *shutdown.borrow() {
            break;
        }
        let Some(error) = result.err() else {
            slot.set_source(None);
            continue;
        };
        slot.set_source(None);
        reporter.retrying(attempt, error);
        if !wait_for_retry(&mut shutdown, reconnect_backoff(attempt - 1)).await {
            break;
        }
    }
    slot.set_source(None);
    reporter.stopped(attempt);
}

async fn run_once(
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
    slot: DeviceLogSlot,
    reporter: &ServiceReporter,
    attempt: u32,
    enabled: &mut watch::Receiver<bool>,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    let mut ready = false;
    let unified = tokio::time::timeout(CONNECT_TIMEOUT, async {
        let client = OsTraceRelayClient::connect_rsd(adapter, handshake).await?;
        client.start_trace(None).await
    })
    .await;
    match unified {
        Ok(Ok(receiver)) => {
            slot.set_source(Some(DeviceLogSource::Unified));
            reporter.ready(attempt);
            ready = true;
            match run_unified(receiver, slot.clone(), enabled, shutdown).await {
                Ok(()) => return Ok(()),
                Err(error) => {
                    tracing::warn!(
                        %error,
                        "unified device log stream failed; falling back to syslog relay"
                    );
                    slot.set_source(None);
                }
            }
        }
        Ok(Err(error)) => {
            tracing::info!(
                ?error,
                "unified device log unavailable; falling back to syslog relay"
            );
        }
        Err(_) => {
            tracing::info!("unified device log connection timed out; falling back to syslog relay");
        }
    }

    let mut client = tokio::time::timeout(
        CONNECT_TIMEOUT,
        SyslogRelayClient::connect_rsd(adapter, handshake),
    )
    .await
    .map_err(|_| "device syslog connection timed out".to_string())?
    .map_err(|error| format!("device syslog connection failed: {error:?}"))?;
    slot.set_source(Some(DeviceLogSource::Syslog));
    if !ready {
        reporter.ready(attempt);
    }
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
            changed = enabled.changed() => {
                if changed.is_err() || !*enabled.borrow() {
                    return Ok(());
                }
            }
            line = client.next() => match line {
                Ok(line) => slot.publish(line),
                Err(error) => return Err(format!("device syslog stream failed: {error:?}")),
            }
        }
    }
}

async fn run_unified(
    mut receiver: OsTraceRelayReceiver,
    slot: DeviceLogSlot,
    enabled: &mut watch::Receiver<bool>,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
            changed = enabled.changed() => {
                if changed.is_err() || !*enabled.borrow() {
                    return Ok(());
                }
            }
            log = receiver.next() => match log {
                Ok(log) => slot.publish_structured(log.message, DeviceLogMetadata {
                    level: Some(device_log_level(log.level)),
                    process: Some(log.image_name),
                    pid: Some(log.pid),
                    subsystem: log.label.as_ref().map(|label| label.subsystem.clone()),
                    category: log.label.as_ref().map(|label| label.category.clone()),
                    filename: Some(log.filename),
                }),
                Err(error) => return Err(format!("unified device log stream failed: {error:?}")),
            }
        }
    }
}

fn device_log_level(level: LogLevel) -> DeviceLogLevel {
    match level {
        LogLevel::Notice => DeviceLogLevel::Notice,
        LogLevel::Info => DeviceLogLevel::Info,
        LogLevel::Debug => DeviceLogLevel::Debug,
        LogLevel::Error => DeviceLogLevel::Error,
        LogLevel::Fault => DeviceLogLevel::Fault,
    }
}

async fn wait_until_enabled(
    enabled: &mut watch::Receiver<bool>,
    shutdown: &mut watch::Receiver<bool>,
    reporter: &ServiceReporter,
    attempt: u32,
) -> bool {
    while !*enabled.borrow() {
        reporter.stopped(attempt);
        tokio::select! {
            changed = enabled.changed() => {
                if changed.is_err() {
                    return false;
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return false;
                }
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use idevice::IdeviceService;
    use idevice::usbmuxd::{UsbmuxdAddr, UsbmuxdConnection};

    #[tokio::test]
    #[ignore = "requires a connected physical device"]
    async fn reads_syslog_from_hardware() {
        let mut usbmuxd = UsbmuxdConnection::default().await.unwrap();
        let device = usbmuxd
            .get_devices()
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("no connected device");
        let provider = device.to_provider(UsbmuxdAddr::default(), "devicehub-mask-device-log-test");
        let mut client = SyslogRelayClient::connect(&provider).await.unwrap();
        let line = tokio::time::timeout(Duration::from_secs(10), client.next())
            .await
            .expect("timed out waiting for syslog")
            .unwrap();
        let slot = DeviceLogSlot::default();
        slot.publish(line);
        assert!(!slot.snapshot(None, 1, true).entries.is_empty());
    }
}
