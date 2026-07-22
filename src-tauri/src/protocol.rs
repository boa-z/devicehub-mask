// Shared types passed between the web server and the async device session.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use bytes::Bytes;
use serde::Serialize;

use tokio::sync::broadcast;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

use crate::hid::TouchContact;

/// A decoded screen frame. `rgb` is `width * height * 3` bytes, top-down RGB24.
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
    pub jpeg: OnceLock<Result<Bytes, String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum VideoPipeline {
    Jpeg = 0,
    H264Qsv = 1,
    H264Nvenc = 2,
    H264Amf = 3,
    H264VideoToolbox = 4,
}

impl VideoPipeline {
    pub fn label(self) -> &'static str {
        match self {
            Self::Jpeg => "jpeg",
            Self::H264Qsv => "h264-qsv",
            Self::H264Nvenc => "h264-nvenc",
            Self::H264Amf => "h264-amf",
            Self::H264VideoToolbox => "h264-videotoolbox",
        }
    }

    pub fn is_h264(self) -> bool {
        self != Self::Jpeg
    }
}

/// Live MPEG-TS bytes and counters shared by the capture session and web clients.
/// A zero-length message asks connected video clients to reset after a pipeline
/// transition; MPEG-TS itself never emits an empty chunk.
#[derive(Clone)]
pub struct EncodedVideoStream {
    sender: broadcast::Sender<Bytes>,
    pipeline: Arc<AtomicU8>,
    source_frames: Arc<AtomicU64>,
    encoded_bytes: Arc<AtomicU64>,
}

