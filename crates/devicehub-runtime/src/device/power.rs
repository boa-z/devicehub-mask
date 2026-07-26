//! Serialized, bounded device power operations.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use idevice::{
    IdeviceService, diagnostics_relay::DiagnosticsRelayClient, provider::IdeviceProvider,
};
use tokio::sync::oneshot;

const POWER_COMMAND_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Copy)]
pub enum DevicePowerAction {
    Lock,
    Restart,
    Shutdown,
}

#[derive(Clone)]
pub struct DevicePowerController {
    provider: Arc<dyn IdeviceProvider>,
    slot: DevicePowerSlot,
}

impl DevicePowerController {
    pub fn new(provider: Arc<dyn IdeviceProvider>) -> Self {
        Self {
            provider,
            slot: DevicePowerSlot::default(),
        }
    }

    /// Start one power operation without blocking the session command loop.
    pub fn start(&self, action: DevicePowerAction, reply: oneshot::Sender<Result<(), String>>) {
        match self.slot.try_start() {
            Ok(lease) => spawn(self.provider.clone(), action, reply, lease),
            Err(error) => {
                let _ = reply.send(Err(error));
            }
        }
    }
}

#[derive(Clone, Default)]
struct DevicePowerSlot(Arc<AtomicBool>);

impl DevicePowerSlot {
    fn try_start(&self) -> Result<DevicePowerLease, String> {
        self.0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| DevicePowerLease(self.clone()))
            .map_err(|_| "another device power command is already running".into())
    }
}

struct DevicePowerLease(DevicePowerSlot);

impl Drop for DevicePowerLease {
    fn drop(&mut self) {
        self.0.0.store(false, Ordering::Release);
    }
}

fn spawn(
    provider: Arc<dyn IdeviceProvider>,
    action: DevicePowerAction,
    reply: oneshot::Sender<Result<(), String>>,
    _lease: DevicePowerLease,
) {
    tokio::spawn(async move {
        let result = tokio::time::timeout(POWER_COMMAND_TIMEOUT, async {
            let mut diagnostics = DiagnosticsRelayClient::connect(provider.as_ref())
                .await
                .map_err(|error| format!("cannot connect diagnostics relay: {error:?}"))?;
            match action {
                DevicePowerAction::Lock => diagnostics.sleep().await,
                DevicePowerAction::Restart => diagnostics.restart().await,
                DevicePowerAction::Shutdown => diagnostics.shutdown().await,
            }
            .map_err(|error| format!("device power command failed: {error:?}"))
        })
        .await
        .unwrap_or_else(|_| Err("device power command timed out".into()));
        match &result {
            Ok(()) => tracing::info!(?action, "device power command accepted"),
            Err(error) => tracing::warn!(?action, %error, "device power command failed"),
        }
        let _ = reply.send(result);
    });
}

#[cfg(test)]
mod tests {
    use super::DevicePowerSlot;

    #[test]
    fn slot_rejects_concurrent_commands_and_releases_on_drop() {
        let slot = DevicePowerSlot::default();
        let lease = slot.try_start().unwrap();
        assert!(slot.try_start().is_err());
        drop(lease);
        assert!(slot.try_start().is_ok());
    }
}
