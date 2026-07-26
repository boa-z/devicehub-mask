//! Explicit USB trust management for runtime-owned device discovery.
//!
//! Pairing and revocation are user-triggered operations, separate from opening a
//! transport. This service owns their deadlines and stable public outcomes. It
//! also coordinates local credential cleanup with discovery without exposing the
//! Wi-Fi pairing cache implementation to the session manager.

use std::collections::HashMap;
use std::time::Duration;

use idevice::{IdeviceError, IdeviceService, lockdown::LockdownClient, usbmuxd::Connection};

use devicehub_core::{
    ForgetDeviceOutcome, ForgetDeviceResult, PairDeviceOutcome, PairDeviceResult, StatusSlot,
    device_id_fingerprint,
};

use crate::{SessionEndpoint, UsbmuxdEndpoint};

/// Pairing includes an on-device confirmation and therefore gets a user-facing
/// deadline rather than inheriting a short transport timeout.
const PAIRING_TIMEOUT: Duration = Duration::from_secs(90);
/// Revocation performs bounded device and local I/O without a confirmation UI.
const FORGET_DEVICE_TIMEOUT: Duration = Duration::from_secs(10);

/// Host credential cleanup required after device and usbmuxd revocation.
pub trait PairingCredentialStore {
    fn remove_cached_pairing(&mut self, udid: &str) -> Result<(), String>;
}

pub async fn pair_device(
    selection_id: &str,
    endpoints: &HashMap<String, SessionEndpoint>,
    status: &StatusSlot,
) -> PairDeviceResult {
    match endpoints.get(selection_id) {
        Some(SessionEndpoint::Usbmuxd(endpoint)) => {
            status.set("waiting for device trust confirmation...");
            pair_usb_endpoint(endpoint).await
        }
        Some(SessionEndpoint::Wifi(_)) => PairDeviceResult {
            outcome: PairDeviceOutcome::Failed,
            error: Some("pairing is available only for a USB device".into()),
        },
        None => PairDeviceResult {
            outcome: PairDeviceOutcome::Failed,
            error: Some("the selected USB device is no longer available".into()),
        },
    }
}

pub async fn forget_device<Store>(
    selection_id: &str,
    endpoints: &HashMap<String, SessionEndpoint>,
    status: &StatusSlot,
    credentials: &mut Store,
) -> ForgetDeviceResult
where
    Store: PairingCredentialStore,
{
    match endpoints.get(selection_id) {
        Some(SessionEndpoint::Usbmuxd(endpoint)) => {
            status.set("removing device trust...");
            forget_usb_endpoint(endpoint, credentials).await
        }
        Some(SessionEndpoint::Wifi(_)) => ForgetDeviceResult {
            outcome: ForgetDeviceOutcome::Failed,
            error: Some("removing trust is available only for a USB device".into()),
        },
        None => ForgetDeviceResult {
            outcome: ForgetDeviceOutcome::Failed,
            error: Some("the selected USB device is no longer available".into()),
        },
    }
}

