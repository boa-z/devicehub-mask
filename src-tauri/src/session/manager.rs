//! Device selection and outer session lifecycle state machine.
//!
//! Discovery produces explicit transport endpoints; this manager chooses one,
//! publishes the active input sink, and decides whether a completed session
//! should become idle, switch devices, or rebuild a failed Wi-Fi tunnel. The
//! single-session runner remains transport-agnostic and never selects a fallback
//! endpoint by itself.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedReceiver;

use super::discovery::DeviceDiscovery;
use super::transport::{WIFI_REAUTHORIZE_REQUIRED, resolve_device_selection};
use super::{run, trust};
use crate::audio_output::AudioOutput;
use crate::protocol::{
    ActiveSlot, AppOperationSlot, ClipboardSlot, ConnKind, ControlCmd, DeviceListSlot,
    DevicePairingState, ErrorSlot, ForgetDeviceResult, InputCmd, InputSink, LocationStatus,
    LocationStatusSlot, OrientationSlot, PairDeviceResult, StatusSlot, VideoCounters,
};
use crate::{performance, supervisor};

/// Idle discovery remains responsive without continuously probing mux services.
const IDLE_RESCAN: Duration = Duration::from_secs(2);
/// Once connected, discovery only keeps the picker reasonably fresh. A slower
/// cadence avoids repeatedly opening mux/Wi-Fi discovery paths beside live media.
const ACTIVE_RESCAN: Duration = Duration::from_secs(8);
/// A user transition must not leave two media sessions fighting for the device.
const SWITCH_GRACE: Duration = Duration::from_secs(3);
/// A dropped Wi-Fi child service cannot repair its parent RemotePairing tunnel.
/// Repeated setup failures back off so a sleeping device or stale Bonjour
/// record cannot create a connection storm.
const WIFI_RECONNECT_MAX_DELAY: Duration = Duration::from_secs(8);
const WIFI_STABLE_SESSION: Duration = Duration::from_secs(30);

fn wifi_reconnect_delay(attempt: u32) -> Duration {
    Duration::from_secs(1_u64 << attempt.min(3)).min(WIFI_RECONNECT_MAX_DELAY)
}

/// What the manager should do once the current session is no longer running.
enum Next {
    Switch(String),
    RetryWifi(String),
    Pair {
        selection_id: String,
        reply: tokio::sync::oneshot::Sender<PairDeviceResult>,
    },
    Forget {
        selection_id: String,
        reply: tokio::sync::oneshot::Sender<ForgetDeviceResult>,
    },
    Idle,
    Quit,
}

fn next_after_session_error(connection: ConnKind, selection_id: &str, error_message: &str) -> Next {
    if connection == ConnKind::Network && error_message != WIFI_REAUTHORIZE_REQUIRED {
        Next::RetryWifi(selection_id.to_owned())
    } else {
        Next::Idle
    }
}

fn interrupts_active_session(next: &Next) -> bool {
    matches!(
        next,
        Next::Switch(_) | Next::Pair { .. } | Next::Forget { .. } | Next::Quit
    )
}

#[derive(Clone)]
pub(super) struct SessionViews {
    pub(super) status: StatusSlot,
    pub(super) orientation: OrientationSlot,
    pub(super) error: ErrorSlot,
    pub(super) app_operation: AppOperationSlot,
    pub(super) app_document_activity: crate::app_documents::AppDocumentActivitySlot,
    pub(super) device_file_activity: crate::device_files::DeviceFileActivitySlot,
    pub(super) location: LocationStatusSlot,
    pub(super) performance: performance::PerformanceSlot,
    pub(super) performance_demand: performance::PerformanceDemand,
    pub(super) device_logs: crate::device_logs::DeviceLogSlot,
    pub(super) device_log_demand: crate::device_logs::DeviceLogDemand,
    pub(super) services: supervisor::ServiceRegistry,
    pub(super) device_events: crate::device_events::DeviceEventSlot,
    pub(super) network_capture: crate::network_capture::NetworkCaptureSlot,
    pub(super) bluetooth_capture: crate::bluetooth_capture::BluetoothCaptureSlot,
    pub(super) device_backup: crate::device_backup::DeviceBackupSlot,
    pub(super) sysdiagnose: crate::sysdiagnose::SysdiagnoseSlot,
    pub(super) log_archive: crate::log_archive::LogArchiveSlot,
    pub(super) developer_image: crate::developer_image::DeveloperImageMountSlot,
    pub(super) device_conditions: crate::device_conditions::DeviceConditionSlot,
}