impl Default for EncodedVideoStream {
    fn default() -> Self {
        let (sender, _) = broadcast::channel(256);
        Self {
            sender,
            pipeline: Arc::new(AtomicU8::new(VideoPipeline::Jpeg as u8)),
            source_frames: Arc::new(AtomicU64::new(0)),
            encoded_bytes: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl EncodedVideoStream {
    pub fn subscribe(&self) -> broadcast::Receiver<Bytes> {
        self.sender.subscribe()
    }

    pub fn publish(&self, bytes: Bytes) {
        self.encoded_bytes
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        let _ = self.sender.send(bytes);
    }

    pub fn set_pipeline(&self, pipeline: VideoPipeline) {
        let previous = self.pipeline.swap(pipeline as u8, Ordering::AcqRel);
        if previous != pipeline as u8 {
            tracing::info!(pipeline = pipeline.label(), "selected video pipeline");
            if pipeline == VideoPipeline::Jpeg {
                let _ = self.sender.send(Bytes::new());
            }
        }
    }

    pub fn pipeline(&self) -> VideoPipeline {
        match self.pipeline.load(Ordering::Acquire) {
            1 => VideoPipeline::H264Qsv,
            2 => VideoPipeline::H264Nvenc,
            3 => VideoPipeline::H264Amf,
            4 => VideoPipeline::H264VideoToolbox,
            _ => VideoPipeline::Jpeg,
        }
    }

    pub fn note_source_frame(&self) {
        self.source_frames.fetch_add(1, Ordering::Relaxed);
    }

    pub fn counters(&self) -> (u64, u64) {
        (
            self.source_frames.load(Ordering::Relaxed),
            self.encoded_bytes.load(Ordering::Relaxed),
        )
    }
}

/// The latest decoded frame, shared with connected WebSocket clients.
///
/// Consumers read without consuming and diff the version counter, so each sees
/// new frames independently and none steal from others; laggards drop to latest.
#[derive(Clone, Default)]
pub struct FrameSlot(Arc<Mutex<FrameSlotInner>>);

#[derive(Default)]
struct FrameSlotInner {
    frame: Option<Arc<Frame>>,
    version: u64,
}

impl FrameSlot {
    pub fn publish(&self, frame: Arc<Frame>) -> Option<Arc<Frame>> {
        let mut inner = self.0.lock().unwrap();
        let prev = inner.frame.replace(frame);
        inner.version = inner.version.wrapping_add(1);
        prev
    }

    /// The latest frame and its version, without consuming it.
    pub fn latest(&self) -> Option<(u64, Arc<Frame>)> {
        let inner = self.0.lock().unwrap();
        inner.frame.clone().map(|f| (inner.version, f))
    }

    pub fn version(&self) -> u64 {
        self.0.lock().unwrap().version
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

/// A clipboard sync event, surfaced to the UI's "copied" indicator.
#[derive(Clone)]
#[allow(dead_code)]
pub struct ClipboardEvent {
    /// `true` if the text came *from* the device, `false` if pushed host → device.
    pub from_device: bool,
    pub preview: String,
}

/// The most recent [`ClipboardEvent`]; the UI `take`s it for a transient indicator.
#[derive(Clone, Default)]
pub struct ClipboardSlot(Arc<Mutex<Option<ClipboardEvent>>>);

impl ClipboardSlot {
    pub fn set(&self, event: ClipboardEvent) {
        *self.0.lock().unwrap() = Some(event);
    }
}

/// Single-line clipboard preview: collapse whitespace and truncate to `max` chars.
pub fn clipboard_preview(text: &str, max: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > max {
        let mut s: String = collapsed.chars().take(max).collect();
        s.push_str("...");
        s
    } else {
        collapsed
    }
}

/// A control command from the UI to the device session.
///
/// Touch coordinates are normalized `0..=65535` across the screen
/// (resolution-independent), so the UI needn't know the device's pixel size.
#[derive(Debug)]
#[allow(dead_code)]
pub enum InputCmd {
    /// A tap at a normalized point.
    Tap {
        x: u16,
        y: u16,
    },
    /// Live touch phases for a continuous gesture: preserves velocity, allows
    /// press-and-hold, and lets iOS apply its own momentum on release.
    TouchDown {
        x: u16,
        y: u16,
    },
    TouchMove {
        x: u16,
        y: u16,
    },
    TouchUp {
        x: u16,
        y: u16,
    },
    /// Complete multi-touch frame. Active contacts have `touching = true`; a
    /// released identity is included once with `touching = false`.
    MultiTouchFrame(Vec<TouchContact>),
    /// Type printable text.
    Text(String),
    /// Press a single HID Keyboard/Keypad usage (enter, arrows...).
    KeyUsage(u64),
    /// Press a key with modifiers held (e.g. ⌘C); iOS reads these as
    /// hardware-keyboard shortcuts (⌘H home, ⌘Space search...).
    KeyCombo {
        usage: u64,
        mods: KeyMods,
    },
    /// Raw physical keyboard state forwarded from keyboard-control mode.
    KeyboardDown(u64),
    KeyboardUp(u64),
    /// Press a named hardware button for its default hold duration (see `NAMED_BUTTONS`).
    Button(&'static str),
    /// Press and hold a named hardware button until the matching `ButtonUp`.
    ButtonDown(&'static str),
    ButtonUp(&'static str),
    /// Rotate the device 90° via the CoreDevice orientation service.
    Rotate(RotateDir),
    /// Return metadata collected from Lockdown for the active device.
    GetDeviceDetails(oneshot::Sender<Result<DeviceDetails, String>>),
    /// List user-facing applications through CoreDevice AppService.
    ListApps(oneshot::Sender<Result<Vec<DeviceApp>, String>>),
    /// List installed provisioning profiles through the Mobile Installation Agent.
    ListProvisioningProfiles(oneshot::Sender<Result<Vec<ProvisioningProfile>, String>>),
    /// Launch an application through CoreDevice AppService.
    LaunchApp {
        bundle_id: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Validate and install a local IPA without blocking the HID dispatch loop.
    InstallApp {
        path: PathBuf,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Uninstall a removable user application without blocking HID dispatch.
    UninstallApp {
        bundle_id: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Stop the media stream and tear the session down.
    Shutdown,
}

/// Which way to rotate the device by 90°.
#[derive(Debug, Clone, Copy)]
pub enum RotateDir {
    Left,
    Right,
}

/// The device's screen orientation, reported by the CoreDevice orientation service.
///
/// The video always arrives in native (portrait) orientation — the content is
/// rotated *within* a portrait frame, not the frame itself. The UI uses this to
/// rotate the displayed texture upright and inverse-rotate pointer coords.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Orientation {
    #[default]
    Portrait,
    PortraitUpsideDown,
    LandscapeLeft,
    LandscapeRight,
}

impl Orientation {
    /// Quarter-turns *clockwise* to show the native-portrait frame upright. The
    /// `Landscape{Left,Right}` → turn assignment was verified against a device.
    pub fn quarter_turns_cw(self) -> u8 {
        match self {
            Orientation::Portrait => 0,
            Orientation::LandscapeRight => 1,
            Orientation::PortraitUpsideDown => 2,
            Orientation::LandscapeLeft => 3,
        }
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

/// Keyboard modifiers held during a [`InputCmd::KeyCombo`].
#[derive(Debug, Clone, Copy, Default)]
pub struct KeyMods {
    pub cmd: bool,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// Clamp a `0.0..=1.0` fraction to a normalized `0..=65535` touch coordinate.
pub fn norm(frac: f32) -> u16 {
    (frac.clamp(0.0, 1.0) * 65535.0).round() as u16
}

/// Inverse-map a point in the *displayed* (upright) normalized space back into
/// the device's *native* (unrotated framebuffer) normalized space.
///
/// The native (portrait) space is also the touch space, so upright points must
/// be un-rotated before sending; this doubles as the UV mapping for rendering.
/// The web renderer and input path must use this same transform. The
/// `Landscape{Left,Right}` mapping was verified against a device.
pub fn unrotate_norm(dx: f32, dy: f32, turns: u8) -> (f32, f32) {
    match turns % 4 {
        0 => (dx, dy),
        1 => (dy, 1.0 - dx),
        2 => (1.0 - dx, 1.0 - dy),
        _ => (1.0 - dy, dx),
    }
}

// --- Device selection ---

/// How a device is attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnKind {
    Usb,
    Network,
    Other,
}

impl ConnKind {
    /// A short label for the picker.
    pub fn label(self) -> &'static str {
        match self {
            ConnKind::Usb => "USB",
            ConnKind::Network => "Wi-Fi",
            ConnKind::Other => "?",
        }
    }
}

/// One device usbmuxd currently knows about, for the picker dropdown.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub udid: String,
    /// The device's `DeviceName` (best-effort; falls back to the UDID).
    pub name: String,
    pub connection: ConnKind,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceDetails {
    pub udid: String,
    pub name: String,
    pub product_type: String,
    pub product_version: String,
    pub build_version: Option<String>,
    pub hardware_model: Option<String>,
    pub serial_number: Option<String>,
    /// Decimal text avoids losing 64-bit ECID precision in JavaScript clients.
    pub ecid: Option<String>,
    pub total_disk_capacity: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceApp {
    pub bundle_id: String,
    pub name: String,
    pub version: Option<String>,
    pub bundle_version: Option<String>,
    pub is_removable: bool,
    pub is_first_party: bool,
    pub is_developer_app: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProvisioningProfile {
    pub name: String,
    pub uuid: String,
    pub team_identifiers: Vec<String>,
    pub application_identifier: Option<String>,
    pub creation_date: Option<String>,
    pub expiration_date: Option<String>,
    pub provisioned_devices: usize,
    pub is_expired: bool,
    pub get_task_allow: bool,
    pub parse_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppOperationKind {
    Install,
    Uninstall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppOperationState {
    Idle,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppOperationView {
    pub id: u64,
    pub kind: Option<AppOperationKind>,
    pub state: AppOperationState,
    pub stage: Option<String>,
    pub progress: Option<u8>,
    pub label: Option<String>,
    pub error: Option<String>,
}

impl Default for AppOperationView {
    fn default() -> Self {
        Self {
            id: 0,
            kind: None,
            state: AppOperationState::Idle,
            stage: None,
            progress: None,
            label: None,
            error: None,
        }
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

/// The UDID of the device the session is currently connected to. `None` while idle.
#[derive(Clone, Default)]
pub struct ActiveSlot(Arc<Mutex<Option<String>>>);

impl ActiveSlot {
    pub fn set(&self, udid: Option<String>) {
        *self.0.lock().unwrap() = udid;
    }

    pub fn get(&self) -> Option<String> {
        self.0.lock().unwrap().clone()
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

/// A control command from the UI to the session *manager*: which device to talk to.
#[derive(Debug, Clone)]
pub enum ControlCmd {
    /// Re-enumerate attached devices and refresh the picker list.
    Refresh,
    /// Tear down the current session (if any) and connect to this UDID.
    Connect(String),
    /// Tear down the current session and exit the manager
    Quit,
}

/// The input channel to the *current* session; the manager swaps the inner
/// sender on each reconnect, and commands are dropped while idle.
#[derive(Clone, Default)]
pub struct InputSink(Arc<Mutex<Option<UnboundedSender<InputCmd>>>>);

impl InputSink {
    /// Point the sink at a new session's input channel (or `None` when idle).
    pub fn set(&self, tx: Option<UnboundedSender<InputCmd>>) {
        *self.0.lock().unwrap() = tx;
    }

    /// Send a command to the live session, if any.
    pub fn send(&self, cmd: InputCmd) {
        let _ = self.try_send(cmd);
    }

    pub fn try_send(&self, cmd: InputCmd) -> bool {
        if let Some(tx) = self.0.lock().unwrap().as_ref() {
            tx.send(cmd).is_ok()
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_operation_tracks_progress_and_success() {
        let slot = AppOperationSlot::default();
        let id = slot
            .start(AppOperationKind::Install, "Example.ipa".into())
            .unwrap();

        slot.update(id, "installing", Some(101));
        let running = slot.get();
        assert_eq!(running.state, AppOperationState::Running);
        assert_eq!(running.stage.as_deref(), Some("installing"));
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
            .start(AppOperationKind::Install, "first.ipa".into())
            .unwrap();
        assert!(
            slot.start(AppOperationKind::Uninstall, "com.example.app".into())
                .is_err()
        );
        slot.fail(first, "failed".into());

        let second = slot
            .start(AppOperationKind::Uninstall, "com.example.app".into())
            .unwrap();
        slot.update(first, "installing", Some(50));
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
            .start(AppOperationKind::Install, "Example.ipa".into())
            .unwrap();
        slot.cancel(id);

        let view = slot.get();
        assert_eq!(view.state, AppOperationState::Cancelled);
        assert_eq!(view.error.as_deref(), Some("device session ended"));
    }

    #[tokio::test]
    async fn encoded_video_stream_broadcasts_data_and_resets_on_fallback() {
        let stream = EncodedVideoStream::default();
        let mut receiver = stream.subscribe();
        stream.set_pipeline(VideoPipeline::H264Qsv);
        stream.note_source_frame();
        stream.publish(Bytes::from_static(b"transport-stream"));

        assert_eq!(receiver.recv().await.unwrap(), &b"transport-stream"[..]);
        assert_eq!(stream.counters(), (1, 16));
        assert_eq!(stream.pipeline(), VideoPipeline::H264Qsv);

        stream.set_pipeline(VideoPipeline::Jpeg);
        assert!(receiver.recv().await.unwrap().is_empty());
    }
}