async fn pair_usb_endpoint(endpoint: &UsbmuxdEndpoint) -> PairDeviceResult {
    if !matches!(endpoint.device.connection_type, Connection::Usb) {
        return PairDeviceResult {
            outcome: PairDeviceOutcome::Failed,
            error: Some("pairing is available only for a USB device".into()),
        };
    }

    let device_id = device_id_fingerprint(&endpoint.device.udid);
    tracing::info!(%device_id, "USB pairing requested by user");
    let operation = async {
        let mut usbmuxd = endpoint.address.connect(0).await?;
        let system_buid = usbmuxd.get_buid().await?;
        let provider = endpoint
            .device
            .to_provider(endpoint.address.clone(), "devicehub-mask-pairing");
        let mut lockdown = LockdownClient::connect(&provider).await?;
        let host_id = uuid::Uuid::new_v4().to_string().to_uppercase();
        let mut pairing_file = lockdown
            .pair(host_id, system_buid, Some("DeviceHub Mask"))
            .await?;

        // Credentials become durable only after the device accepts them and a
        // secure Lockdown session proves the generated record is usable.
        lockdown.start_session(&pairing_file).await?;
        pairing_file.udid = Some(endpoint.device.udid.clone());
        let serialized = pairing_file.serialize()?;
        usbmuxd
            .save_pair_record(&endpoint.device.udid, serialized)
            .await?;
        Ok::<(), IdeviceError>(())
    };

    match tokio::time::timeout(PAIRING_TIMEOUT, operation).await {
        Ok(Ok(())) => {
            tracing::info!(%device_id, "USB pairing completed");
            PairDeviceResult {
                outcome: PairDeviceOutcome::Paired,
                error: None,
            }
        }
        Ok(Err(error)) => {
            tracing::warn!(%device_id, ?error, "USB pairing failed");
            pairing_failure(error)
        }
        Err(_) => {
            tracing::warn!(%device_id, timeout_ms = PAIRING_TIMEOUT.as_millis(), "USB pairing timed out");
            PairDeviceResult {
                outcome: PairDeviceOutcome::TimedOut,
                error: Some("timed out waiting for the device trust confirmation".into()),
            }
        }
    }
}

fn pairing_failure(error: IdeviceError) -> PairDeviceResult {
    let outcome = match error {
        IdeviceError::UserDeniedPairing => PairDeviceOutcome::Denied,
        IdeviceError::PasswordProtected | IdeviceError::DeviceLocked => PairDeviceOutcome::Locked,
        _ => PairDeviceOutcome::Failed,
    };
    PairDeviceResult {
        outcome,
        error: Some(error.to_string()),
    }
}

async fn forget_usb_endpoint<Store>(
    endpoint: &UsbmuxdEndpoint,
    credentials: &mut Store,
) -> ForgetDeviceResult
where
    Store: PairingCredentialStore,
{
    if !matches!(endpoint.device.connection_type, Connection::Usb) {
        return ForgetDeviceResult {
            outcome: ForgetDeviceOutcome::Failed,
            error: Some("removing trust is available only for a USB device".into()),
        };
    }

    let device_id = device_id_fingerprint(&endpoint.device.udid);
    tracing::info!(%device_id, "USB trust removal requested by user");
    let pairing_record = tokio::time::timeout(FORGET_DEVICE_TIMEOUT, async {
        let mut usbmuxd = endpoint.address.connect(0).await?;
        usbmuxd.get_pair_record(&endpoint.device.udid).await
    })
    .await
    .map_err(|_| IdeviceError::Timeout)
    .and_then(|result| result);

    let device_error = match pairing_record {
        Ok(pairing_file) => {
            let revoke = async {
                let provider = endpoint
                    .device
                    .to_provider(endpoint.address.clone(), "devicehub-mask-unpairing");
                let mut lockdown = LockdownClient::connect(&provider).await?;
                lockdown.unpair(pairing_file.host_id).await
            };
            match tokio::time::timeout(FORGET_DEVICE_TIMEOUT, revoke).await {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(error.to_string()),
                Err(_) => Some(IdeviceError::Timeout.to_string()),
            }
        }
        Err(error) => Some(error.to_string()),
    };

    // An explicit forget must remove host private-key material even when the
    // device response is lost or the device had already revoked this host.
    let pair_record_error = delete_host_pair_record(endpoint)
        .await
        .err()
        .map(|error| error.to_string());
    let cache_error = credentials
        .remove_cached_pairing(&endpoint.device.udid)
        .err();
    let host_error = match (pair_record_error, cache_error) {
        (Some(pair_record), Some(cache)) => Some(format!(
            "usbmuxd record removal failed: {pair_record}; cached record removal failed: {cache}"
        )),
        (Some(pair_record), None) => Some(format!("usbmuxd record removal failed: {pair_record}")),
        (None, Some(cache)) => Some(format!("cached record removal failed: {cache}")),
        (None, None) => None,
    };
    let result = forget_device_result(device_error, host_error);
    if result.outcome == ForgetDeviceOutcome::Forgotten {
        tracing::info!(%device_id, "USB trust relationship removed");
    } else {
        tracing::warn!(%device_id, outcome = ?result.outcome, error = ?result.error, "USB trust removal completed with an incomplete result");
    }
    result
}