#[derive(Clone)]
pub(super) struct SessionVideo {
    pub(super) counters: VideoCounters,
    pub(super) browser_frames: crate::browser_video::BrowserVideoSlot,
    pub(super) audio_enabled: bool,
    pub(super) clipboard_sync_enabled: bool,
    pub(super) audio: AudioOutput,
}

/// Supervise discovery and ensure exactly one device session owns the media and
/// input surfaces at a time.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn manage(
    initial_udid: Option<String>,
    pairing_dir: PathBuf,
    resource_dir: Option<PathBuf>,
    settings: Arc<crate::settings::AppSettings>,
    video_counters: VideoCounters,
    browser_frames: crate::browser_video::BrowserVideoSlot,
    audio: AudioOutput,
    status: StatusSlot,
    clipboard: ClipboardSlot,
    device_events: crate::device_events::DeviceEventSlot,
    network_capture: crate::network_capture::NetworkCaptureSlot,
    bluetooth_capture: crate::bluetooth_capture::BluetoothCaptureSlot,
    device_backup: crate::device_backup::DeviceBackupSlot,
    sysdiagnose: crate::sysdiagnose::SysdiagnoseSlot,
    log_archive: crate::log_archive::LogArchiveSlot,
    developer_image: crate::developer_image::DeveloperImageMountSlot,
    device_conditions: crate::device_conditions::DeviceConditionSlot,
    orientation_view: OrientationSlot,
    device_list: DeviceListSlot,
    active: ActiveSlot,
    error: ErrorSlot,
    app_operation: AppOperationSlot,
    app_document_activity: crate::app_documents::AppDocumentActivitySlot,
    device_file_activity: crate::device_files::DeviceFileActivitySlot,
    location: LocationStatusSlot,
    performance: performance::PerformanceSlot,
    performance_demand: performance::PerformanceDemand,
    device_logs: crate::device_logs::DeviceLogSlot,
    device_log_demand: crate::device_logs::DeviceLogDemand,
    services: supervisor::ServiceRegistry,
    input_sink: InputSink,
    mut control_rx: UnboundedReceiver<ControlCmd>,
) {
    let mut discovery = DeviceDiscovery::new(pairing_dir.clone(), resource_dir);
    // Auto-pick only before the first connection. Returning to idle after a
    // session ends prevents a persistent hardware failure from hot-looping.
    let mut auto_pick = initial_udid.is_none();
    let mut target = initial_udid;
    let mut wifi_retry_attempt = 0_u32;

    loop {
        let (devices, endpoints) = discovery.refresh().await;
        device_list.set(devices);
        let wifi_setup_required = discovery.requires_pairing();

        if let Some(requested) = target.as_deref()
            && let Some(resolved) = resolve_device_selection(requested, &device_list.get())
        {
            target = Some(resolved);
        }

        if target.is_none()
            && auto_pick
            && let Some(first) = device_list
                .get()
                .into_iter()
                .find(|device| device.pairing != DevicePairingState::Unpaired)
        {
            target = Some(first.id.clone());
            auto_pick = false;
        }

        let Some(selection_id) = target.clone() else {
            active.set(None);
            location.set(LocationStatus::default());
            performance.reset();
            device_logs.reset();
            services.clear();
            status.set(if wifi_setup_required {
                "Wi-Fi device found - connect it by USB once to authorize this app"
            } else {
                "no device - pick one from the menu"
            });
            tokio::select! {
                cmd = control_rx.recv() => match cmd {
                    Some(ControlCmd::Connect(id) | ControlCmd::Reconnect(id)) => target = Some(id),
                    Some(ControlCmd::Refresh) => discovery.invalidate(),
                    Some(ControlCmd::Pair { selection_id, reply }) => {
                        let requested = selection_id.clone();
                        if trust::pair(selection_id, reply, &endpoints, &status).await {
                            target = Some(requested);
                        }
                        discovery.invalidate();
                    }
                    Some(ControlCmd::Forget { selection_id, reply }) => {
                        trust::forget(
                            selection_id,
                            reply,
                            &endpoints,
                            &status,
                            &mut discovery,
                        ).await;
                        discovery.invalidate();
                    }
                    Some(ControlCmd::Quit) | None => return,
                },
                _ = tokio::time::sleep(IDLE_RESCAN) => {}
            }
            continue;
        };

        let Some(endpoint) = endpoints.get(&selection_id).cloned() else {
            tracing::debug!(
                transport = %selection_id,
                "requested device transport not discovered yet"
            );
            active.set(None);
            status.set("waiting for selected device transport...");
            tokio::select! {
                cmd = control_rx.recv() => match cmd {
                    Some(ControlCmd::Connect(id) | ControlCmd::Reconnect(id)) => target = Some(id),
                    Some(ControlCmd::Refresh) => discovery.invalidate(),
                    Some(ControlCmd::Pair { selection_id, reply }) => {
                        let requested = selection_id.clone();
                        if trust::pair(selection_id, reply, &endpoints, &status).await {
                            target = Some(requested);
                        }
                        discovery.invalidate();
                    }
                    Some(ControlCmd::Forget { selection_id, reply }) => {
                        trust::forget(
                            selection_id,
                            reply,
                            &endpoints,
                            &status,
                            &mut discovery,
                        ).await;
                        target = None;
                        discovery.invalidate();
                    }
                    Some(ControlCmd::Quit) | None => return,
                },
                _ = tokio::time::sleep(IDLE_RESCAN) => {}
            }
            continue;
        };
        let udid = endpoint.udid().to_owned();
        let connection = endpoint.connection();

        // Publishing this sender is the ownership hand-off from the manager to
        // HTTP/MCP/Tauri adapters. It is cleared before any next session starts.
        let (in_tx, in_rx) = tokio::sync::mpsc::unbounded_channel();
        input_sink.set(Some(in_tx.clone()));
        active.set_selected(udid.clone(), selection_id.clone());
        error.set(None);

        let session = run(
            endpoint,
            pairing_dir.clone(),
            SessionVideo {
                counters: video_counters.clone(),
                browser_frames: browser_frames.clone(),
                audio_enabled: settings.audio_enabled(),
                clipboard_sync_enabled: settings.clipboard_sync_enabled(),
                audio: audio.clone(),
            },
            clipboard.clone(),
            SessionViews {
                status: status.clone(),
                orientation: orientation_view.clone(),
                error: error.clone(),
                app_operation: app_operation.clone(),
                app_document_activity: app_document_activity.clone(),
                device_file_activity: device_file_activity.clone(),
                location: location.clone(),
                performance: performance.clone(),
                performance_demand: performance_demand.clone(),
                device_logs: device_logs.clone(),
                device_log_demand: device_log_demand.clone(),
                services: services.clone(),
                device_events: device_events.clone(),
                network_capture: network_capture.clone(),
                bluetooth_capture: bluetooth_capture.clone(),
                device_backup: device_backup.clone(),
                sysdiagnose: sysdiagnose.clone(),
                log_archive: log_archive.clone(),
                developer_image: developer_image.clone(),
                device_conditions: device_conditions.clone(),
            },
            in_rx,
        );
        tokio::pin!(session);
        let session_started = std::time::Instant::now();
        let mut active_rescan = tokio::time::interval(ACTIVE_RESCAN);
        active_rescan.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first interval tick is immediate; consume it because discovery was
        // refreshed immediately before this session started.
        active_rescan.tick().await;

        let outcome = loop {
            tokio::select! {
                result = &mut session => match result {
                    Ok(()) => break Next::Idle,
                    Err(error_message) => {
                        tracing::error!(connection = connection.label(), "session ended: {error_message}");
                        let next = next_after_session_error(
                            connection,
                            &selection_id,
                            &error_message,
                        );
                        error.set(Some(error_message));
                        if connection == ConnKind::Network
                            && session_started.elapsed() >= WIFI_STABLE_SESSION
                        {
                            wifi_retry_attempt = 0;
                        }
                        break next;
                    }
                },
                cmd = control_rx.recv() => match cmd {
                    Some(ControlCmd::Connect(id)) if id != selection_id && id != udid => {
                        break Next::Switch(id);
                    }
                    Some(ControlCmd::Connect(_)) => {}
                    Some(ControlCmd::Reconnect(id)) => break Next::Switch(id),
                    Some(ControlCmd::Refresh) => {
                        discovery.invalidate();
                        let (devices, _) = discovery.refresh().await;
                        device_list.set(devices);
                    }
                    Some(ControlCmd::Pair { selection_id, reply }) => {
                        break Next::Pair { selection_id, reply };
                    }
                    Some(ControlCmd::Forget { selection_id, reply }) => {
                        break Next::Forget { selection_id, reply };
                    }
                    Some(ControlCmd::Quit) | None => break Next::Quit,
                },
                _ = active_rescan.tick() => {
                    let (devices, _) = discovery.refresh().await;
                    device_list.set(devices);
                }
            }
        };

        // User-directed transitions interrupt a live session. Await teardown so
        // two media/HID owners can never overlap on the same physical device.
        if interrupts_active_session(&outcome) {
            let _ = in_tx.send(InputCmd::Shutdown);
            let _ = tokio::time::timeout(SWITCH_GRACE, &mut session).await;
        }
        input_sink.set(None);
        active.set(None);
        location.set(LocationStatus::default());

        match outcome {
            Next::Switch(id) => {
                wifi_retry_attempt = 0;
                target = Some(id);
            }
            Next::RetryWifi(id) => {
                let retry_delay = wifi_reconnect_delay(wifi_retry_attempt);
                wifi_retry_attempt = wifi_retry_attempt.saturating_add(1);
                status.set("Wi-Fi control interrupted - retrying connection...");
                tracing::info!(
                    attempt = wifi_retry_attempt,
                    retry_ms = retry_delay.as_millis(),
                    "Wi-Fi session transport dropped; rebuilding the complete tunnel"
                );
                target = Some(id);
                tokio::time::sleep(retry_delay).await;
            }
            Next::Pair {
                selection_id,
                reply,
            } => {
                wifi_retry_attempt = 0;
                let requested = selection_id.clone();
                target = trust::pair(selection_id, reply, &endpoints, &status)
                    .await
                    .then_some(requested);
                discovery.invalidate();
            }
            Next::Forget {
                selection_id,
                reply,
            } => {
                wifi_retry_attempt = 0;
                trust::forget(selection_id, reply, &endpoints, &status, &mut discovery).await;
                target = None;
                discovery.invalidate();
            }
            Next::Idle => {
                wifi_retry_attempt = 0;
                target = None;
            }
            Next::Quit => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Next, interrupts_active_session, next_after_session_error, wifi_reconnect_delay};
    use crate::protocol::{ConnKind, ForgetDeviceResult, PairDeviceResult};

    #[test]
    fn only_network_failures_rebuild_the_parent_tunnel() {
        assert!(matches!(
            next_after_session_error(ConnKind::Network, "wifi:device", "transient failure"),
            Next::RetryWifi(id) if id == "wifi:device"
        ));
        assert!(matches!(
            next_after_session_error(ConnKind::Usb, "usb:device", "transport failure"),
            Next::Idle
        ));
        assert!(matches!(
            next_after_session_error(
                ConnKind::Network,
                "wifi:device",
                super::WIFI_REAUTHORIZE_REQUIRED,
            ),
            Next::Idle
        ));
    }

    #[test]
    fn repeated_wifi_failures_use_bounded_backoff() {
        assert_eq!(wifi_reconnect_delay(0), std::time::Duration::from_secs(1));
        assert_eq!(wifi_reconnect_delay(1), std::time::Duration::from_secs(2));
        assert_eq!(wifi_reconnect_delay(2), std::time::Duration::from_secs(4));
        assert_eq!(wifi_reconnect_delay(3), std::time::Duration::from_secs(8));
        assert_eq!(wifi_reconnect_delay(20), std::time::Duration::from_secs(8));
    }

    #[test]
    fn user_transitions_stop_the_active_session_before_handoff() {
        let (pair_reply, _) = tokio::sync::oneshot::channel::<PairDeviceResult>();
        let (forget_reply, _) = tokio::sync::oneshot::channel::<ForgetDeviceResult>();
        assert!(interrupts_active_session(&Next::Switch("other".into())));
        assert!(interrupts_active_session(&Next::Pair {
            selection_id: "usb:device".into(),
            reply: pair_reply,
        }));
        assert!(interrupts_active_session(&Next::Forget {
            selection_id: "usb:device".into(),
            reply: forget_reply,
        }));
        assert!(interrupts_active_session(&Next::Quit));
        assert!(!interrupts_active_session(&Next::Idle));
        assert!(!interrupts_active_session(&Next::RetryWifi(
            "wifi:device".into()
        )));
    }
}
