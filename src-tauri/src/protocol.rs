// Shared types passed between the web server and the async device session.

use std::sync::{Arc, Mutex, OnceLock};

use bytes::Bytes;
use serde::Serialize;

use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

use crate::hid::TouchContact;

/// A decoded screen frame. `rgba` is `width * height * 4` bytes, top-down, non-premultiplied.
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    pub jpeg: OnceLock<Result<Bytes, String>>,
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
    /// Launch an application through CoreDevice AppService.
    LaunchApp {
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
