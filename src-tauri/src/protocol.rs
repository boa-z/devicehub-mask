// Shared types passed between the web server and the async device session.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use tokio::sync::broadcast;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use tokio::sync::watch;

use crate::hid::TouchContact;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameFormat {
    #[default]
    Rgb24,
    Yuv420p,
}

/// A decoded screen frame. Pixels are tightly packed RGB24 or planar YUV420P.
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub format: FrameFormat,
    pub pixels: Vec<u8>,
    pub decoded_at: Instant,
    pub jpeg: OnceLock<Result<Bytes, String>>,
}

#[derive(Debug, Default)]
struct VideoCountersInner {
    source_frames: AtomicU64,
    decoded_frames: AtomicU64,
    duplicate_frames: AtomicU64,
}

#[derive(Debug, Default, Clone)]
pub struct VideoCounters(Arc<VideoCountersInner>);

#[derive(Debug, Clone, Copy)]
pub struct VideoCounterSnapshot {
    pub source_frames: u64,
    pub decoded_frames: u64,
    pub duplicate_frames: u64,
}

impl VideoCounters {
    pub fn note_source_frame(&self) {
        self.0.source_frames.fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_decoded_frame(&self) {
        self.0.decoded_frames.fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_duplicate_frame(&self) {
        self.0.duplicate_frames.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> VideoCounterSnapshot {
        VideoCounterSnapshot {
            source_frames: self.0.source_frames.load(Ordering::Relaxed),
            decoded_frames: self.0.decoded_frames.load(Ordering::Relaxed),
            duplicate_frames: self.0.duplicate_frames.load(Ordering::Relaxed),
        }
    }
}

/// The latest decoded frame, shared with connected WebSocket clients.
///
/// Each consumer subscribes independently and none steal from others; laggards
/// automatically drop to the latest published frame.
#[derive(Clone)]
pub struct FrameSlot(Arc<FrameSlotInner>);

struct FrameSlotInner {
    frame: watch::Sender<Option<Arc<Frame>>>,
    version: AtomicU64,
}

impl Default for FrameSlot {
    fn default() -> Self {
        let (frame, _) = watch::channel(None);
        Self(Arc::new(FrameSlotInner {
            frame,
            version: AtomicU64::new(0),
        }))
    }
}

impl FrameSlot {
    pub fn publish(&self, frame: Arc<Frame>) -> Option<Arc<Frame>> {
        self.0.version.fetch_add(1, Ordering::Relaxed);
        self.0.frame.send_replace(Some(frame))
    }

    /// Subscribe to newest-frame notifications. Slow consumers automatically
    /// skip stale frames because `watch` retains only the latest value.
    pub fn subscribe(&self) -> watch::Receiver<Option<Arc<Frame>>> {
        self.0.frame.subscribe()
    }

    pub fn latest(&self) -> Option<(u64, Arc<Frame>)> {
        self.0
            .frame
            .borrow()
            .clone()
            .map(|frame| (self.version(), frame))
    }

    pub fn version(&self) -> u64 {
        self.0.version.load(Ordering::Relaxed)
    }
}

pub const AUDIO_SAMPLE_RATE: u32 = 48_000;
pub const AUDIO_CHANNELS: u8 = 2;

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

/// Current DVT location simulation state for the active device session.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct LocationStatus {
    pub available: bool,
    pub active: bool,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub error: Option<String>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardContentKind {
    Text,
    Image,
}

/// A transient clipboard sync event surfaced only to authenticated WebSocket clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClipboardEvent {
    /// `true` if the text came *from* the device, `false` if pushed host → device.
    pub from_device: bool,
    pub kind: ClipboardContentKind,
    pub preview: String,
}

/// Bounded fan-out for clipboard activity. A slow UI loses stale notices rather
/// than delaying the device pasteboard session.
#[derive(Clone)]
pub struct ClipboardSlot(broadcast::Sender<ClipboardEvent>);

impl Default for ClipboardSlot {
    fn default() -> Self {
        let (sender, _) = broadcast::channel(8);
        Self(sender)
    }
}

impl ClipboardSlot {
    pub fn set(&self, event: ClipboardEvent) {
        let _ = self.0.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ClipboardEvent> {
        self.0.subscribe()
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

pub fn validate_paste_text(text: &str) -> Result<usize, &'static str> {
    let characters = text.chars().count();
    if text.is_empty() || text.len() > 4_096 || characters > 1_024 || text.contains('\0') {
        Err(
            "paste text must contain 1..1024 characters, fit in 4096 UTF-8 bytes, and contain no NUL bytes",
        )
    } else {
        Ok(characters)
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
    /// Put arbitrary Unicode text on the device pasteboard, then issue Cmd+V.
    PasteText {
        text: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
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
    /// Set a fixed simulated GPS location through the active DVT session.
    SetLocation {
        latitude: f64,
        longitude: f64,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Stop location simulation through the active DVT session.
    ClearLocation {
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Return metadata collected from Lockdown for the active device.
    GetDeviceDetails(oneshot::Sender<Result<DeviceDetails, String>>),
    /// Rename the active device through a verified Lockdown write.
    RenameDevice {
        name: String,
        reply: oneshot::Sender<Result<String, String>>,
    },
    /// Prepare the device-side Developer Mode workflow through AMFI.
    DeveloperMode(crate::developer_mode::DeveloperModeCommand),
    /// List user-facing applications through CoreDevice AppService.
    ListApps(oneshot::Sender<Result<Vec<DeviceApp>, String>>),
    /// Read one validated PNG application icon through SpringBoardServices.
    GetAppIcon {
        bundle_id: String,
        reply: oneshot::Sender<Result<Vec<u8>, String>>,
    },
    /// Capture a lossless PNG directly through CoreDevice ScreenCaptureService.
    TakeScreenshot(oneshot::Sender<Result<Vec<u8>, String>>),
    /// Start or stop a bounded pcapd capture owned by the active device session.
    NetworkCapture(crate::network_capture::NetworkCaptureCommand),
    /// Apply or clear a DVT device-wide network/thermal condition.
    DeviceCondition(crate::device_conditions::DeviceConditionCommand),
    /// Access one application's vended Documents root through House Arrest.
    AppDocuments(crate::app_documents::AppDocumentCommand),
    /// Restart the active device through DiagnosticsRelay.
    RestartDevice(oneshot::Sender<Result<(), String>>),
    /// Shut down the active device through DiagnosticsRelay.
    ShutdownDevice(oneshot::Sender<Result<(), String>>),
    /// List, install, or remove provisioning profiles through Misagent.
    Provisioning(crate::provisioning::ProvisioningCommand),
    /// Launch an application through CoreDevice AppService.
    LaunchApp {
        bundle_id: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Stop an application's main process through CoreDevice AppService.
    StopApp {
        bundle_id: String,
        reply: oneshot::Sender<Result<bool, String>>,
    },
    /// List bounded crash report metadata from the active device session.
    ListCrashReports(oneshot::Sender<Result<DeviceCrashReportList, String>>),
    /// Read a validated, size-bounded crash report for agent diagnostics.
    ReadCrashReport {
        device_path: String,
        max_bytes: usize,
        reply: oneshot::Sender<Result<DeviceCrashReportContent, String>>,
    },
    /// Export one validated crash report to a user-selected host path.
    ExportCrashReport {
        device_path: String,
        destination: PathBuf,
        reply: oneshot::Sender<Result<u64, String>>,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

    pub fn selector_suffix(self) -> &'static str {
        match self {
            ConnKind::Usb => "usb",
            ConnKind::Network => "wifi",
            ConnKind::Other => "other",
        }
    }
}

pub fn device_selector(udid: &str, connection: ConnKind) -> String {
    format!("{udid}::{}", connection.selector_suffix())
}

/// One device usbmuxd currently knows about, for the picker dropdown.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    /// Stable picker value. Unlike the UDID, this distinguishes USB and Wi-Fi.
    pub id: String,
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
    pub storage: Option<DeviceStorage>,
    pub activation_state: Option<DeviceActivationState>,
    pub developer_mode_enabled: Option<bool>,
    pub battery: Option<DeviceBattery>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceActivationState {
    Activated,
    Unactivated,
    FactoryActivated,
    SoftActivated,
    Unknown,
}

pub fn validate_device_name(name: &str) -> Result<String, &'static str> {
    let normalized = name.trim();
    let characters = normalized.chars().count();
    if characters == 0 {
        return Err("device name cannot be empty");
    }
    if characters > 64 || normalized.len() > 255 {
        return Err("device name is too long");
    }
    if normalized.chars().any(char::is_control) {
        return Err("device name cannot contain control characters");
    }
    Ok(normalized.to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceStorage {
    pub data_capacity_bytes: Option<u64>,
    pub data_available_bytes: Option<u64>,
    pub system_capacity_bytes: Option<u64>,
    pub system_available_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceBattery {
    pub level_percent: Option<u8>,
    pub is_charging: Option<bool>,
    pub external_connected: Option<bool>,
    pub fully_charged: Option<bool>,
    pub cycle_count: Option<u64>,
    pub voltage_mv: Option<u64>,
    pub instant_amperage_ma: Option<i64>,
    pub design_capacity_mah: Option<u64>,
    pub full_charge_capacity_mah: Option<u64>,
    pub health_percent: Option<f64>,
    pub time_remaining_minutes: Option<u64>,
    pub adapter_watts: Option<u64>,
    pub adapter_name: Option<String>,
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
    pub documents_available: bool,
    /// `None` means the process list was unavailable for this request.
    pub is_running: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceCrashReport {
    pub path: String,
    pub name: String,
    pub size_bytes: u64,
    pub modified: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceCrashReportList {
    pub reports: Vec<DeviceCrashReport>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceCrashReportContent {
    pub device_path: String,
    pub size_bytes: u64,
    pub bytes_read: usize,
    pub truncated: bool,
    pub lossy_utf8: bool,
    pub content: String,
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
    pub removal_supported: bool,
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

/// A control command from the UI to the session *manager*: which device to talk to.
#[derive(Debug, Clone)]
pub enum ControlCmd {
    /// Re-enumerate attached devices and refresh the picker list.
    Refresh,
    /// Tear down the current session (if any) and connect to this UDID.
    Connect(String),
    /// Tear down the current session even when it already targets this UDID, then reconnect.
    Reconnect(String),
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

    #[tokio::test]
    async fn clipboard_activity_is_broadcast_without_retaining_content() {
        let slot = ClipboardSlot::default();
        let mut receiver = slot.subscribe();
        let event = ClipboardEvent {
            from_device: true,
            kind: ClipboardContentKind::Text,
            preview: "copied text".into(),
        };
        slot.set(event.clone());

        assert_eq!(receiver.recv().await.unwrap(), event);
        assert_eq!(clipboard_preview(" a\n b  c ", 4), "a b ...");
    }

    #[test]
    fn paste_text_validation_accepts_unicode_and_bounds_payloads() {
        assert_eq!(validate_paste_text("你好, iPhone").unwrap(), 10);
        for invalid in [String::new(), "bad\0text".into(), "x".repeat(1_025)] {
            assert!(validate_paste_text(&invalid).is_err());
        }
        assert!(validate_paste_text(&"界".repeat(1_024)).is_ok());
        assert!(validate_paste_text(&"😀".repeat(1_024)).is_ok());
    }

    #[test]
    fn device_name_validation_preserves_unicode_and_rejects_unsafe_values() {
        assert_eq!(
            validate_device_name("  Boa 的 iPhone  ").unwrap(),
            "Boa 的 iPhone"
        );
        assert!(validate_device_name("").is_err());
        assert!(validate_device_name("name\nwith control").is_err());
        assert!(validate_device_name(&"界".repeat(64)).is_ok());
        assert!(validate_device_name(&"界".repeat(65)).is_err());
        assert!(validate_device_name(&"😀".repeat(64)).is_err());
    }

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
}