async fn delete_host_pair_record(endpoint: &UsbmuxdEndpoint) -> Result<(), IdeviceError> {
    let delete = async {
        let mut usbmuxd = endpoint.address.connect(0).await?;
        usbmuxd.delete_pair_record(&endpoint.device.udid).await
    };
    match tokio::time::timeout(FORGET_DEVICE_TIMEOUT, delete).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(first_error)) => {
            tracing::debug!(?first_error, "retrying host pairing record removal");
            let retry = async {
                let mut usbmuxd = endpoint.address.connect(0).await?;
                usbmuxd.delete_pair_record(&endpoint.device.udid).await
            };
            tokio::time::timeout(FORGET_DEVICE_TIMEOUT, retry)
                .await
                .map_err(|_| IdeviceError::Timeout)?
        }
        Err(_) => Err(IdeviceError::Timeout),
    }
}

fn forget_device_result(
    device_error: Option<String>,
    host_error: Option<String>,
) -> ForgetDeviceResult {
    let outcome = match (device_error.is_some(), host_error.is_some()) {
        (false, false) => ForgetDeviceOutcome::Forgotten,
        (true, false) => ForgetDeviceOutcome::HostRecordRemoved,
        (false, true) => ForgetDeviceOutcome::DeviceForgottenHostCleanupFailed,
        (true, true) => ForgetDeviceOutcome::Failed,
    };
    let error = match (device_error, host_error) {
        (Some(device), Some(host)) => Some(format!(
            "device did not confirm revocation: {device}; host record cleanup failed: {host}"
        )),
        (Some(device), None) => Some(format!("device did not confirm revocation: {device}")),
        (None, Some(host)) => Some(format!("host record cleanup failed: {host}")),
        (None, None) => None,
    };
    ForgetDeviceResult { outcome, error }
}

#[cfg(test)]
mod tests {
    use idevice::IdeviceError;

    use super::{forget_device_result, pairing_failure};
    use devicehub_core::{ForgetDeviceOutcome, PairDeviceOutcome};

    #[test]
    fn pairing_errors_are_normalized_for_the_frontend() {
        assert_eq!(
            pairing_failure(IdeviceError::UserDeniedPairing).outcome,
            PairDeviceOutcome::Denied
        );
        assert_eq!(
            pairing_failure(IdeviceError::PasswordProtected).outcome,
            PairDeviceOutcome::Locked
        );
        assert_eq!(
            pairing_failure(IdeviceError::DeviceLocked).outcome,
            PairDeviceOutcome::Locked
        );
        assert_eq!(
            pairing_failure(IdeviceError::DeviceNotFound).outcome,
            PairDeviceOutcome::Failed
        );
    }

    #[test]
    fn trust_removal_preserves_partial_success() {
        assert_eq!(
            forget_device_result(None, None).outcome,
            ForgetDeviceOutcome::Forgotten
        );
        assert_eq!(
            forget_device_result(Some("device unavailable".into()), None).outcome,
            ForgetDeviceOutcome::HostRecordRemoved
        );
        assert_eq!(
            forget_device_result(None, Some("host cleanup failed".into())).outcome,
            ForgetDeviceOutcome::DeviceForgottenHostCleanupFailed
        );
        let failed = forget_device_result(
            Some("device unavailable".into()),
            Some("host cleanup failed".into()),
        );
        assert_eq!(failed.outcome, ForgetDeviceOutcome::Failed);
        assert!(failed.error.unwrap().contains("host record cleanup failed"));
    }
}
