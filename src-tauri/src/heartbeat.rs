//! Supervised Lockdown heartbeat for long-running device sessions.

use std::sync::Arc;
use std::time::Duration;

use idevice::IdeviceService;
use idevice::provider::IdeviceProvider;
use idevice::services::heartbeat::HeartbeatClient;
use tokio::sync::watch;

use crate::supervisor::{ServiceReporter, reconnect_backoff, wait_for_retry};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
const INITIAL_HEARTBEAT_WAIT_SECS: u64 = 15;
const HEARTBEAT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);

pub async fn supervise(
    provider: Arc<dyn IdeviceProvider>,
    reporter: ServiceReporter,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut attempt = 0;
    loop {
        if *shutdown.borrow() {
            break;
        }
        attempt += 1;
        reporter.connecting(attempt);
        let result = run_once(provider.clone(), &reporter, attempt, &mut shutdown).await;
        if *shutdown.borrow() {
            break;
        }
        let Some(error) = result.err() else {
            continue;
        };
        reporter.retrying(attempt, error);
        if !wait_for_retry(&mut shutdown, reconnect_backoff(attempt - 1)).await {
            break;
        }
    }
    reporter.stopped(attempt);
}

async fn run_once(
    provider: Arc<dyn IdeviceProvider>,
    reporter: &ServiceReporter,
    attempt: u32,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    let mut client =
        tokio::time::timeout(CONNECT_TIMEOUT, HeartbeatClient::connect(provider.as_ref()))
            .await
            .map_err(|_| "device heartbeat connection timed out".to_string())?
            .map_err(|error| format!("device heartbeat connection failed: {error}"))?;
    let mut wait_secs = INITIAL_HEARTBEAT_WAIT_SECS;
    let mut ready = false;

    loop {
        let interval = tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
                continue;
            }
            result = client.get_marco(wait_secs) => {
                result.map_err(normalize_heartbeat_error)?
            }
        };
        tokio::time::timeout(HEARTBEAT_RESPONSE_TIMEOUT, client.send_polo())
            .await
            .map_err(|_| "device heartbeat response timed out".to_string())?
            .map_err(normalize_heartbeat_error)?;
        wait_secs = heartbeat_wait_secs(interval);
        if !ready {
            reporter.ready(attempt);
            ready = true;
        }
        tracing::debug!(
            interval_secs = interval,
            wait_secs,
            "device heartbeat acknowledged"
        );
    }
}

fn heartbeat_wait_secs(interval: u64) -> u64 {
    interval.saturating_add(5).clamp(5, 60)
}

fn normalize_heartbeat_error(error: idevice::IdeviceError) -> String {
    match error {
        idevice::IdeviceError::Heartbeat(idevice::HeartbeatError::SleepyTime) => {
            "device entered sleep".into()
        }
        idevice::IdeviceError::Heartbeat(idevice::HeartbeatError::Timeout) => {
            "device heartbeat timed out".into()
        }
        error => format!("device heartbeat failed: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use idevice::usbmuxd::{UsbmuxdAddr, UsbmuxdConnection};

    #[test]
    fn heartbeat_wait_adds_grace_and_bounds_device_values() {
        assert_eq!(heartbeat_wait_secs(0), 5);
        assert_eq!(heartbeat_wait_secs(10), 15);
        assert_eq!(heartbeat_wait_secs(55), 60);
        assert_eq!(heartbeat_wait_secs(u64::MAX), 60);
    }

    #[test]
    fn heartbeat_errors_are_reduced_to_actionable_messages() {
        assert_eq!(
            normalize_heartbeat_error(idevice::HeartbeatError::SleepyTime.into()),
            "device entered sleep"
        );
        assert_eq!(
            normalize_heartbeat_error(idevice::HeartbeatError::Timeout.into()),
            "device heartbeat timed out"
        );
    }

    #[tokio::test]
    #[ignore = "requires a connected physical device"]
    async fn acknowledges_heartbeat_from_hardware() {
        let mut usbmuxd = UsbmuxdConnection::default().await.unwrap();
        let device = usbmuxd
            .get_devices()
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("no connected device");
        let provider = device.to_provider(UsbmuxdAddr::default(), "devicehub-mask-heartbeat-test");
        let mut client = HeartbeatClient::connect(&provider).await.unwrap();
        let interval = tokio::time::timeout(
            Duration::from_secs(20),
            client.get_marco(INITIAL_HEARTBEAT_WAIT_SECS),
        )
        .await
        .expect("timed out waiting for heartbeat")
        .unwrap();
        client.send_polo().await.unwrap();
        assert!(heartbeat_wait_secs(interval) >= 5);
    }
}
