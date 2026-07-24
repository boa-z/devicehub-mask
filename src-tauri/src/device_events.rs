//! Event-driven device metadata changes from the Lockdown notification proxy.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use idevice::RsdService;
use idevice::rsd::RsdHandshake;
use idevice::services::notification_proxy::NotificationProxyClient;
use idevice::tcp::handle::AdapterHandle;
use serde::Serialize;
use tokio::sync::{broadcast, watch};

use crate::supervisor::{ServiceReporter, reconnect_backoff, wait_for_retry};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
const COALESCED_EVENT_INTERVAL: Duration = Duration::from_secs(1);
const OBSERVED_NOTIFICATIONS: &[&str] = &[
    "com.apple.mobile.application_installed",
    "com.apple.mobile.application_uninstalled",
    "com.apple.mobile.lockdown.activation_state",
    "com.apple.mobile.lockdown.disk_usage_changed",
    "com.apple.mobile.lockdown.device_name_changed",
    "com.apple.mobile.lockdown.timezone_changed",
    "com.apple.language.changed",
    "com.apple.mobile.developer_image_mounted",
    "com.apple.springboard.lockstate",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceEventKind {
    AppInstalled,
    AppUninstalled,
    ActivationStateChanged,
    DiskUsageChanged,
    DeviceNameChanged,
    RegionalSettingsChanged,
    DeveloperImageMounted,
    LockStateChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DeviceEvent {
    pub sequence: u64,
    pub kind: DeviceEventKind,
}

struct DeviceEventSlotInner {
    sender: broadcast::Sender<DeviceEvent>,
    sequence: AtomicU64,
    latest: Mutex<Option<DeviceEvent>>,
}

#[derive(Clone)]
pub struct DeviceEventSlot(Arc<DeviceEventSlotInner>);

impl Default for DeviceEventSlot {
    fn default() -> Self {
        let (sender, _) = broadcast::channel(16);
        Self(Arc::new(DeviceEventSlotInner {
            sender,
            sequence: AtomicU64::new(0),
            latest: Mutex::new(None),
        }))
    }
}

impl DeviceEventSlot {
    pub fn publish(&self, kind: DeviceEventKind) {
        let sequence = self.0.sequence.fetch_add(1, Ordering::Relaxed) + 1;
        let event = DeviceEvent { sequence, kind };
        *self.0.latest.lock().unwrap() = Some(event);
        let _ = self.0.sender.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<DeviceEvent> {
        self.0.sender.subscribe()
    }

    pub fn latest(&self) -> Option<DeviceEvent> {
        *self.0.latest.lock().unwrap()
    }

    pub fn reset(&self) {
        *self.0.latest.lock().unwrap() = None;
    }
}

fn normalize_notification(name: &str) -> Option<DeviceEventKind> {
    match name {
        "com.apple.mobile.application_installed" => Some(DeviceEventKind::AppInstalled),
        "com.apple.mobile.application_uninstalled" => Some(DeviceEventKind::AppUninstalled),
        "com.apple.mobile.lockdown.activation_state" => {
            Some(DeviceEventKind::ActivationStateChanged)
        }
        "com.apple.mobile.lockdown.disk_usage_changed" => Some(DeviceEventKind::DiskUsageChanged),
        "com.apple.mobile.lockdown.device_name_changed" => Some(DeviceEventKind::DeviceNameChanged),
        "com.apple.mobile.lockdown.timezone_changed" | "com.apple.language.changed" => {
            Some(DeviceEventKind::RegionalSettingsChanged)
        }
        "com.apple.mobile.developer_image_mounted" => Some(DeviceEventKind::DeveloperImageMounted),
        "com.apple.springboard.lockstate" => Some(DeviceEventKind::LockStateChanged),
        _ => None,
    }
}

fn should_publish(
    kind: DeviceEventKind,
    last_disk_usage_event: &mut Option<Instant>,
    last_regional_event: &mut Option<Instant>,
    now: Instant,
) -> bool {
    let last_event = match kind {
        DeviceEventKind::DiskUsageChanged => last_disk_usage_event,
        DeviceEventKind::RegionalSettingsChanged => last_regional_event,
        _ => return true,
    };
    if last_event.is_some_and(|last| now.saturating_duration_since(last) < COALESCED_EVENT_INTERVAL)
    {
        false
    } else {
        *last_event = Some(now);
        true
    }
}

pub async fn supervise(
    mut adapter: AdapterHandle,
    mut handshake: RsdHandshake,
    events: DeviceEventSlot,
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
        let result = run_once(
            &mut adapter,
            &mut handshake,
            events.clone(),
            &reporter,
            attempt,
            &mut shutdown,
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
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
    events: DeviceEventSlot,
    reporter: &ServiceReporter,
    attempt: u32,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    let mut client = tokio::time::timeout(
        CONNECT_TIMEOUT,
        NotificationProxyClient::connect_rsd(adapter, handshake),
    )
    .await
    .map_err(|_| "device notification connection timed out".to_string())?
    .map_err(|error| format!("device notification connection failed: {error:?}"))?;
    tokio::time::timeout(
        CONNECT_TIMEOUT,
        client.observe_notifications(OBSERVED_NOTIFICATIONS),
    )
    .await
    .map_err(|_| "device notification registration timed out".to_string())?
    .map_err(|error| format!("device notification registration failed: {error:?}"))?;
    reporter.ready(attempt);
    let mut last_disk_usage_event = None;
    let mut last_regional_event = None;

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
            notification = client.receive_notification() => {
                let name = notification
                    .map_err(|error| format!("device notification stream failed: {error:?}"))?;
                if let Some(kind) = normalize_notification(&name)
                    && should_publish(
                        kind,
                        &mut last_disk_usage_event,
                        &mut last_regional_event,
                        Instant::now(),
                    )
                {
                    tracing::debug!(?kind, "received normalized device notification");
                    events.publish(kind);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_names_are_reduced_to_safe_events() {
        assert_eq!(
            normalize_notification("com.apple.mobile.application_installed"),
            Some(DeviceEventKind::AppInstalled)
        );
        assert_eq!(
            normalize_notification("com.apple.mobile.application_uninstalled"),
            Some(DeviceEventKind::AppUninstalled)
        );
        assert_eq!(
            normalize_notification("com.apple.mobile.lockdown.activation_state"),
            Some(DeviceEventKind::ActivationStateChanged)
        );
        assert_eq!(
            normalize_notification("com.apple.mobile.lockdown.disk_usage_changed"),
            Some(DeviceEventKind::DiskUsageChanged)
        );
        assert_eq!(
            normalize_notification("com.apple.mobile.lockdown.device_name_changed"),
            Some(DeviceEventKind::DeviceNameChanged)
        );
        assert_eq!(
            normalize_notification("com.apple.mobile.lockdown.timezone_changed"),
            Some(DeviceEventKind::RegionalSettingsChanged)
        );
        assert_eq!(
            normalize_notification("com.apple.language.changed"),
            Some(DeviceEventKind::RegionalSettingsChanged)
        );
        assert_eq!(
            normalize_notification("com.apple.mobile.developer_image_mounted"),
            Some(DeviceEventKind::DeveloperImageMounted)
        );
        assert_eq!(
            normalize_notification("com.apple.springboard.lockstate"),
            Some(DeviceEventKind::LockStateChanged)
        );
        assert_eq!(normalize_notification("private.payload.event"), None);
    }

    #[tokio::test]
    async fn slot_broadcasts_repeated_events_with_distinct_sequences() {
        let slot = DeviceEventSlot::default();
        let mut receiver = slot.subscribe();
        slot.publish(DeviceEventKind::AppInstalled);
        slot.publish(DeviceEventKind::AppInstalled);

        let first = receiver.recv().await.unwrap();
        let second = receiver.recv().await.unwrap();
        assert_eq!(first.kind, DeviceEventKind::AppInstalled);
        assert_eq!(second.kind, DeviceEventKind::AppInstalled);
        assert_eq!(second.sequence, first.sequence + 1);
        assert_eq!(slot.latest(), Some(second));
    }

    #[test]
    fn reset_clears_retained_event_without_reusing_sequence() {
        let slot = DeviceEventSlot::default();
        slot.publish(DeviceEventKind::AppInstalled);
        slot.reset();
        assert_eq!(slot.latest(), None);
        slot.publish(DeviceEventKind::AppUninstalled);
        assert_eq!(slot.latest().unwrap().sequence, 2);
    }

    #[test]
    fn bursty_metadata_events_are_coalesced_independently() {
        let start = Instant::now();
        let mut last_disk_usage_event = None;
        let mut last_regional_event = None;
        assert!(should_publish(
            DeviceEventKind::DiskUsageChanged,
            &mut last_disk_usage_event,
            &mut last_regional_event,
            start,
        ));
        assert!(!should_publish(
            DeviceEventKind::DiskUsageChanged,
            &mut last_disk_usage_event,
            &mut last_regional_event,
            start + Duration::from_millis(500),
        ));
        assert!(should_publish(
            DeviceEventKind::RegionalSettingsChanged,
            &mut last_disk_usage_event,
            &mut last_regional_event,
            start + Duration::from_millis(500),
        ));
        assert!(!should_publish(
            DeviceEventKind::RegionalSettingsChanged,
            &mut last_disk_usage_event,
            &mut last_regional_event,
            start + Duration::from_millis(750),
        ));
        assert!(should_publish(
            DeviceEventKind::AppInstalled,
            &mut last_disk_usage_event,
            &mut last_regional_event,
            start + Duration::from_millis(500),
        ));
        assert!(should_publish(
            DeviceEventKind::DiskUsageChanged,
            &mut last_disk_usage_event,
            &mut last_regional_event,
            start + COALESCED_EVENT_INTERVAL,
        ));
    }
}
