//! Thread-safe state ports shared by runtimes and host adapters.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::{
    AppOperationKind, AppOperationState, AppOperationView, DeviceInfo, LocationStatus, Orientation,
};

#[derive(Debug, Default)]
struct VideoCountersInner {
    transport_events: AtomicU64,
    source_frames: AtomicU64,
    decoded_frames: AtomicU64,
}

#[derive(Debug, Default, Clone)]
pub struct VideoCounters(Arc<VideoCountersInner>);

#[derive(Debug, Clone, Copy)]
pub struct VideoCounterSnapshot {
    pub transport_events: u64,
    pub source_frames: u64,
    pub decoded_frames: u64,
}

impl VideoCounters {
    pub fn note_transport_activity(&self) {
        self.0.transport_events.fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_source_frame(&self) {
        self.0.source_frames.fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_decoded_frame(&self) {
        self.0.decoded_frames.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> VideoCounterSnapshot {
        VideoCounterSnapshot {
            transport_events: self.0.transport_events.load(Ordering::Relaxed),
            source_frames: self.0.source_frames.load(Ordering::Relaxed),
            decoded_frames: self.0.decoded_frames.load(Ordering::Relaxed),
        }
    }
}

/// Human-readable connection/stream status surfaced in the UI status bar.
#[derive(Clone, Default)]
pub struct StatusSlot(Arc<Mutex<String>>);

impl StatusSlot {
    pub fn set(&self, s: impl Into<String>) {
        *self.0.lock().unwrap() = s.into();
    }

    pub fn get(&self) -> String {
        self.0.lock().unwrap().clone()
    }
}

#[derive(Clone, Default)]
pub struct LocationStatusSlot(Arc<Mutex<LocationStatus>>);

impl LocationStatusSlot {
    pub fn set(&self, status: LocationStatus) {
        *self.0.lock().unwrap() = status;
    }

    pub fn get(&self) -> LocationStatus {
        self.0.lock().unwrap().clone()
    }
}

/// The device's current screen orientation, shared from the session to the UI.
#[derive(Clone, Default)]
pub struct OrientationSlot(Arc<Mutex<Orientation>>);

impl OrientationSlot {
    pub fn set(&self, o: Orientation) {
        *self.0.lock().unwrap() = o;
    }

    pub fn get(&self) -> Orientation {
        *self.0.lock().unwrap()
    }
}

#[derive(Clone, Default)]
pub struct AppOperationSlot(Arc<Mutex<AppOperationInner>>);

#[derive(Default)]
struct AppOperationInner {
    next_id: u64,
    view: AppOperationView,
}

impl AppOperationSlot {
    pub fn start(&self, kind: AppOperationKind, label: String) -> Result<u64, String> {
        let mut inner = self.0.lock().unwrap();
        if inner.view.state == AppOperationState::Running {
            return Err("another app operation is already running".into());
        }
        inner.next_id = inner.next_id.wrapping_add(1).max(1);
        let id = inner.next_id;
        inner.view = AppOperationView {
            id,
            kind: Some(kind),
            state: AppOperationState::Running,
            stage: Some("validating".into()),
            progress: None,
            label: Some(label),
            error: None,
        };
        Ok(id)
    }

    pub fn update(&self, id: u64, stage: &str, progress: Option<u8>) {
        let mut inner = self.0.lock().unwrap();
        if inner.view.id == id && inner.view.state == AppOperationState::Running {
            inner.view.stage = Some(stage.into());
            inner.view.progress = progress.map(|value| value.min(100));
        }
    }

    pub fn succeed(&self, id: u64) {
        self.finish(id, AppOperationState::Succeeded, None);
    }

    pub fn fail(&self, id: u64, error: String) {
        self.finish(id, AppOperationState::Failed, Some(error));
    }

    pub fn cancel(&self, id: u64) {
        self.finish(
            id,
            AppOperationState::Cancelled,
            Some("device session ended".into()),
        );
    }

    fn finish(&self, id: u64, state: AppOperationState, error: Option<String>) {
        let mut inner = self.0.lock().unwrap();
        if inner.view.id == id && inner.view.state == AppOperationState::Running {
            inner.view.state = state;
            inner.view.stage = None;
            inner.view.progress = (state == AppOperationState::Succeeded).then_some(100);
            inner.view.error = error;
        }
    }

    pub fn get(&self) -> AppOperationView {
        self.0.lock().unwrap().view.clone()
    }
}

/// The set of currently-attached devices, published by the manager for the picker.
#[derive(Clone, Default)]
pub struct DeviceListSlot(Arc<Mutex<Vec<DeviceInfo>>>);

impl DeviceListSlot {
    pub fn set(&self, devices: Vec<DeviceInfo>) {
        *self.0.lock().unwrap() = devices;
    }

    pub fn get(&self) -> Vec<DeviceInfo> {
        self.0.lock().unwrap().clone()
    }
}

#[derive(Clone)]
struct ActiveDevice {
    udid: String,
    selection_id: String,
}

/// Identity of the device the session is currently connected to. `None` while idle.
#[derive(Clone, Default)]
pub struct ActiveSlot(Arc<Mutex<Option<ActiveDevice>>>);

impl ActiveSlot {
    pub fn set(&self, udid: Option<String>) {
        *self.0.lock().unwrap() = udid.map(|udid| ActiveDevice {
            selection_id: udid.clone(),
            udid,
        });
    }

    pub fn set_selected(&self, udid: String, selection_id: String) {
        *self.0.lock().unwrap() = Some(ActiveDevice { udid, selection_id });
    }

    pub fn get(&self) -> Option<String> {
        self.0
            .lock()
            .unwrap()
            .as_ref()
            .map(|active| active.udid.clone())
    }

    pub fn selection_id(&self) -> Option<String> {
        self.0
            .lock()
            .unwrap()
            .as_ref()
            .map(|active| active.selection_id.clone())
    }
}

/// The reason the last session failed, shown by the UI. `None` means no outstanding error.
#[derive(Clone, Default)]
pub struct ErrorSlot(Arc<Mutex<Option<String>>>);

impl ErrorSlot {
    pub fn set(&self, message: Option<String>) {
        *self.0.lock().unwrap() = message;
    }

    pub fn get(&self) -> Option<String> {
        self.0.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloned_state_ports_share_runtime_observations() {
        let status = StatusSlot::default();
        let status_reader = status.clone();
        status.set("streaming");
        assert_eq!(status_reader.get(), "streaming");

        let counters = VideoCounters::default();
        let counter_reader = counters.clone();
        counters.note_transport_activity();
        counters.note_source_frame();
        counters.note_decoded_frame();
        let snapshot = counter_reader.snapshot();
        assert_eq!(snapshot.transport_events, 1);
        assert_eq!(snapshot.source_frames, 1);
        assert_eq!(snapshot.decoded_frames, 1);
    }

    #[test]
    fn app_operation_tracks_progress_and_success() {
        let slot = AppOperationSlot::default();
        let id = slot
            .start(AppOperationKind::Uninstall, "com.example.app".into())
            .unwrap();

        slot.update(id, "uninstalling", Some(101));
        let running = slot.get();
        assert_eq!(running.state, AppOperationState::Running);
        assert_eq!(running.stage.as_deref(), Some("uninstalling"));
        assert_eq!(running.progress, Some(100));

        slot.succeed(id);
        let completed = slot.get();
        assert_eq!(completed.state, AppOperationState::Succeeded);
        assert_eq!(completed.progress, Some(100));
        assert!(completed.stage.is_none());
    }

    #[test]
    fn app_operation_rejects_concurrency_and_ignores_stale_updates() {
        let slot = AppOperationSlot::default();
        let first = slot
            .start(AppOperationKind::Uninstall, "com.example.first".into())
            .unwrap();
        assert!(
            slot.start(AppOperationKind::Uninstall, "com.example.app".into())
                .is_err()
        );
        slot.fail(first, "failed".into());

        let second = slot
            .start(AppOperationKind::Uninstall, "com.example.app".into())
            .unwrap();
        slot.update(first, "uninstalling", Some(50));
        slot.succeed(first);
        let view = slot.get();
        assert_eq!(view.id, second);
        assert_eq!(view.stage.as_deref(), Some("validating"));
        assert_eq!(view.state, AppOperationState::Running);
    }

    #[test]
    fn app_operation_can_be_cancelled() {
        let slot = AppOperationSlot::default();
        let id = slot
            .start(AppOperationKind::Uninstall, "com.example.app".into())
            .unwrap();
        slot.cancel(id);

        let view = slot.get();
        assert_eq!(view.state, AppOperationState::Cancelled);
        assert_eq!(view.error.as_deref(), Some("device session ended"));
    }
}
