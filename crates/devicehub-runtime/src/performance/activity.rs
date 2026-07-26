//! Bounded application lifecycle observations from DVT notifications.

use idevice::ReadWrite;
use idevice::dvt::notifications::NotificationsClient;
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use tokio::sync::watch;

use super::PerformanceSlot;
use super::source::{SETUP_TIMEOUT, connect_remote, wait_until_enabled};
use crate::supervisor::{ServiceReporter, reconnect_backoff, wait_for_retry};

pub async fn supervise_performance_app_activity(
    adapter: AdapterHandle,
    handshake: RsdHandshake,
    slot: PerformanceSlot,
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
            adapter.clone(),
            handshake.clone(),
            slot.clone(),
            &mut shutdown,
            &mut enabled,
            &reporter,
            attempt,
        )
        .await;
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
    adapter: AdapterHandle,
    handshake: RsdHandshake,
    slot: PerformanceSlot,
    shutdown: &mut watch::Receiver<bool>,
    enabled: &mut watch::Receiver<bool>,
    reporter: &ServiceReporter,
    attempt: u32,
) -> Result<(), String> {
    let mut remote = connect_remote(adapter, handshake).await?;
    let mut client = NotificationsClient::new(&mut remote)
        .await
        .map_err(|error| format!("DVT app activity channel failed: {error:?}"))?;
    tokio::time::timeout(SETUP_TIMEOUT, client.start_notifications())
        .await
        .map_err(|_| "DVT app activity setup timed out".to_string())?
        .map_err(|error| format!("DVT app activity setup failed: {error:?}"))?;
    reporter.ready(attempt);
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    stop(&mut client).await;
                    return Ok(());
                }
            }
            changed = enabled.changed() => {
                if changed.is_err() || !*enabled.borrow() {
                    stop(&mut client).await;
                    return Ok(());
                }
            }
            notification = client.get_notification() => match notification {
                Ok(notification) => slot.publish_app_activity(notification),
                Err(error) => return Err(format!("DVT app activity stream failed: {error:?}")),
            }
        }
    }
}

async fn stop<R: ReadWrite>(client: &mut NotificationsClient<'_, R>) {
    let _ = tokio::time::timeout(SETUP_TIMEOUT, client.stop_notifications()).await;
}
