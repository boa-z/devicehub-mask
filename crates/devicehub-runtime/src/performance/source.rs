//! Shared connection and demand gates for supervised DVT performance sources.

use std::time::Duration;

use idevice::dvt::remote_server::RemoteServerClient;
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use idevice::{ReadWrite, RsdService};
use tokio::sync::watch;

use crate::supervisor::ServiceReporter;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
pub(super) const SETUP_TIMEOUT: Duration = Duration::from_secs(8);

pub(super) async fn connect_remote(
    mut adapter: AdapterHandle,
    mut handshake: RsdHandshake,
) -> Result<RemoteServerClient<Box<dyn ReadWrite + 'static>>, String> {
    tokio::time::timeout(
        CONNECT_TIMEOUT,
        RemoteServerClient::connect_rsd(&mut adapter, &mut handshake),
    )
    .await
    .map_err(|_| "DVT performance connection timed out".to_string())?
    .map_err(|error| format!("DVT performance connection failed: {error:?}"))
}

pub(super) async fn wait_until_enabled(
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
