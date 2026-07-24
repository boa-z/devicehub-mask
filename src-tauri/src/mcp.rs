//! Streamable HTTP MCP frontend for observing and controlling the active device.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{mpsc::UnboundedSender, oneshot};

use crate::application::DeviceControlService;
use crate::device_events::DeviceEventSlot;
use crate::device_logs::{DeviceLogDemand, DeviceLogEntry, DeviceLogLevel, DeviceLogSlot};
use crate::hid::TouchContact;
use crate::performance::{PerformanceDemand, PerformanceSlot};
use crate::protocol::{
    ActiveSlot, ControlCmd, DeviceListSlot, ErrorSlot, Frame, FrameFormat, InputCmd,
    LocationStatusSlot, Orientation, OrientationSlot, RotateDir, StatusSlot, norm, unrotate_norm,
    validate_paste_text,
};
#[cfg(test)]
use crate::protocol::{FrameSlot, InputSink};

const DEFAULT_ADDR: &str = "127.0.0.1:8009";
const DEFAULT_MAX_DIM: u32 = 1024;
const MAX_SCREENSHOT_DIM: u32 = 4096;
const TAP_SAMPLE_MS: u64 = 25;
const DEFAULT_TAP_HOLD_MS: u64 = 100;
const SETTLE_MIN: Duration = Duration::from_millis(200);
const SETTLE_MAX: Duration = Duration::from_millis(2600);
const SETTLE_POLL: Duration = Duration::from_millis(110);
const SETTLE_DIFF: f32 = 2.5;
const SETTLE_STABLE_SAMPLES: u32 = 3;
const GRID_STEP: u32 = 100;
const GRID_LABEL_EVERY: u32 = 2;
const DEVICE_WAIT: Duration = Duration::from_secs(20);
const DEVICE_DETAILS_WAIT: Duration = Duration::from_secs(8);
const DEVICE_EVENT_WAIT_MAX: Duration = Duration::from_secs(30);
const LOCATION_WAIT: Duration = Duration::from_secs(10);
const CLIPBOARD_WAIT: Duration = Duration::from_secs(10);
const APP_WAIT: Duration = Duration::from_secs(15);
const CRASH_REPORT_WAIT: Duration = Duration::from_secs(20);
const DEFAULT_CRASH_REPORT_BYTES: usize = 256 * 1024;
const OBSERVABILITY_WAIT_MAX: Duration = Duration::from_secs(10);
const SCREENSHOT_WAIT: Duration = Duration::from_secs(18);
const PERFORMANCE_WAIT_DEFAULT: Duration = Duration::from_millis(2500);
const DEVICE_LOG_WAIT_DEFAULT: Duration = Duration::from_millis(1000);
const WDA_WAIT: Duration = Duration::from_secs(15);
const WDA_COMMAND_DEADLINE: Duration = Duration::from_secs(12);
const WDA_RUNNER_START_WAIT: Duration = Duration::from_secs(35);
const DEVICE_CONDITION_COMMAND_DEADLINE: Duration = Duration::from_secs(7);
const DEVICE_CONDITION_WAIT: Duration = Duration::from_secs(8);

#[derive(Clone, Default)]
struct McpObservability {
    device_events: DeviceEventSlot,
    device_conditions: crate::device_conditions::DeviceConditionSlot,
    performance: PerformanceSlot,
    performance_demand: PerformanceDemand,
    device_logs: DeviceLogSlot,
    device_log_demand: DeviceLogDemand,
}

#[derive(Clone)]
struct DeviceHub {
    device_control: DeviceControlService,
    orientation: OrientationSlot,
    devices: DeviceListSlot,
    active: ActiveSlot,
    error: ErrorSlot,
    status: StatusSlot,
    location: LocationStatusSlot,
    observability: McpObservability,
    control: UnboundedSender<ControlCmd>,
    last_image: Arc<Mutex<Option<(u32, u32)>>>,
    gesture_lock: Arc<tokio::sync::Mutex<()>>,
    tool_router: ToolRouter<DeviceHub>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ScreenshotParams {
    /// Draw a coordinate grid over the returned image. Defaults to true.
    grid: Option<bool>,
    /// Maximum length of the image's longer edge. Defaults to 1024; 0 keeps native size.
    max_dim: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct PointParams {
    /// Horizontal coordinate in pixels in the referenced screenshot.
    x: f32,
    /// Vertical coordinate in pixels in the referenced screenshot.
    y: f32,
    /// Width of the screenshot used for the coordinates. Must accompany image_height.
    image_width: Option<u32>,
    /// Height of the screenshot used for the coordinates. Must accompany image_width.
    image_height: Option<u32>,
    /// Contact hold time in milliseconds. Defaults to 100 and is clamped to 25..5000.
    hold_ms: Option<u64>,
    /// Wait for the screen to become visually stable after the action. Defaults to true.
    wait_for_settle: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SwipeParams {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    /// Gesture duration in milliseconds. Defaults to 300 and is clamped to 50..5000.
    duration_ms: Option<u64>,
    image_width: Option<u32>,
    image_height: Option<u32>,
    /// Wait for the screen to become visually stable after the action. Defaults to true.
    wait_for_settle: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TouchPathParams {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct MultiTouchParams {
    /// One to five simultaneous touch paths. Use identical start/end points for held buttons.
    contacts: Vec<TouchPathParams>,
    /// Shared gesture duration in milliseconds. Defaults to 250 and is clamped to 25..5000.
    duration_ms: Option<u64>,
    image_width: Option<u32>,
    image_height: Option<u32>,
    /// Wait for visual stability. Defaults to false for latency-sensitive game actions.
    wait_for_settle: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct WaitFrameParams {
    /// Wait for a frame newer than this version. Defaults to the version current at tool entry.
    after_version: Option<u64>,
    /// Maximum wait in milliseconds. Defaults to 2000 and is clamped to 1..10000.
    timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TextParams {
    text: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct KeyParams {
    /// enter, escape, backspace, tab, delete, arrows, home, end, pageup or pagedown.
    key: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ButtonParams {
    /// home, lock, volume-up, volume-down, mute, siri or action.
    button: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct RotateParams {
    /// left or right.
    direction: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct DeviceParams {
    udid: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct DeviceDetailsParams {
    /// Include UDID, serial number, and ECID. Defaults to false.
    include_identifiers: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct DeviceEventParams {
    /// Return immediately when the latest event sequence is newer than this cursor.
    /// When omitted, wait only for an event occurring after this call starts.
    after_sequence: Option<u64>,
    /// Maximum wait in milliseconds. Defaults to 10000 and is clamped to 0..30000.
    timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct LocationParams {
    latitude: f64,
    longitude: f64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct DeviceConditionParams {
    /// Group identifier returned by list_device_conditions.
    group_identifier: String,
    /// Profile identifier returned within the selected group.
    profile_identifier: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ListAppsParams {
    /// Case-insensitive app-name or bundle-ID filter.
    query: Option<String>,
    /// Include Apple default apps through CoreDevice AppService. Defaults to false.
    include_system: Option<bool>,
    /// Include App Clips through CoreDevice AppService. Defaults to false.
    include_app_clips: Option<bool>,
    /// Maximum returned apps. Defaults to 100 and is clamped to 1..200.
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct AppParams {
    bundle_id: String,
    /// Wait for the launched app screen to become stable. Defaults to true.
    wait_for_settle: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct StopAppParams {
    bundle_id: String,
    /// Wait for the screen after the app stops to become stable. Defaults to true.
    wait_for_settle: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct CrashReportListParams {
    /// Optional case-insensitive report name or path filter.
    query: Option<String>,
    /// Maximum returned reports. Defaults to 50 and is clamped to 1..200.
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct CrashReportReadParams {
    /// Absolute device path returned by list_crash_reports.
    device_path: String,
    /// Maximum bytes to return. Defaults to 262144 and cannot exceed 1048576.
    max_bytes: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct PerformanceSnapshotParams {
    /// Wait for a fresh DVT sample in milliseconds. Defaults to 2500; 0 returns immediately.
    wait_ms: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct DeviceLogParams {
    /// Return entries with sequence numbers greater than this cursor.
    after: Option<u64>,
    /// Maximum matching entries. Defaults to 100 and is clamped to 1..500.
    limit: Option<usize>,
    /// Wait for a matching entry in milliseconds. Defaults to 1000; 0 returns immediately.
    wait_ms: Option<u64>,
    /// Optional level filter: notice, info, debug, error, or fault.
    level: Option<String>,
    /// Optional case-insensitive text filter across message and metadata.
    query: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct WdaUiTreeParams {
    /// Maximum XML characters returned. Defaults to 131072 and cannot exceed 1048576.
    max_characters: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct WdaFindParams {
    /// WDA strategy: accessibility id, name, class name, xpath, -ios predicate string, or -ios class chain.
    using: String,
    /// Selector expression interpreted by WebDriverAgent.
    value: String,
    /// Maximum returned matches. Defaults to 10 and cannot exceed 20.
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct WdaClickParams {
    /// WDA strategy: accessibility id, name, class name, xpath, -ios predicate string, or -ios class chain.
    using: String,
    /// Selector expression interpreted by WebDriverAgent.
    value: String,
    /// Zero-based match index. Defaults to 0 and cannot exceed 19.
    index: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct WdaStartParams {
    /// Installed developer application bundle ID ending in .xctrunner, discovered with list_apps.
    runner_bundle_id: String,
}

fn ok_text(value: impl Into<String>) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![Content::text(value.into())]))
}

fn image_size(width: Option<u32>, height: Option<u32>) -> Result<Option<(u32, u32)>, McpError> {
    match (width, height) {
        (None, None) => Ok(None),
        (Some(width), Some(height)) if width > 0 && height > 0 => Ok(Some((width, height))),
        (Some(0), _) | (_, Some(0)) => Err(McpError::invalid_params(
            "image_width and image_height must be greater than zero",
            None,
        )),
        _ => Err(McpError::invalid_params(
            "image_width and image_height must be provided together",
            None,
        )),
    }
}

fn valid_bundle_identifier(bundle_id: &str) -> bool {
    !bundle_id.is_empty()
        && bundle_id.len() <= 255
        && bundle_id.contains('.')
        && bundle_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
}

fn validate_touch_count(count: usize) -> Result<(), McpError> {
    if (1..=5).contains(&count) {
        Ok(())
    } else {
        Err(McpError::invalid_params(
            "contacts must contain between one and five touch paths",
            None,
        ))
    }
}

fn display_dims(frame: &Frame, turns: u8) -> (u32, u32) {
    let (width, height) = (frame.width as u32, frame.height as u32);
    if turns % 2 == 1 {
        (height, width)
    } else {
        (width, height)
    }
}

fn rgb_at(frame: &Frame, x: usize, y: usize) -> [u8; 3] {
    match frame.format {
        FrameFormat::Rgb24 => {
            let offset = (y * frame.width + x) * 3;
            frame
                .pixels
                .get(offset..offset + 3)
                .map(|pixel| [pixel[0], pixel[1], pixel[2]])
                .unwrap_or([0, 0, 0])
        }
        FrameFormat::Yuv420p => {
            let y_len = frame.width.saturating_mul(frame.height);
            let chroma_width = frame.width.div_ceil(2);
            let chroma_height = frame.height.div_ceil(2);
            let chroma_len = chroma_width.saturating_mul(chroma_height);
            let y_value = *frame.pixels.get(y * frame.width + x).unwrap_or(&16) as i32;
            let chroma_offset = (y / 2) * chroma_width + x / 2;
            let u = *frame.pixels.get(y_len + chroma_offset).unwrap_or(&128) as i32;
            let v = *frame
                .pixels
                .get(y_len + chroma_len + chroma_offset)
                .unwrap_or(&128) as i32;
            let c = (y_value - 16).max(0);
            let d = u - 128;
            let e = v - 128;
            let clamp = |value: i32| ((value + 128) >> 8).clamp(0, 255) as u8;
            [
                clamp(298 * c + 409 * e),
                clamp(298 * c - 100 * d - 208 * e),
                clamp(298 * c + 516 * d),
            ]
        }
    }
}

fn render_upright(frame: &Frame, turns: u8) -> (u32, u32, Vec<u8>) {
    let (width, height) = display_dims(frame, turns);
    if width == 0 || height == 0 || frame.width == 0 || frame.height == 0 {
        return (width, height, Vec::new());
    }
    let mut rgb = vec![0; width as usize * height as usize * 3];
    for output_y in 0..height {
        for output_x in 0..width {
            let dx = (output_x as f32 + 0.5) / width as f32;
            let dy = (output_y as f32 + 0.5) / height as f32;
            let (nx, ny) = unrotate_norm(dx, dy, turns);
            let source_x = ((nx * frame.width as f32) as usize).min(frame.width - 1);
            let source_y = ((ny * frame.height as f32) as usize).min(frame.height - 1);
            let offset = (output_y as usize * width as usize + output_x as usize) * 3;
            rgb[offset..offset + 3].copy_from_slice(&rgb_at(frame, source_x, source_y));
        }
    }
    (width, height, rgb)
}

fn downscale(rgb: Vec<u8>, width: u32, height: u32, max_dim: u32) -> (u32, u32, Vec<u8>) {
    let longer = width.max(height);
    if max_dim == 0 || longer <= max_dim || rgb.len() != width as usize * height as usize * 3 {
        return (width, height, rgb);
    }
    let scale = max_dim as f32 / longer as f32;
    let output_width = ((width as f32 * scale).round() as u32).max(1);
    let output_height = ((height as f32 * scale).round() as u32).max(1);
    let image = image::RgbImage::from_raw(width, height, rgb).expect("RGB dimensions validated");
    let resized = image::imageops::resize(
        &image,
        output_width,
        output_height,
        image::imageops::FilterType::Triangle,
    );
    (output_width, output_height, resized.into_raw())
}

fn frame_signature(frame: &Frame) -> Vec<u8> {
    const SAMPLES: usize = 24;
    if frame.width == 0 || frame.height == 0 {
        return Vec::new();
    }
    let mut signature = Vec::with_capacity(SAMPLES * SAMPLES);
    for row in 0..SAMPLES {
        let y =
            ((row * frame.height) / SAMPLES + frame.height / (2 * SAMPLES)).min(frame.height - 1);
        for column in 0..SAMPLES {
            let x = ((column * frame.width) / SAMPLES + frame.width / (2 * SAMPLES))
                .min(frame.width - 1);
            let [red, green, blue] = rgb_at(frame, x, y);
            signature.push(((red as u16 * 2 + green as u16 * 5 + blue as u16) / 8) as u8);
        }
    }
    signature
}

fn signature_diff(left: &[u8], right: &[u8]) -> f32 {
    if left.is_empty() || left.len() != right.len() {
        return f32::INFINITY;
    }
    let total: u32 = left
        .iter()
        .zip(right)
        .map(|(left, right)| (*left as i32 - *right as i32).unsigned_abs())
        .sum();
    total as f32 / left.len() as f32
}

const DIGITS: [[u8; 5]; 10] = [
    [7, 5, 5, 5, 7],
    [2, 6, 2, 2, 7],
    [7, 1, 7, 4, 7],
    [7, 1, 7, 1, 7],
    [5, 5, 7, 1, 1],
    [7, 4, 7, 1, 7],
    [7, 4, 7, 5, 7],
    [7, 1, 2, 4, 4],
    [7, 5, 7, 5, 7],
    [7, 5, 7, 1, 7],
];

fn blend(rgb: &mut [u8], width: u32, height: u32, x: i32, y: i32, color: [u8; 3], alpha: f32) {
    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
        return;
    }
    let offset = (y as usize * width as usize + x as usize) * 3;
    for channel in 0..3 {
        rgb[offset + channel] = (rgb[offset + channel] as f32 * (1.0 - alpha)
            + color[channel] as f32 * alpha)
            .round() as u8;
    }
}

fn fill(rgb: &mut [u8], width: u32, height: u32, rect: (i32, i32, u32, u32), color: [u8; 3]) {
    let (x, y, w, h) = rect;
    for dy in 0..h as i32 {
        for dx in 0..w as i32 {
            blend(rgb, width, height, x + dx, y + dy, color, 1.0);
        }
    }
}

fn draw_number(rgb: &mut [u8], width: u32, height: u32, x: i32, y: i32, scale: u32, value: u32) {
    let text = value.to_string();
    let text_width = text.len() as u32 * 4 * scale;
    fill(
        rgb,
        width,
        height,
        (
            x - scale as i32,
            y - scale as i32,
            text_width + 2 * scale,
            7 * scale,
        ),
        [0, 0, 0],
    );
    let mut cursor = x;
    for byte in text.bytes() {
        for (row, bits) in DIGITS[(byte - b'0') as usize].iter().enumerate() {
            for column in 0..3 {
                if bits & (1 << (2 - column)) != 0 {
                    fill(
                        rgb,
                        width,
                        height,
                        (
                            cursor + (column * scale) as i32,
                            y + (row as u32 * scale) as i32,
                            scale,
                            scale,
                        ),
                        [255, 255, 0],
                    );
                }
            }
        }
        cursor += (4 * scale) as i32;
    }
}

fn draw_grid(rgb: &mut [u8], width: u32, height: u32) {
    let scale = (width / 240).max(2);
    let label_every = GRID_STEP * GRID_LABEL_EVERY;
    let mut x = GRID_STEP;
    while x < width {
        let major = x.is_multiple_of(label_every);
        for y in 0..height {
            blend(
                rgb,
                width,
                height,
                x as i32,
                y as i32,
                [255, 0, 170],
                if major { 0.7 } else { 0.3 },
            );
        }
        if major {
            draw_number(rgb, width, height, x as i32 + 3, scale as i32, scale, x);
        }
        x += GRID_STEP;
    }
    let mut y = GRID_STEP;
    while y < height {
        let major = y.is_multiple_of(label_every);
        for x in 0..width {
            blend(
                rgb,
                width,
                height,
                x as i32,
                y as i32,
                [255, 0, 170],
                if major { 0.7 } else { 0.3 },
            );
        }
        if major {
            draw_number(rgb, width, height, scale as i32, y as i32 + 3, scale, y);
        }
        y += GRID_STEP;
    }
}

fn key_usage(name: &str) -> Option<u64> {
    Some(match name.to_ascii_lowercase().as_str() {
        "enter" | "return" => 0x28,
        "escape" | "esc" => 0x29,
        "backspace" => 0x2a,
        "tab" => 0x2b,
        "delete" | "del" => 0x4c,
        "right" => 0x4f,
        "left" => 0x50,
        "down" => 0x51,
        "up" => 0x52,
        "home" => 0x4a,
        "end" => 0x4d,
        "pageup" => 0x4b,
        "pagedown" => 0x4e,
        _ => return None,
    })
}

fn button_label(name: &str) -> Option<&'static str> {
    Some(match name.to_ascii_lowercase().replace('_', "-").as_str() {
        "home" => "home",
        "lock" | "power" | "sleep" => "lock",
        "volume-up" | "volup" => "volume-up",
        "volume-down" | "voldown" => "volume-down",
        "mute" | "ring" => "mute",
        "siri" => "siri",
        "action" => "action",
        _ => return None,
    })
}

fn parse_log_level(level: Option<&str>) -> Result<Option<DeviceLogLevel>, McpError> {
    let Some(level) = level else {
        return Ok(None);
    };
    let level = match level.trim().to_ascii_lowercase().as_str() {
        "notice" => DeviceLogLevel::Notice,
        "info" => DeviceLogLevel::Info,
        "debug" => DeviceLogLevel::Debug,
        "error" => DeviceLogLevel::Error,
        "fault" => DeviceLogLevel::Fault,
        _ => {
            return Err(McpError::invalid_params(
                "level must be notice, info, debug, error, or fault",
                None,
            ));
        }
    };
    Ok(Some(level))
}

fn log_entry_matches(
    entry: &DeviceLogEntry,
    level: Option<DeviceLogLevel>,
    query: Option<&str>,
) -> bool {
    if level.is_some_and(|level| entry.level != Some(level)) {
        return false;
    }
    let Some(query) = query else {
        return true;
    };
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return true;
    }
    [
        Some(entry.message.as_str()),
        entry.process.as_deref(),
        entry.subsystem.as_deref(),
        entry.category.as_deref(),
        entry.filename.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|field| field.to_ascii_lowercase().contains(&query))
}

async fn await_device_condition(
    response: oneshot::Receiver<Result<(), String>>,
    operation: &str,
) -> Result<(), McpError> {
    tokio::time::timeout(DEVICE_CONDITION_WAIT, response)
        .await
        .map_err(|_| McpError::internal_error(format!("{operation} timed out"), None))?
        .map_err(|_| McpError::internal_error("device session ended", None))?
        .map_err(|error| McpError::internal_error(error, None))
}

#[tool_router]
impl DeviceHub {
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    fn new(
        frames: FrameSlot,
        browser_frames: crate::browser_video::BrowserVideoSlot,
        input: InputSink,
        orientation: OrientationSlot,
        devices: DeviceListSlot,
        active: ActiveSlot,
        error: ErrorSlot,
        status: StatusSlot,
        location: LocationStatusSlot,
        observability: McpObservability,
        control: UnboundedSender<ControlCmd>,
    ) -> Self {
        Self::new_with_service(
            DeviceControlService::new(frames, browser_frames, input),
            orientation,
            devices,
            active,
            error,
            status,
            location,
            observability,
            control,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_with_service(
        device_control: DeviceControlService,
        orientation: OrientationSlot,
        devices: DeviceListSlot,
        active: ActiveSlot,
        error: ErrorSlot,
        status: StatusSlot,
        location: LocationStatusSlot,
        observability: McpObservability,
        control: UnboundedSender<ControlCmd>,
    ) -> Self {
        Self {
            device_control,
            orientation,
            devices,
            active,
            error,
            status,
            location,
            observability,
            control,
            last_image: Arc::new(Mutex::new(None)),
            gesture_lock: Arc::new(tokio::sync::Mutex::new(())),
            tool_router: Self::tool_router(),
        }
    }

    fn to_device(&self, x: f32, y: f32, size: Option<(u32, u32)>) -> Option<(u16, u16)> {
        let turns = self.orientation.get().quarter_turns_cw();
        let (width, height) = size
            .or_else(|| *self.last_image.lock().unwrap())
            .or_else(|| {
                self.device_control
                    .latest_frame()
                    .map(|(_, frame)| display_dims(&frame, turns))
            })
            .or_else(|| {
                self.device_control
                    .browser_dimensions()
                    .map(|(width, height)| {
                        if turns.is_multiple_of(2) {
                            (width, height)
                        } else {
                            (height, width)
                        }
                    })
            })?;
        let dx = ((x + 0.5) / width as f32).clamp(0.0, 1.0);
        let dy = ((y + 0.5) / height as f32).clamp(0.0, 1.0);
        let (nx, ny) = unrotate_norm(dx, dy, turns);
        Some((norm(nx), norm(ny)))
    }

    fn send(&self, command: InputCmd) -> Result<(), McpError> {
        self.device_control
            .send(command)
            .map_err(|error| McpError::internal_error(error.to_string(), None))
    }

    fn frame_version(&self) -> u64 {
        self.device_control.frame_version()
    }

    async fn native_screenshot(&self) -> Result<(u32, u32, Vec<u8>), McpError> {
        let png = self
            .device_control
            .capture_screenshot(SCREENSHOT_WAIT)
            .await
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        let image = image::load_from_memory(&png)
            .map_err(|error| {
                McpError::internal_error(format!("device screenshot decode failed: {error}"), None)
            })?
            .to_rgb8();
        let (width, height) = image.dimensions();
        Ok((width, height, image.into_raw()))
    }

    async fn settle(&self) {
        tokio::time::sleep(SETTLE_MIN).await;
        if self.device_control.latest_frame().is_none() {
            return;
        }
        let started = Instant::now();
        let mut previous = self
            .device_control
            .latest_frame()
            .map(|(_, frame)| frame_signature(&frame));
        let mut stable = 0;
        while started.elapsed() < SETTLE_MAX {
            tokio::time::sleep(SETTLE_POLL).await;
            let current = self
                .device_control
                .latest_frame()
                .map(|(_, frame)| frame_signature(&frame));
            match (&previous, &current) {
                (Some(left), Some(right)) if signature_diff(left, right) < SETTLE_DIFF => {
                    stable += 1;
                    if stable >= SETTLE_STABLE_SAMPLES {
                        break;
                    }
                }
                _ => stable = 0,
            }
            previous = current;
        }
    }

    #[tool(
        description = "Capture the current iPhone screen as a PNG with a frame version. The returned image is the coordinate space for tap/swipe/multi_touch. A labeled 100px grid is enabled by default; max_dim defaults to 1024 and 0 keeps native resolution."
    )]
    async fn screenshot(
        &self,
        Parameters(params): Parameters<ScreenshotParams>,
    ) -> Result<CallToolResult, McpError> {
        let frame_version = self.frame_version();
        let (width, height, rgb) = if let Some((_, frame)) = self.device_control.latest_frame() {
            render_upright(&frame, self.orientation.get().quarter_turns_cw())
        } else {
            self.native_screenshot().await?
        };
        if rgb.is_empty() {
            return Err(McpError::internal_error("current frame is empty", None));
        }
        let requested = params.max_dim.unwrap_or(DEFAULT_MAX_DIM);
        let max_dim = if requested == 0 {
            0
        } else {
            requested.min(MAX_SCREENSHOT_DIM)
        };
        let (width, height, mut rgb) = downscale(rgb, width, height, max_dim);
        *self.last_image.lock().unwrap() = Some((width, height));
        if params.grid.unwrap_or(true) {
            draw_grid(&mut rgb, width, height);
        }
        let mut png = Vec::new();
        PngEncoder::new(&mut png)
            .write_image(&rgb, width, height, ExtendedColorType::Rgb8)
            .map_err(|error| {
                McpError::internal_error(format!("PNG encode failed: {error}"), None)
            })?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(png);
        Ok(CallToolResult::success(vec![
            Content::text(
                json!({
                    "frame_version": frame_version,
                    "image_width": width,
                    "image_height": height,
                    "origin": "top-left",
                    "coordinate_hint": "Pass image_width and image_height to coordinate-based actions."
                })
                .to_string(),
            ),
            Content::image(encoded, "image/png"),
        ]))
    }

    #[tool(
        description = "Tap once at a pixel coordinate in a screenshot. Supply image_width/image_height from that screenshot for deterministic scaling."
    )]
    async fn tap(
        &self,
        Parameters(params): Parameters<PointParams>,
    ) -> Result<CallToolResult, McpError> {
        let size = image_size(params.image_width, params.image_height)?;
        let Some((x, y)) = self.to_device(params.x, params.y, size) else {
            return ok_text("No screen available. Connect a device first.");
        };
        let hold_ms = params
            .hold_ms
            .unwrap_or(DEFAULT_TAP_HOLD_MS)
            .clamp(TAP_SAMPLE_MS, 5000);
        let samples = (hold_ms / TAP_SAMPLE_MS).clamp(1, 200);
        let interval = Duration::from_millis((hold_ms / samples).max(1));
        let frame_version = self.frame_version();
        let _gesture = self.gesture_lock.lock().await;
        self.send(InputCmd::TouchDown { x, y })?;
        for _ in 0..samples {
            tokio::time::sleep(interval).await;
            self.send(InputCmd::TouchMove { x, y })?;
        }
        self.send(InputCmd::TouchUp { x, y })?;
        if params.wait_for_settle.unwrap_or(true) {
            self.settle().await;
        }
        let frame_version_after = self.frame_version();
        ok_text(
            json!({
                "action": "tap",
                "x": params.x,
                "y": params.y,
                "hold_ms": hold_ms,
                "frame_version_before": frame_version,
                "frame_version_after": frame_version_after,
            })
            .to_string(),
        )
    }

    #[tool(
        description = "Swipe or drag between two screenshot pixel coordinates. duration_ms controls velocity."
    )]
    async fn swipe(
        &self,
        Parameters(params): Parameters<SwipeParams>,
    ) -> Result<CallToolResult, McpError> {
        let size = image_size(params.image_width, params.image_height)?;
        let Some((start_x, start_y)) = self.to_device(params.x1, params.y1, size) else {
            return ok_text("No screen available. Connect a device first.");
        };
        let Some((end_x, end_y)) = self.to_device(params.x2, params.y2, size) else {
            return ok_text("No screen available. Connect a device first.");
        };
        let duration = params.duration_ms.unwrap_or(300).clamp(50, 5000);
        let steps = (duration / 16).clamp(2, 150);
        let frame_version = self.frame_version();
        let _gesture = self.gesture_lock.lock().await;
        self.send(InputCmd::TouchDown {
            x: start_x,
            y: start_y,
        })?;
        for step in 1..=steps {
            let progress = step as f32 / steps as f32;
            let x = (start_x as f32 + (end_x as f32 - start_x as f32) * progress).round() as u16;
            let y = (start_y as f32 + (end_y as f32 - start_y as f32) * progress).round() as u16;
            self.send(InputCmd::TouchMove { x, y })?;
            tokio::time::sleep(Duration::from_millis((duration / steps).max(1))).await;
        }
        self.send(InputCmd::TouchUp { x: end_x, y: end_y })?;
        if params.wait_for_settle.unwrap_or(true) {
            self.settle().await;
        }
        let frame_version_after = self.frame_version();
        ok_text(
            json!({
                "action": "swipe",
                "from": [params.x1, params.y1],
                "to": [params.x2, params.y2],
                "duration_ms": duration,
                "frame_version_before": frame_version,
                "frame_version_after": frame_version_after,
            })
            .to_string(),
        )
    }

    #[tool(
        description = "Perform one to five simultaneous touch paths as a single HID multi-touch gesture. Use fixed start/end points for held game buttons. wait_for_settle defaults to false for low latency."
    )]
    async fn multi_touch(
        &self,
        Parameters(params): Parameters<MultiTouchParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_touch_count(params.contacts.len())?;
        let size = image_size(params.image_width, params.image_height)?;
        let paths = params
            .contacts
            .iter()
            .map(|contact| {
                Some((
                    self.to_device(contact.x1, contact.y1, size)?,
                    self.to_device(contact.x2, contact.y2, size)?,
                ))
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| McpError::internal_error("no screen available", None))?;
        let duration = params.duration_ms.unwrap_or(250).clamp(25, 5000);
        let steps = (duration / 16).clamp(2, 150);
        let interval = Duration::from_millis((duration / steps).max(1));
        let contacts_at = |progress: f32, touching: bool| {
            paths
                .iter()
                .enumerate()
                .map(|(identity, (start, end))| TouchContact {
                    identity: identity as u8,
                    touching,
                    x: (start.0 as f32 + (end.0 as f32 - start.0 as f32) * progress).round() as u16,
                    y: (start.1 as f32 + (end.1 as f32 - start.1 as f32) * progress).round() as u16,
                })
                .collect::<Vec<_>>()
        };
        let frame_version = self.frame_version();
        let _gesture = self.gesture_lock.lock().await;
        self.send(InputCmd::MultiTouchFrame(contacts_at(0.0, true)))?;
        for step in 1..=steps {
            let progress = step as f32 / steps as f32;
            self.send(InputCmd::MultiTouchFrame(contacts_at(progress, true)))?;
            tokio::time::sleep(interval).await;
        }
        self.send(InputCmd::MultiTouchFrame(contacts_at(1.0, false)))?;
        if params.wait_for_settle.unwrap_or(false) {
            self.settle().await;
        }
        let frame_version_after = self.frame_version();
        ok_text(
            json!({
                "action": "multi_touch",
                "contacts": paths.len(),
                "duration_ms": duration,
                "frame_version_before": frame_version,
                "frame_version_after": frame_version_after,
            })
            .to_string(),
        )
    }

    #[tool(
        description = "Wait for a newer decoded screen frame. Use frame_version from screenshot or frame_version_after from a low-latency action."
    )]
    async fn wait_for_frame(
        &self,
        Parameters(params): Parameters<WaitFrameParams>,
    ) -> Result<CallToolResult, McpError> {
        let after = params.after_version.unwrap_or_else(|| self.frame_version());
        let timeout = Duration::from_millis(params.timeout_ms.unwrap_or(2000).clamp(1, 10_000));
        let changed = self.device_control.wait_for_frame(after, timeout).await;
        ok_text(
            json!({
                "changed": changed,
                "frame_version": self.frame_version(),
            })
            .to_string(),
        )
    }

    #[tool(description = "Type printable text into the currently focused field.")]
    async fn type_text(
        &self,
        Parameters(params): Parameters<TextParams>,
    ) -> Result<CallToolResult, McpError> {
        let count = params.text.chars().count();
        self.send(InputCmd::Text(params.text))?;
        ok_text(format!("Typed {count} characters."))
    }

    #[tool(
        description = "Paste arbitrary Unicode text into the focused field through the iOS pasteboard and Cmd+V."
    )]
    async fn paste_text(
        &self,
        Parameters(params): Parameters<TextParams>,
    ) -> Result<CallToolResult, McpError> {
        let count = validate_paste_text(&params.text)
            .map_err(|error| McpError::invalid_params(error, None))?;
        let (reply, response) = oneshot::channel();
        self.send(InputCmd::PasteText {
            text: params.text,
            reply,
        })?;
        let result = tokio::time::timeout(CLIPBOARD_WAIT, response)
            .await
            .map_err(|_| McpError::internal_error("paste text timed out", None))?
            .map_err(|_| McpError::internal_error("device session ended", None))?;
        result.map_err(|error| McpError::internal_error(error, None))?;
        ok_text(format!("Pasted {count} characters."))
    }

    #[tool(
        description = "Press a special keyboard key: enter, escape, backspace, tab, delete, arrows, home, end, pageup or pagedown."
    )]
    async fn press_key(
        &self,
        Parameters(params): Parameters<KeyParams>,
    ) -> Result<CallToolResult, McpError> {
        let usage = key_usage(&params.key).ok_or_else(|| {
            McpError::invalid_params(format!("unknown key: {}", params.key), None)
        })?;
        self.send(InputCmd::KeyUsage(usage))?;
        self.settle().await;
        ok_text(format!("Pressed {}.", params.key))
    }

    #[tool(
        description = "Press an iPhone hardware button: home, lock, volume-up, volume-down, mute, siri or action."
    )]
    async fn press_button(
        &self,
        Parameters(params): Parameters<ButtonParams>,
    ) -> Result<CallToolResult, McpError> {
        let button = button_label(&params.button).ok_or_else(|| {
            McpError::invalid_params(format!("unknown hardware button: {}", params.button), None)
        })?;
        self.send(InputCmd::Button(button))?;
        self.settle().await;
        ok_text(format!("Pressed {button}."))
    }

    #[tool(
        description = "Lock the connected iPhone through Diagnostics Relay. Unlike press_button with lock, this is a one-way sleep request and will not wake an already locked device."
    )]
    async fn lock_device(&self) -> Result<CallToolResult, McpError> {
        let (reply, response) = oneshot::channel();
        self.send(InputCmd::LockDevice(reply))?;
        tokio::time::timeout(DEVICE_DETAILS_WAIT, response)
            .await
            .map_err(|_| McpError::internal_error("device lock request timed out", None))?
            .map_err(|_| McpError::internal_error("device session ended", None))?
            .map_err(|error| McpError::internal_error(error, None))?;
        ok_text(json!({ "lock_requested": true }).to_string())
    }

    #[tool(description = "Rotate the device 90 degrees left or right.")]
    async fn rotate(
        &self,
        Parameters(params): Parameters<RotateParams>,
    ) -> Result<CallToolResult, McpError> {
        let direction = match params.direction.to_ascii_lowercase().as_str() {
            "left" | "ccw" | "counterclockwise" => RotateDir::Left,
            "right" | "cw" | "clockwise" => RotateDir::Right,
            _ => {
                return Err(McpError::invalid_params(
                    "direction must be left or right",
                    None,
                ));
            }
        };
        self.send(InputCmd::Rotate(direction))?;
        self.settle().await;
        ok_text(format!("Rotated {}.", params.direction))
    }

    #[tool(
        description = "List launchable apps on the connected iPhone. User-installed apps are returned by default; include_system requests Apple default apps and include_app_clips requests App Clips through CoreDevice AppService. Filter by app name or bundle ID."
    )]
    async fn list_apps(
        &self,
        Parameters(params): Parameters<ListAppsParams>,
    ) -> Result<CallToolResult, McpError> {
        let include_system = params.include_system.unwrap_or(false);
        let include_app_clips = params.include_app_clips.unwrap_or(false);
        let (reply, response) = oneshot::channel();
        self.send(InputCmd::ListApps {
            include_system,
            include_app_clips,
            reply,
        })?;
        let mut apps = tokio::time::timeout(APP_WAIT, response)
            .await
            .map_err(|_| McpError::internal_error("app list request timed out", None))?
            .map_err(|_| McpError::internal_error("device session ended", None))?
            .map_err(|error| McpError::internal_error(error, None))?;
        let query = params.query.unwrap_or_default().trim().to_lowercase();
        apps.retain(|app| {
            (include_system || !app.is_first_party)
                && (query.is_empty()
                    || app.name.to_lowercase().contains(&query)
                    || app.bundle_id.to_lowercase().contains(&query))
        });
        apps.sort_by(|left, right| left.name.cmp(&right.name));
        let total = apps.len();
        apps.truncate(params.limit.unwrap_or(100).clamp(1, 200));
        let returned = apps.len();
        ok_text(
            json!({
                "apps": apps,
                "returned": returned,
                "total_matches": total,
            })
            .to_string(),
        )
    }

    #[tool(
        description = "List Apple Watch devices paired with the connected iPhone. Returns read-only name, product type, and watchOS version metadata when available."
    )]
    async fn list_companion_devices(&self) -> Result<CallToolResult, McpError> {
        let (reply, response) = oneshot::channel();
        self.send(InputCmd::ListCompanionDevices(reply))?;
        let devices = tokio::time::timeout(APP_WAIT, response)
            .await
            .map_err(|_| McpError::internal_error("companion device request timed out", None))?
            .map_err(|_| McpError::internal_error("device session ended", None))?
            .map_err(|error| McpError::internal_error(error, None))?;
        let count = devices.len();
        ok_text(
            json!({
                "devices": devices,
                "count": count,
            })
            .to_string(),
        )
    }

    #[tool(
        description = "Read the connected device's home-screen application locations. Positions are 1-based ordinal positions, not pixel or tap coordinates. Returns Dock/page placement and folder routes while omitting widgets, private UUIDs, Web Clip URLs, and raw SpringBoard configuration."
    )]
    async fn home_screen_layout(&self) -> Result<CallToolResult, McpError> {
        let (reply, response) = oneshot::channel();
        self.send(InputCmd::GetHomeScreenLayout(reply))?;
        let layout = tokio::time::timeout(APP_WAIT, response)
            .await
            .map_err(|_| McpError::internal_error("home screen layout request timed out", None))?
            .map_err(|_| McpError::internal_error("device session ended", None))?
            .map_err(|error| McpError::internal_error(error, None))?;
        ok_text(serde_json::to_string(&layout).map_err(|error| {
            McpError::internal_error(
                format!("unable to serialize home screen layout: {error}"),
                None,
            )
        })?)
    }

    #[tool(
        description = "List a bounded, read-only inventory of processes currently running on the connected device through DVT DeviceInfo. Returns PID, sanitized process/app names, and whether iOS classifies each entry as an application. This tool cannot terminate or inspect arbitrary process memory."
    )]
    async fn list_processes(&self) -> Result<CallToolResult, McpError> {
        let (reply, response) = oneshot::channel();
        self.send(InputCmd::ListRunningProcesses(reply))?;
        let list = tokio::time::timeout(APP_WAIT, response)
            .await
            .map_err(|_| McpError::internal_error("running process request timed out", None))?
            .map_err(|_| McpError::internal_error("device session ended", None))?
            .map_err(|error| McpError::internal_error(error, None))?;
        ok_text(serde_json::to_string(&list).map_err(|error| {
            McpError::internal_error(
                format!("unable to serialize running process inventory: {error}"),
                None,
            )
        })?)
    }

    #[tool(
        description = "Read DeviceHub Mask's supervised WebDriverAgent Runner state. This reports only a runner started by DeviceHub Mask; call wda_status separately to probe an externally managed WDA."
    )]
    async fn wda_runner_status(&self) -> Result<CallToolResult, McpError> {
        let (reply, response) = oneshot::channel();
        self.send(InputCmd::WdaRunner(
            crate::wda_runner::WdaRunnerCommand::Status { reply },
        ))?;
        let status = tokio::time::timeout(APP_WAIT, response)
            .await
            .map_err(|_| McpError::internal_error("WDA runner status timed out", None))?
            .map_err(|_| McpError::internal_error("device session ended", None))?;
        ok_text(serde_json::to_string(&status).map_err(|error| {
            McpError::internal_error(
                format!("WDA runner status serialization failed: {error}"),
                None,
            )
        })?)
    }

    #[tool(
        description = "Explicitly start one installed, developer-signed WebDriverAgent .xctrunner through XCTest and wait up to 30 seconds for WDA readiness. Use list_apps to discover an eligible runner. This does not install or sign WDA."
    )]
    async fn wda_start(
        &self,
        Parameters(params): Parameters<WdaStartParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::wda_runner::validate_runner_bundle_id(&params.runner_bundle_id)
            .map_err(|error| McpError::invalid_params(error, None))?;
        let (reply, response) = oneshot::channel();
        self.send(InputCmd::WdaRunner(
            crate::wda_runner::WdaRunnerCommand::Start {
                bundle_id: params.runner_bundle_id,
                reply,
            },
        ))?;
        let status = tokio::time::timeout(WDA_RUNNER_START_WAIT, response)
            .await
            .map_err(|_| McpError::internal_error("WDA runner startup timed out", None))?
            .map_err(|_| McpError::internal_error("device session ended", None))?
            .map_err(|error| McpError::internal_error(error, None))?;
        ok_text(serde_json::to_string(&status).map_err(|error| {
            McpError::internal_error(
                format!("WDA runner status serialization failed: {error}"),
                None,
            )
        })?)
    }

    #[tool(
        description = "Stop only the WebDriverAgent Runner owned by DeviceHub Mask. This does not terminate an externally managed WDA instance."
    )]
    async fn wda_stop(&self) -> Result<CallToolResult, McpError> {
        let (reply, response) = oneshot::channel();
        self.send(InputCmd::WdaRunner(
            crate::wda_runner::WdaRunnerCommand::Stop { reply },
        ))?;
        let status = tokio::time::timeout(APP_WAIT, response)
            .await
            .map_err(|_| McpError::internal_error("WDA runner stop timed out", None))?
            .map_err(|_| McpError::internal_error("device session ended", None))?
            .map_err(|error| McpError::internal_error(error, None))?;
        ok_text(serde_json::to_string(&status).map_err(|error| {
            McpError::internal_error(
                format!("WDA runner status serialization failed: {error}"),
                None,
            )
        })?)
    }

    #[tool(
        description = "Probe an already-running WebDriverAgent on the connected device, whether externally managed or started with wda_start. This performs no background polling."
    )]
    async fn wda_status(&self) -> Result<CallToolResult, McpError> {
        let (reply, response) = oneshot::channel();
        self.send(InputCmd::WdaAutomation(
            crate::wda_automation::WdaAutomationCommand::Status {
                expires_at: tokio::time::Instant::now() + WDA_COMMAND_DEADLINE,
                reply,
            },
        ))?;
        let status = tokio::time::timeout(WDA_WAIT, response)
            .await
            .map_err(|_| McpError::internal_error("WDA status request timed out", None))?
            .map_err(|_| McpError::internal_error("device session ended", None))?
            .map_err(|error| McpError::internal_error(error, None))?;
        ok_text(serde_json::to_string(&status).map_err(|error| {
            McpError::internal_error(format!("WDA status serialization failed: {error}"), None)
        })?)
    }

    #[tool(
        description = "Read a size-bounded XML accessibility tree from an already-running WebDriverAgent. The tree may contain sensitive on-screen text."
    )]
    async fn wda_ui_tree(
        &self,
        Parameters(params): Parameters<WdaUiTreeParams>,
    ) -> Result<CallToolResult, McpError> {
        let max_characters = params
            .max_characters
            .unwrap_or(crate::wda_automation::DEFAULT_SOURCE_CHARS);
        if !(1..=crate::wda_automation::MAX_SOURCE_CHARS).contains(&max_characters) {
            return Err(McpError::invalid_params(
                format!(
                    "max_characters must be between 1 and {}",
                    crate::wda_automation::MAX_SOURCE_CHARS
                ),
                None,
            ));
        }
        let (reply, response) = oneshot::channel();
        self.send(InputCmd::WdaAutomation(
            crate::wda_automation::WdaAutomationCommand::Source {
                max_characters,
                expires_at: tokio::time::Instant::now() + WDA_COMMAND_DEADLINE,
                reply,
            },
        ))?;
        let tree = tokio::time::timeout(WDA_WAIT, response)
            .await
            .map_err(|_| McpError::internal_error("WDA UI tree request timed out", None))?
            .map_err(|_| McpError::internal_error("device session ended", None))?
            .map_err(|error| McpError::internal_error(error, None))?;
        ok_text(serde_json::to_string(&tree).map_err(|error| {
            McpError::internal_error(format!("WDA UI tree serialization failed: {error}"), None)
        })?)
    }

    #[tool(
        description = "Find up to 20 accessibility elements through an already-running WebDriverAgent and return zero-based match indexes with screen rectangles."
    )]
    async fn wda_find_elements(
        &self,
        Parameters(params): Parameters<WdaFindParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::wda_automation::validate_selector(&params.using, &params.value)
            .map_err(|error| McpError::invalid_params(error, None))?;
        let limit = params.limit.unwrap_or(10);
        if !(1..=crate::wda_automation::MAX_ELEMENTS).contains(&limit) {
            return Err(McpError::invalid_params(
                format!(
                    "limit must be between 1 and {}",
                    crate::wda_automation::MAX_ELEMENTS
                ),
                None,
            ));
        }
        let (reply, response) = oneshot::channel();
        self.send(InputCmd::WdaAutomation(
            crate::wda_automation::WdaAutomationCommand::Find {
                using: params.using,
                value: params.value,
                limit,
                expires_at: tokio::time::Instant::now() + WDA_COMMAND_DEADLINE,
                reply,
            },
        ))?;
        let elements = tokio::time::timeout(WDA_WAIT, response)
            .await
            .map_err(|_| McpError::internal_error("WDA element search timed out", None))?
            .map_err(|_| McpError::internal_error("device session ended", None))?
            .map_err(|error| McpError::internal_error(error, None))?;
        ok_text(
            json!({
                "elements": elements,
                "returned": elements.len(),
                "indexes_are_zero_based": true,
            })
            .to_string(),
        )
    }

    #[tool(
        description = "Find and click one accessibility element through an already-running WebDriverAgent. index is the zero-based order returned by wda_find_elements."
    )]
    async fn wda_click(
        &self,
        Parameters(params): Parameters<WdaClickParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::wda_automation::validate_selector(&params.using, &params.value)
            .map_err(|error| McpError::invalid_params(error, None))?;
        let index = params.index.unwrap_or(0);
        if index >= crate::wda_automation::MAX_ELEMENTS {
            return Err(McpError::invalid_params(
                format!(
                    "index must be between 0 and {}",
                    crate::wda_automation::MAX_ELEMENTS - 1
                ),
                None,
            ));
        }
        let (reply, response) = oneshot::channel();
        let _gesture = self.gesture_lock.lock().await;
        self.send(InputCmd::WdaAutomation(
            crate::wda_automation::WdaAutomationCommand::Click {
                using: params.using,
                value: params.value,
                index,
                expires_at: tokio::time::Instant::now() + WDA_COMMAND_DEADLINE,
                reply,
            },
        ))?;
        let element = tokio::time::timeout(WDA_WAIT, response)
            .await
            .map_err(|_| McpError::internal_error("WDA element click timed out", None))?
            .map_err(|_| McpError::internal_error("device session ended", None))?
            .map_err(|error| McpError::internal_error(error, None))?;
        ok_text(json!({ "clicked": true, "element": element }).to_string())
    }

    #[tool(
        description = "Launch an installed app by bundle ID, or restart it when already running, and optionally wait for its screen to become stable. Use list_apps to discover bundle IDs and running state."
    )]
    async fn launch_app(
        &self,
        Parameters(params): Parameters<AppParams>,
    ) -> Result<CallToolResult, McpError> {
        if !valid_bundle_identifier(&params.bundle_id) {
            return Err(McpError::invalid_params("invalid bundle identifier", None));
        }
        let frame_version = self.frame_version();
        let _gesture = self.gesture_lock.lock().await;
        let (reply, response) = oneshot::channel();
        self.send(InputCmd::LaunchApp {
            bundle_id: params.bundle_id.clone(),
            reply,
        })?;
        tokio::time::timeout(APP_WAIT, response)
            .await
            .map_err(|_| McpError::internal_error("app launch request timed out", None))?
            .map_err(|_| McpError::internal_error("device session ended", None))?
            .map_err(|error| McpError::internal_error(error, None))?;
        if params.wait_for_settle.unwrap_or(true) {
            self.settle().await;
        }
        let frame_version_after = self.frame_version();
        ok_text(
            json!({
                "launched": params.bundle_id,
                "frame_version_before": frame_version,
                "frame_version_after": frame_version_after,
            })
            .to_string(),
        )
    }

    #[tool(
        description = "Stop a running user app by bundle ID. The server resolves the app's current main process and sends SIGTERM; callers cannot provide a PID or signal."
    )]
    async fn stop_app(
        &self,
        Parameters(params): Parameters<StopAppParams>,
    ) -> Result<CallToolResult, McpError> {
        if !valid_bundle_identifier(&params.bundle_id) {
            return Err(McpError::invalid_params("invalid bundle identifier", None));
        }
        let frame_version = self.frame_version();
        let _gesture = self.gesture_lock.lock().await;
        let (reply, response) = oneshot::channel();
        self.send(InputCmd::StopApp {
            bundle_id: params.bundle_id.clone(),
            reply,
        })?;
        let was_running = tokio::time::timeout(APP_WAIT, response)
            .await
            .map_err(|_| McpError::internal_error("app stop request timed out", None))?
            .map_err(|_| McpError::internal_error("device session ended", None))?
            .map_err(|error| McpError::internal_error(error, None))?;
        if params.wait_for_settle.unwrap_or(true) {
            self.settle().await;
        }
        ok_text(
            json!({
                "stopped": params.bundle_id,
                "was_running": was_running,
                "frame_version_before": frame_version,
                "frame_version_after": self.frame_version(),
            })
            .to_string(),
        )
    }

    #[tool(
        description = "List recent device crash reports through the active session. Results are newest first and contain metadata only; use read_crash_report with an exact returned device_path to inspect one bounded report."
    )]
    async fn list_crash_reports(
        &self,
        Parameters(params): Parameters<CrashReportListParams>,
    ) -> Result<CallToolResult, McpError> {
        let (reply, response) = oneshot::channel();
        self.send(InputCmd::ListCrashReports(reply))?;
        let mut list = tokio::time::timeout(CRASH_REPORT_WAIT, response)
            .await
            .map_err(|_| McpError::internal_error("crash report list request timed out", None))?
            .map_err(|_| McpError::internal_error("device session ended", None))?
            .map_err(|error| McpError::internal_error(error, None))?;
        let query = params.query.unwrap_or_default().trim().to_ascii_lowercase();
        if !query.is_empty() {
            list.reports.retain(|report| {
                report.name.to_ascii_lowercase().contains(&query)
                    || report.path.to_ascii_lowercase().contains(&query)
            });
        }
        let total_matches = list.reports.len();
        list.reports
            .truncate(params.limit.unwrap_or(50).clamp(1, 200));
        let returned = list.reports.len();
        ok_text(
            json!({
                "reports": list.reports,
                "returned": returned,
                "total_matches": total_matches,
                "source_truncated": list.truncated,
            })
            .to_string(),
        )
    }

    #[tool(
        description = "Read a size-bounded device crash report for diagnosis together with a normalized summary. device_path must be an exact absolute path returned by list_crash_reports. The default limit is 256 KiB and the hard limit is 1 MiB."
    )]
    async fn read_crash_report(
        &self,
        Parameters(params): Parameters<CrashReportReadParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::crash_reports::validate_device_path(&params.device_path)
            .map_err(|error| McpError::invalid_params(error, None))?;
        let max_bytes = params.max_bytes.unwrap_or(DEFAULT_CRASH_REPORT_BYTES);
        if !(1..=crate::crash_reports::MAX_READ_BYTES).contains(&max_bytes) {
            return Err(McpError::invalid_params(
                format!(
                    "max_bytes must be between 1 and {}",
                    crate::crash_reports::MAX_READ_BYTES
                ),
                None,
            ));
        }
        let (reply, response) = oneshot::channel();
        self.send(InputCmd::ReadCrashReport {
            device_path: params.device_path,
            max_bytes,
            reply,
        })?;
        let report = tokio::time::timeout(CRASH_REPORT_WAIT, response)
            .await
            .map_err(|_| McpError::internal_error("crash report read request timed out", None))?
            .map_err(|_| McpError::internal_error("device session ended", None))?
            .map_err(|error| McpError::internal_error(error, None))?;
        ok_text(serde_json::to_string(&report).map_err(|error| {
            McpError::internal_error(format!("crash report serialization failed: {error}"), None)
        })?)
    }

    #[tool(
        description = "Return refreshed Lockdown device metadata, normalized language, locale, time zone and clock format, storage, battery, activation, Developer Mode, and Developer Disk Image state. Stable hardware identifiers are omitted unless include_identifiers is true."
    )]
    async fn device_details(
        &self,
        Parameters(params): Parameters<DeviceDetailsParams>,
    ) -> Result<CallToolResult, McpError> {
        let (reply, response) = oneshot::channel();
        self.send(InputCmd::GetDeviceDetails(reply))?;
        let details = tokio::time::timeout(DEVICE_DETAILS_WAIT, response)
            .await
            .map_err(|_| McpError::internal_error("device metadata request timed out", None))?
            .map_err(|_| McpError::internal_error("device session ended", None))?
            .map_err(|error| McpError::internal_error(error, None))?;
        let identifiers = params.include_identifiers.unwrap_or(false).then(|| {
            json!({
                "udid": details.udid,
                "serial_number": details.serial_number,
                "ecid": details.ecid,
            })
        });
        ok_text(
            json!({
                "name": details.name,
                "product_type": details.product_type,
                "product_version": details.product_version,
                "build_version": details.build_version,
                "hardware_model": details.hardware_model,
                "total_disk_capacity": details.total_disk_capacity,
                "storage": details.storage,
                "activation_state": details.activation_state,
                "developer_mode_enabled": details.developer_mode_enabled,
                "developer_image_mounted": details.developer_image_mounted,
                "regional_settings": details.regional_settings,
                "battery": details.battery,
                "identifiers": identifiers,
            })
            .to_string(),
        )
    }

    #[tool(
        description = "Wait for a normalized device event: app_installed, app_uninstalled, activation_state_changed, disk_usage_changed, device_name_changed, or lock_state_changed. A lock_state_changed event reports only that the state changed; take a screenshot to observe the current screen. Pass the returned sequence as after_sequence for race-free incremental waiting."
    )]
    async fn wait_for_device_event(
        &self,
        Parameters(params): Parameters<DeviceEventParams>,
    ) -> Result<CallToolResult, McpError> {
        let baseline = params.after_sequence.unwrap_or_else(|| {
            self.observability
                .device_events
                .latest()
                .map_or(0, |event| event.sequence)
        });
        let mut receiver = self.observability.device_events.subscribe();
        if let Some(event) = self
            .observability
            .device_events
            .latest()
            .filter(|event| event.sequence > baseline)
        {
            return ok_text(
                json!({
                    "changed": true,
                    "event": event,
                })
                .to_string(),
            );
        }
        let wait =
            Duration::from_millis(params.timeout_ms.unwrap_or(10_000)).min(DEVICE_EVENT_WAIT_MAX);
        let event = tokio::time::timeout(wait, async {
            loop {
                match receiver.recv().await {
                    Ok(event) if event.sequence > baseline => return Ok(event),
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        if let Some(event) = self
                            .observability
                            .device_events
                            .latest()
                            .filter(|event| event.sequence > baseline)
                        {
                            return Ok(event);
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        return Err(McpError::internal_error("device event stream ended", None));
                    }
                }
            }
        })
        .await;
        match event {
            Ok(Ok(event)) => ok_text(
                json!({
                    "changed": true,
                    "event": event,
                })
                .to_string(),
            ),
            Ok(Err(error)) => Err(error),
            Err(_) => ok_text(
                json!({
                    "changed": false,
                    "after_sequence": baseline,
                    "latest_sequence": self
                        .observability
                        .device_events
                        .latest()
                        .map(|event| event.sequence),
                })
                .to_string(),
            ),
        }
    }

    #[tool(
        description = "List attached iOS device transports with stable selection ID, UDID, name, connection type, pairing state, and active state. Use the selection ID to distinguish USB from Wi-Fi."
    )]
    async fn list_devices(&self) -> Result<CallToolResult, McpError> {
        let active = self.active.selection_id();
        let listed = self.devices.get();
        let devices: Vec<_> = listed
            .into_iter()
            .map(|device| {
                let is_active = active.as_deref() == Some(device.id.as_str());
                json!({
                    "id": device.id,
                    "udid": device.udid,
                    "name": device.name,
                    "connection": device.connection.label(),
                    "pairing": device.pairing,
                    "active": is_active,
                })
            })
            .collect();
        ok_text(json!({ "devices": devices }).to_string())
    }

    async fn switch_device(
        &self,
        udid: String,
        reconnect: bool,
    ) -> Result<CallToolResult, McpError> {
        let devices = self.devices.get();
        let selected = devices
            .iter()
            .find(|device| device.id == udid)
            .or_else(|| devices.iter().find(|device| device.udid == udid));
        if selected
            .is_some_and(|device| device.pairing == crate::protocol::DevicePairingState::Unpaired)
        {
            return ok_text(
                "This USB device has not trusted the computer. Complete pairing from the DeviceHub Mask desktop device picker first.",
            );
        }
        if !reconnect
            && self.active.get().as_deref() == Some(udid.as_str())
            && self.device_control.latest_frame().is_some()
        {
            return ok_text(format!("Already connected to {udid}; screen is streaming."));
        }
        let previous_version = self.frame_version();
        let previous_error = self.error.get();
        let mut error_was_cleared = previous_error.is_none();
        let command = if reconnect {
            ControlCmd::Reconnect(udid.clone())
        } else {
            ControlCmd::Connect(udid.clone())
        };
        self.control
            .send(command)
            .map_err(|_| McpError::internal_error("device session manager is not running", None))?;
        let started = Instant::now();
        while started.elapsed() < DEVICE_WAIT {
            if self.active.get().as_deref() == Some(udid.as_str())
                && self.frame_version() > previous_version
            {
                return ok_text(format!("Connected to {udid}; screen is streaming."));
            }
            match self.error.get() {
                None => error_was_cleared = true,
                Some(error)
                    if error_was_cleared || previous_error.as_deref() != Some(error.as_str()) =>
                {
                    return ok_text(format!("Failed to connect to {udid}: {error}"));
                }
                Some(_) => {}
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        ok_text(format!(
            "Connection to {udid} is still being established; call status or screenshot shortly."
        ))
    }

    #[tool(
        description = "Connect to an attached device by selection ID from list_devices (or a legacy UDID) and wait for a new screen frame."
    )]
    async fn connect_device(
        &self,
        Parameters(params): Parameters<DeviceParams>,
    ) -> Result<CallToolResult, McpError> {
        self.switch_device(params.udid, false).await
    }

    #[tool(
        description = "Tear down and reconnect a device by selection ID from list_devices (or a legacy UDID), then wait for a new screen frame."
    )]
    async fn reconnect_device(
        &self,
        Parameters(params): Parameters<DeviceParams>,
    ) -> Result<CallToolResult, McpError> {
        self.switch_device(params.udid, true).await
    }

    #[tool(
        description = "Set a fixed simulated GPS location through the active device's DVT or legacy location service."
    )]
    async fn set_location(
        &self,
        Parameters(params): Parameters<LocationParams>,
    ) -> Result<CallToolResult, McpError> {
        if !params.latitude.is_finite()
            || !params.longitude.is_finite()
            || !(-90.0..=90.0).contains(&params.latitude)
            || !(-180.0..=180.0).contains(&params.longitude)
        {
            return Err(McpError::invalid_params(
                "invalid latitude or longitude",
                None,
            ));
        }
        let (reply, response) = oneshot::channel();
        self.send(InputCmd::SetLocation {
            latitude: params.latitude,
            longitude: params.longitude,
            reply,
        })?;
        let result = tokio::time::timeout(LOCATION_WAIT, response)
            .await
            .map_err(|_| McpError::internal_error("set location timed out", None))?
            .map_err(|_| McpError::internal_error("device session ended", None))?;
        result.map_err(|error| McpError::internal_error(error, None))?;
        ok_text("Simulated location applied.")
    }

    #[tool(description = "Stop the active DVT GPS location simulation.")]
    async fn clear_location(&self) -> Result<CallToolResult, McpError> {
        let (reply, response) = oneshot::channel();
        self.send(InputCmd::ClearLocation { reply })?;
        let result = tokio::time::timeout(LOCATION_WAIT, response)
            .await
            .map_err(|_| McpError::internal_error("clear location timed out", None))?
            .map_err(|_| McpError::internal_error("device session ended", None))?;
        result.map_err(|error| McpError::internal_error(error, None))?;
        ok_text("Location simulation stopped.")
    }

    #[tool(
        description = "List the bounded network and thermal condition profiles reported by the active device through DVT, including the active profile and cleanup state. This does not change the device."
    )]
    async fn list_device_conditions(&self) -> Result<CallToolResult, McpError> {
        ok_text(
            serde_json::to_string(&self.observability.device_conditions.get())
                .map_err(|error| McpError::internal_error(error.to_string(), None))?,
        )
    }

    #[tool(
        description = "Apply one enumerated DVT network or thermal condition to the entire active device. Identifiers must come from list_device_conditions. This can interrupt connectivity; call clear_device_condition when the test ends."
    )]
    async fn apply_device_condition(
        &self,
        Parameters(params): Parameters<DeviceConditionParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::device_conditions::validate_identifiers(
            &params.group_identifier,
            &params.profile_identifier,
        )
        .map_err(|error| McpError::invalid_params(error, None))?;
        let (reply, response) = oneshot::channel();
        self.send(InputCmd::DeviceCondition(
            crate::device_conditions::DeviceConditionCommand::Apply {
                group_identifier: params.group_identifier,
                profile_identifier: params.profile_identifier,
                expires_at: tokio::time::Instant::now() + DEVICE_CONDITION_COMMAND_DEADLINE,
                reply,
            },
        ))?;
        await_device_condition(response, "apply device condition").await?;
        ok_text(
            serde_json::to_string(&self.observability.device_conditions.get())
                .map_err(|error| McpError::internal_error(error.to_string(), None))?,
        )
    }

    #[tool(
        description = "Restore normal device-wide conditions by disabling the active DVT network or thermal profile. Call this after every condition test, including after a failed test."
    )]
    async fn clear_device_condition(&self) -> Result<CallToolResult, McpError> {
        let (reply, response) = oneshot::channel();
        self.send(InputCmd::DeviceCondition(
            crate::device_conditions::DeviceConditionCommand::Clear {
                expires_at: tokio::time::Instant::now() + DEVICE_CONDITION_COMMAND_DEADLINE,
                reply,
            },
        ))?;
        await_device_condition(response, "clear device condition").await?;
        ok_text(
            serde_json::to_string(&self.observability.device_conditions.get())
                .map_err(|error| McpError::internal_error(error.to_string(), None))?,
        )
    }

    #[tool(
        description = "Collect a bounded DVT performance snapshot from the active device, including CPU, top processes, graphics, GPU, energy, and network rates. Sampling is enabled only for this call unless the desktop performance workspace is already active."
    )]
    async fn performance_snapshot(
        &self,
        Parameters(params): Parameters<PerformanceSnapshotParams>,
    ) -> Result<CallToolResult, McpError> {
        let sampling_continues = self.observability.performance_demand.enabled();
        let baseline = self.observability.performance.get().captured_at_ms;
        let wait = Duration::from_millis(
            params
                .wait_ms
                .unwrap_or(PERFORMANCE_WAIT_DEFAULT.as_millis() as u64),
        )
        .min(OBSERVABILITY_WAIT_MAX);
        let _lease = self.observability.performance_demand.acquire();
        let deadline = Instant::now() + wait;
        let sample = loop {
            let sample = self.observability.performance.get();
            if wait.is_zero()
                || (sample.captured_at_ms != 0 && sample.captured_at_ms > baseline)
                || Instant::now() >= deadline
            {
                break sample;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        let fresh = sample.captured_at_ms != 0 && sample.captured_at_ms > baseline;
        ok_text(
            json!({
                "available": sample.captured_at_ms != 0,
                "fresh": fresh,
                "sampling_continues": sampling_continues,
                "sample": sample,
            })
            .to_string(),
        )
    }

    #[tool(
        description = "Return bounded recent logs from the active device. Supports sequence cursors, level filtering, case-insensitive text search, and a short wait for new matching entries. Collection is enabled only for this call unless Device Logs is already active in the desktop app."
    )]
    async fn recent_device_logs(
        &self,
        Parameters(params): Parameters<DeviceLogParams>,
    ) -> Result<CallToolResult, McpError> {
        let level = parse_log_level(params.level.as_deref())?;
        let query = params
            .query
            .as_deref()
            .map(str::trim)
            .filter(|query| !query.is_empty());
        let limit = params
            .limit
            .unwrap_or(100)
            .clamp(1, crate::device_logs::MAX_BATCH_ENTRIES);
        let wait = Duration::from_millis(
            params
                .wait_ms
                .unwrap_or(DEVICE_LOG_WAIT_DEFAULT.as_millis() as u64),
        )
        .min(OBSERVABILITY_WAIT_MAX);
        let streaming_continues = self.observability.device_log_demand.enabled();
        let _lease = self.observability.device_log_demand.acquire();
        let deadline = Instant::now() + wait;
        let (batch, mut entries) = loop {
            let batch = self.observability.device_logs.snapshot(
                params.after,
                crate::device_logs::MAX_BATCH_ENTRIES,
                true,
            );
            let entries = batch
                .entries
                .iter()
                .filter(|entry| log_entry_matches(entry, level, query))
                .cloned()
                .collect::<Vec<_>>();
            if !entries.is_empty() || wait.is_zero() || Instant::now() >= deadline {
                break (batch, entries);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        let has_more = batch.has_more || entries.len() > limit;
        if entries.len() > limit {
            if params.after.is_none() {
                entries.drain(..entries.len() - limit);
            } else {
                entries.truncate(limit);
            }
        }
        ok_text(
            json!({
                "entries": entries,
                "oldest_sequence": batch.oldest_sequence,
                "latest_sequence": batch.latest_sequence,
                "cursor_lagged": batch.cursor_lagged,
                "has_more": has_more,
                "source": batch.source,
                "streaming_continues": streaming_continues,
                "filters": {
                    "level": params.level,
                    "query": params.query,
                },
            })
            .to_string(),
        )
    }

    #[tool(
        description = "Report active device, stream state, screen size, orientation and virtual-location state."
    )]
    async fn status(&self) -> Result<CallToolResult, McpError> {
        let screen_size = self
            .device_control
            .latest_frame()
            .map(|(_, frame)| {
                let (width, height) =
                    display_dims(&frame, self.orientation.get().quarter_turns_cw());
                json!([width, height])
            })
            .or_else(|| {
                let turns = self.orientation.get().quarter_turns_cw();
                self.device_control
                    .browser_dimensions()
                    .map(|(width, height)| {
                        let (width, height) = if turns.is_multiple_of(2) {
                            (width, height)
                        } else {
                            (height, width)
                        };
                        json!([width, height])
                    })
            })
            .unwrap_or(json!(null));
        let orientation = match self.orientation.get() {
            Orientation::Portrait => "portrait",
            Orientation::PortraitUpsideDown => "portrait-upside-down",
            Orientation::LandscapeLeft => "landscape-left",
            Orientation::LandscapeRight => "landscape-right",
        };
        ok_text(
            json!({
                "active_udid": self.active.get(),
                "status": self.status.get(),
                "error": self.error.get(),
                "streaming": self.device_control.latest_frame().is_some() || self.device_control.browser_dimensions().is_some(),
                "screen_size": screen_size,
                "orientation": orientation,
                "location": self.location.get(),
            })
            .to_string(),
        )
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for DeviceHub {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_instructions("Control the connected iPhone by calling screenshot, then an input tool, then screenshot again. Coordinates are pixels in the screenshot; include image_width and image_height in actions. For games, use multi_touch for simultaneous controls and set wait_for_settle=false on tap/swipe, then call wait_for_frame with frame_version_after. For semantic accessibility automation, use wda_status, wda_ui_tree, wda_find_elements, and wda_click. If WDA is not already reachable, list_apps can discover an installed developer .xctrunner for explicit wda_start; wda_stop affects only a runner DeviceHub Mask started. DeviceHub Mask never installs or signs WDA. Use list_apps with launch_app or stop_app for app lifecycle control, home_screen_layout for ordinal Dock/page/folder context, and list_devices/connect_device when no device is active. Use lock_device for a one-way lock request; press_button with lock toggles the hardware button and can wake an already locked device. Use device_details for battery and system context, list_companion_devices for paired Apple Watch context, and wait_for_device_event instead of polling for app, storage, or name changes. Use list_processes, performance_snapshot, and recent_device_logs to diagnose device-side behavior. For network or thermal testing, select only identifiers returned by list_device_conditions and always call clear_device_condition afterward. After an app unexpectedly exits, call list_crash_reports and read_crash_report to inspect a bounded device crash report.")
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn serve(
    device_control: DeviceControlService,
    orientation: OrientationSlot,
    devices: DeviceListSlot,
    active: ActiveSlot,
    error: ErrorSlot,
    status: StatusSlot,
    location: LocationStatusSlot,
    device_events: DeviceEventSlot,
    device_conditions: crate::device_conditions::DeviceConditionSlot,
    performance: PerformanceSlot,
    performance_demand: PerformanceDemand,
    device_logs: DeviceLogSlot,
    device_log_demand: DeviceLogDemand,
    control: UnboundedSender<ControlCmd>,
) {
    let address = std::env::var("DEVICEHUB_MCP_ADDR").unwrap_or_else(|_| DEFAULT_ADDR.into());
    if !address.starts_with("127.0.0.1:")
        && !address.starts_with("[::1]:")
        && !address.starts_with("localhost:")
    {
        tracing::warn!(
            address,
            "MCP has no authentication and is binding beyond loopback"
        );
    }
    let hub = DeviceHub::new_with_service(
        device_control,
        orientation,
        devices,
        active,
        error,
        status,
        location,
        McpObservability {
            device_events,
            device_conditions,
            performance,
            performance_demand,
            device_logs,
            device_log_demand,
        },
        control,
    );
    let router = service_router(hub);
    match tokio::net::TcpListener::bind(&address).await {
        Ok(listener) => {
            tracing::info!(address = %address, "MCP server listening");
            if let Err(error) = axum::serve(listener, router).await {
                tracing::error!(error = %error, "MCP server stopped");
            }
        }
        Err(error) => {
            tracing::warn!(address = %address, error = %error, "MCP server failed to bind")
        }
    }
}

fn service_router(hub: DeviceHub) -> axum::Router {
    let service = StreamableHttpService::new(
        move || Ok(hub.clone()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );
    axum::Router::new().nest_service("/mcp", service)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::{
        ServiceExt,
        model::{CallToolRequestParams, ClientInfo},
        transport::StreamableHttpClientTransport,
    };

    fn test_device_details() -> crate::protocol::DeviceDetails {
        crate::protocol::DeviceDetails {
            udid: "private-udid".into(),
            name: "Test iPhone".into(),
            product_type: "iPhone14,3".into(),
            product_version: "27.0".into(),
            build_version: Some("24A123".into()),
            hardware_model: Some("D64AP".into()),
            serial_number: Some("private-serial".into()),
            ecid: Some("123456789".into()),
            total_disk_capacity: Some(256_000_000_000),
            storage: None,
            activation_state: Some(crate::protocol::DeviceActivationState::Activated),
            developer_mode_enabled: Some(true),
            developer_image_mounted: Some(true),
            regional_settings: Some(crate::protocol::DeviceRegionalSettings {
                language: Some("zh-Hant".into()),
                locale: Some("zh_TW".into()),
                time_zone: Some("Asia/Taipei".into()),
                uses_24_hour_clock: Some(true),
            }),
            battery: None,
        }
    }

    #[test]
    fn all_tools_are_registered() {
        let names: Vec<_> = DeviceHub::tool_router()
            .list_all()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect();
        for expected in [
            "screenshot",
            "tap",
            "swipe",
            "multi_touch",
            "wait_for_frame",
            "type_text",
            "paste_text",
            "press_key",
            "press_button",
            "lock_device",
            "rotate",
            "list_apps",
            "list_companion_devices",
            "home_screen_layout",
            "list_processes",
            "wda_runner_status",
            "wda_start",
            "wda_stop",
            "wda_status",
            "wda_ui_tree",
            "wda_find_elements",
            "wda_click",
            "launch_app",
            "stop_app",
            "list_crash_reports",
            "read_crash_report",
            "device_details",
            "wait_for_device_event",
            "list_devices",
            "connect_device",
            "reconnect_device",
            "set_location",
            "clear_location",
            "list_device_conditions",
            "apply_device_condition",
            "clear_device_condition",
            "performance_snapshot",
            "recent_device_logs",
            "status",
        ] {
            assert!(
                names.iter().any(|name| name == expected),
                "missing {expected}"
            );
        }
    }

    #[test]
    fn image_dimensions_require_a_complete_positive_pair() {
        assert_eq!(image_size(None, None).unwrap(), None);
        assert_eq!(image_size(Some(320), Some(640)).unwrap(), Some((320, 640)));
        assert!(image_size(Some(320), None).is_err());
        assert!(image_size(Some(0), Some(640)).is_err());
    }

    #[test]
    fn game_action_parameters_are_bounded() {
        assert!(validate_touch_count(1).is_ok());
        assert!(validate_touch_count(5).is_ok());
        assert!(validate_touch_count(0).is_err());
        assert!(validate_touch_count(6).is_err());
        assert!(valid_bundle_identifier("com.example.game"));
        assert!(!valid_bundle_identifier("invalid bundle"));
    }

    #[tokio::test]
    async fn device_list_distinguishes_usb_and_wifi_transports() {
        let devices = DeviceListSlot::default();
        devices.set(vec![
            crate::protocol::DeviceInfo {
                id: "phone::usb".into(),
                udid: "phone".into(),
                name: "iPhone".into(),
                connection: crate::protocol::ConnKind::Usb,
                pairing: crate::protocol::DevicePairingState::Unpaired,
            },
            crate::protocol::DeviceInfo {
                id: "phone::wifi".into(),
                udid: "phone".into(),
                name: "iPhone".into(),
                connection: crate::protocol::ConnKind::Network,
                pairing: crate::protocol::DevicePairingState::NotApplicable,
            },
        ]);
        let active = ActiveSlot::default();
        active.set_selected("phone".into(), "phone::wifi".into());
        let (control, _control_rx) = tokio::sync::mpsc::unbounded_channel();
        let hub = DeviceHub::new(
            FrameSlot::default(),
            crate::browser_video::BrowserVideoSlot::default(),
            InputSink::default(),
            OrientationSlot::default(),
            devices,
            active,
            ErrorSlot::default(),
            StatusSlot::default(),
            LocationStatusSlot::default(),
            McpObservability::default(),
            control,
        );

        let result = hub.list_devices().await.unwrap();
        let text = result
            .content
            .first()
            .and_then(|content| content.as_text())
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&text.text).unwrap();
        assert_eq!(value["devices"][0]["id"], "phone::usb");
        assert_eq!(value["devices"][0]["pairing"], "unpaired");
        assert_eq!(value["devices"][0]["active"], false);
        assert_eq!(value["devices"][1]["id"], "phone::wifi");
        assert_eq!(value["devices"][1]["active"], true);
    }

    #[tokio::test]
    async fn device_details_redact_identifiers_unless_requested() {
        let input = InputSink::default();
        let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel();
        input.set(Some(input_tx));
        let (control, _control_rx) = tokio::sync::mpsc::unbounded_channel();
        let hub = DeviceHub::new(
            FrameSlot::default(),
            crate::browser_video::BrowserVideoSlot::default(),
            input,
            OrientationSlot::default(),
            DeviceListSlot::default(),
            ActiveSlot::default(),
            ErrorSlot::default(),
            StatusSlot::default(),
            LocationStatusSlot::default(),
            McpObservability::default(),
            control,
        );

        for include_identifiers in [false, true] {
            let details_hub = hub.clone();
            let task = tokio::spawn(async move {
                details_hub
                    .device_details(Parameters(DeviceDetailsParams {
                        include_identifiers: Some(include_identifiers),
                    }))
                    .await
                    .unwrap()
            });
            let InputCmd::GetDeviceDetails(reply) = input_rx.recv().await.unwrap() else {
                panic!("expected device details command");
            };
            reply.send(Ok(test_device_details())).unwrap();
            let result = task.await.unwrap();
            let text = result
                .content
                .iter()
                .find_map(|content| content.as_text().map(|text| text.text.as_str()))
                .unwrap();
            assert!(text.contains("Test iPhone"));
            assert!(text.contains("\"developer_image_mounted\":true"));
            assert!(text.contains("\"time_zone\":\"Asia/Taipei\""));
            assert_eq!(text.contains("private-serial"), include_identifiers);
            assert_eq!(text.contains("private-udid"), include_identifiers);
        }
    }

    #[tokio::test]
    async fn companion_devices_tool_returns_bounded_read_only_metadata() {
        let input = InputSink::default();
        let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel();
        input.set(Some(input_tx));
        let (control, _control_rx) = tokio::sync::mpsc::unbounded_channel();
        let hub = DeviceHub::new(
            FrameSlot::default(),
            crate::browser_video::BrowserVideoSlot::default(),
            input,
            OrientationSlot::default(),
            DeviceListSlot::default(),
            ActiveSlot::default(),
            ErrorSlot::default(),
            StatusSlot::default(),
            LocationStatusSlot::default(),
            McpObservability::default(),
            control,
        );
        let task = tokio::spawn(async move { hub.list_companion_devices().await.unwrap() });
        let InputCmd::ListCompanionDevices(reply) = input_rx.recv().await.unwrap() else {
            panic!("expected companion device command");
        };
        reply
            .send(Ok(vec![crate::companion_devices::CompanionDevice {
                identifier: "watch-id".into(),
                name: Some("Test Watch".into()),
                product_type: Some("Watch7,5".into()),
                product_version: Some("27.0".into()),
                build_version: None,
            }]))
            .unwrap();
        let result = task.await.unwrap();
        let text = result
            .content
            .iter()
            .find_map(|content| content.as_text().map(|text| text.text.as_str()))
            .unwrap();
        assert!(text.contains("Test Watch"));
        assert!(text.contains(r#""count":1"#));
    }

    #[tokio::test]
    async fn running_process_tool_returns_bounded_device_info() {
        let input = InputSink::default();
        let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel();
        input.set(Some(input_tx));
        let (control, _control_rx) = tokio::sync::mpsc::unbounded_channel();
        let hub = DeviceHub::new(
            FrameSlot::default(),
            crate::browser_video::BrowserVideoSlot::default(),
            input,
            OrientationSlot::default(),
            DeviceListSlot::default(),
            ActiveSlot::default(),
            ErrorSlot::default(),
            StatusSlot::default(),
            LocationStatusSlot::default(),
            McpObservability::default(),
            control,
        );
        let task = tokio::spawn(async move { hub.list_processes().await.unwrap() });
        let InputCmd::ListRunningProcesses(reply) = input_rx.recv().await.unwrap() else {
            panic!("expected running process command");
        };
        reply
            .send(Ok(crate::running_processes::RunningProcessList {
                processes: vec![crate::running_processes::RunningProcess {
                    pid: 42,
                    name: "Example".into(),
                    app_name: Some("Example App".into()),
                    is_application: true,
                }],
                truncated: false,
            }))
            .unwrap();
        let result = task.await.unwrap();
        let text = result
            .content
            .iter()
            .find_map(|content| content.as_text().map(|text| text.text.as_str()))
            .unwrap();
        assert!(text.contains("Example App"));
        assert!(text.contains(r#""pid":42"#));
        assert!(text.contains(r#""truncated":false"#));
    }

    #[tokio::test]
    async fn lock_device_tool_dispatches_one_way_power_command() {
        let input = InputSink::default();
        let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel();
        input.set(Some(input_tx));
        let (control, _control_rx) = tokio::sync::mpsc::unbounded_channel();
        let hub = DeviceHub::new(
            FrameSlot::default(),
            crate::browser_video::BrowserVideoSlot::default(),
            input,
            OrientationSlot::default(),
            DeviceListSlot::default(),
            ActiveSlot::default(),
            ErrorSlot::default(),
            StatusSlot::default(),
            LocationStatusSlot::default(),
            McpObservability::default(),
            control,
        );
        let task = tokio::spawn(async move { hub.lock_device().await.unwrap() });
        let InputCmd::LockDevice(reply) = input_rx.recv().await.unwrap() else {
            panic!("expected device lock command");
        };
        reply.send(Ok(())).unwrap();
        let result = task.await.unwrap();
        assert!(result.content.iter().any(|content| {
            content
                .as_text()
                .is_some_and(|text| text.text.contains(r#""lock_requested":true"#))
        }));
    }

    #[tokio::test]
    async fn home_screen_tool_returns_ordinal_locations() {
        let input = InputSink::default();
        let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel();
        input.set(Some(input_tx));
        let (control, _control_rx) = tokio::sync::mpsc::unbounded_channel();
        let hub = DeviceHub::new(
            FrameSlot::default(),
            crate::browser_video::BrowserVideoSlot::default(),
            input,
            OrientationSlot::default(),
            DeviceListSlot::default(),
            ActiveSlot::default(),
            ErrorSlot::default(),
            StatusSlot::default(),
            LocationStatusSlot::default(),
            McpObservability::default(),
            control,
        );
        let task = tokio::spawn(async move { hub.home_screen_layout().await.unwrap() });
        let InputCmd::GetHomeScreenLayout(reply) = input_rx.recv().await.unwrap() else {
            panic!("expected home screen command");
        };
        reply
            .send(Ok(crate::home_screen::HomeScreenLayout {
                apps: vec![crate::home_screen::HomeScreenAppLocation {
                    bundle_id: "com.example.game".into(),
                    name: Some("Game".into()),
                    container: crate::home_screen::HomeScreenContainer::Dock,
                    page: None,
                    position: 2,
                    folders: Vec::new(),
                }],
                page_count: 3,
                metrics: Some(crate::home_screen::HomeScreenIconMetrics {
                    screen_width: Some(810),
                    screen_height: Some(1080),
                    icon_width: Some(68),
                    icon_height: Some(68),
                    columns: Some(5),
                    rows: Some(6),
                    dock_max_count: Some(20),
                    folder_columns: Some(4),
                    folder_rows: Some(4),
                    max_pages: Some(15),
                    folder_max_pages: Some(15),
                }),
                truncated: false,
            }))
            .unwrap();
        let result = task.await.unwrap();
        let text = result
            .content
            .iter()
            .find_map(|content| content.as_text().map(|text| text.text.as_str()))
            .unwrap();
        assert!(text.contains("com.example.game"));
        assert!(text.contains(r#""container":"dock""#));
        assert!(text.contains(r#""position":2"#));
        assert!(text.contains(r#""columns":5"#));
        assert!(text.contains(r#""screen_width":810"#));
    }

    #[tokio::test]
    async fn wda_tools_dispatch_only_bounded_semantic_commands() {
        let input = InputSink::default();
        let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel();
        input.set(Some(input_tx));
        let (control, _control_rx) = tokio::sync::mpsc::unbounded_channel();
        let hub = DeviceHub::new(
            FrameSlot::default(),
            crate::browser_video::BrowserVideoSlot::default(),
            input,
            OrientationSlot::default(),
            DeviceListSlot::default(),
            ActiveSlot::default(),
            ErrorSlot::default(),
            StatusSlot::default(),
            LocationStatusSlot::default(),
            McpObservability::default(),
            control,
        );

        let status_hub = hub.clone();
        let status_task = tokio::spawn(async move { status_hub.wda_status().await.unwrap() });
        let InputCmd::WdaAutomation(crate::wda_automation::WdaAutomationCommand::Status {
            reply,
            expires_at,
        }) = input_rx.recv().await.unwrap()
        else {
            panic!("expected WDA status command");
        };
        assert!(expires_at > tokio::time::Instant::now());
        reply
            .send(Ok(crate::wda_automation::WdaStatus {
                reachable: true,
                ready: Some(true),
                message: None,
            }))
            .unwrap();
        let status = status_task.await.unwrap();
        assert!(status.content.iter().any(|content| {
            content
                .as_text()
                .is_some_and(|text| text.text.contains(r#""reachable":true"#))
        }));

        let source_hub = hub.clone();
        let source_task = tokio::spawn(async move {
            source_hub
                .wda_ui_tree(Parameters(WdaUiTreeParams {
                    max_characters: Some(4096),
                }))
                .await
                .unwrap()
        });
        let InputCmd::WdaAutomation(crate::wda_automation::WdaAutomationCommand::Source {
            max_characters,
            reply,
            ..
        }) = input_rx.recv().await.unwrap()
        else {
            panic!("expected WDA source command");
        };
        assert_eq!(max_characters, 4096);
        reply
            .send(Ok(crate::wda_automation::WdaUiTree {
                xml: "<App/>".into(),
                total_characters: 6,
                truncated: false,
            }))
            .unwrap();
        assert!(source_task.await.unwrap().content.iter().any(|content| {
            content
                .as_text()
                .is_some_and(|text| text.text.contains("<App/>"))
        }));

        let find_hub = hub.clone();
        let find_task = tokio::spawn(async move {
            find_hub
                .wda_find_elements(Parameters(WdaFindParams {
                    using: "accessibility id".into(),
                    value: "Play".into(),
                    limit: Some(2),
                }))
                .await
                .unwrap()
        });
        let InputCmd::WdaAutomation(crate::wda_automation::WdaAutomationCommand::Find {
            using,
            value,
            limit,
            reply,
            ..
        }) = input_rx.recv().await.unwrap()
        else {
            panic!("expected WDA find command");
        };
        assert_eq!(
            (using.as_str(), value.as_str(), limit),
            ("accessibility id", "Play", 2)
        );
        reply.send(Ok(Vec::new())).unwrap();
        assert!(find_task.await.unwrap().content.iter().any(|content| {
            content
                .as_text()
                .is_some_and(|text| text.text.contains(r#""returned":0"#))
        }));

        let click_hub = hub.clone();
        let click_task = tokio::spawn(async move {
            click_hub
                .wda_click(Parameters(WdaClickParams {
                    using: "name".into(),
                    value: "Continue".into(),
                    index: Some(1),
                }))
                .await
                .unwrap()
        });
        let InputCmd::WdaAutomation(crate::wda_automation::WdaAutomationCommand::Click {
            using,
            value,
            index,
            reply,
            ..
        }) = input_rx.recv().await.unwrap()
        else {
            panic!("expected WDA click command");
        };
        assert_eq!(
            (using.as_str(), value.as_str(), index),
            ("name", "Continue", 1)
        );
        reply
            .send(Ok(crate::wda_automation::WdaElement { index, rect: None }))
            .unwrap();
        assert!(click_task.await.unwrap().content.iter().any(|content| {
            content
                .as_text()
                .is_some_and(|text| text.text.contains(r#""clicked":true"#))
        }));

        assert!(
            hub.wda_find_elements(Parameters(WdaFindParams {
                using: "css selector".into(),
                value: "button".into(),
                limit: None,
            }))
            .await
            .is_err()
        );
        assert!(input_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn wda_runner_tools_dispatch_explicit_lifecycle_commands() {
        let input = InputSink::default();
        let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel();
        input.set(Some(input_tx));
        let (control, _control_rx) = tokio::sync::mpsc::unbounded_channel();
        let hub = DeviceHub::new(
            FrameSlot::default(),
            crate::browser_video::BrowserVideoSlot::default(),
            input,
            OrientationSlot::default(),
            DeviceListSlot::default(),
            ActiveSlot::default(),
            ErrorSlot::default(),
            StatusSlot::default(),
            LocationStatusSlot::default(),
            McpObservability::default(),
            control,
        );
        let running = crate::wda_runner::WdaRunnerStatus {
            phase: crate::wda_runner::WdaRunnerPhase::Running,
            managed: true,
            runner_bundle_id: Some("com.example.WDARunner.xctrunner".into()),
            last_error: None,
        };

        let status_hub = hub.clone();
        let status_task =
            tokio::spawn(async move { status_hub.wda_runner_status().await.unwrap() });
        let InputCmd::WdaRunner(crate::wda_runner::WdaRunnerCommand::Status { reply }) =
            input_rx.recv().await.unwrap()
        else {
            panic!("expected WDA runner status command");
        };
        reply.send(running.clone()).unwrap();
        assert!(status_task.await.unwrap().content.iter().any(|content| {
            content
                .as_text()
                .is_some_and(|text| text.text.contains(r#""managed":true"#))
        }));

        let start_hub = hub.clone();
        let start_task = tokio::spawn(async move {
            start_hub
                .wda_start(Parameters(WdaStartParams {
                    runner_bundle_id: "com.example.WDARunner.xctrunner".into(),
                }))
                .await
                .unwrap()
        });
        let InputCmd::WdaRunner(crate::wda_runner::WdaRunnerCommand::Start { bundle_id, reply }) =
            input_rx.recv().await.unwrap()
        else {
            panic!("expected WDA runner start command");
        };
        assert_eq!(bundle_id, "com.example.WDARunner.xctrunner");
        reply.send(Ok(running)).unwrap();
        start_task.await.unwrap();

        let stop_task = tokio::spawn(async move { hub.wda_stop().await.unwrap() });
        let InputCmd::WdaRunner(crate::wda_runner::WdaRunnerCommand::Stop { reply }) =
            input_rx.recv().await.unwrap()
        else {
            panic!("expected WDA runner stop command");
        };
        reply
            .send(Ok(crate::wda_runner::WdaRunnerStatus::default()))
            .unwrap();
        stop_task.await.unwrap();
    }

    #[tokio::test]
    async fn device_event_wait_uses_cursor_and_observes_future_events() {
        let observability = McpObservability::default();
        observability
            .device_events
            .publish(crate::device_events::DeviceEventKind::AppInstalled);
        let (control, _control_rx) = tokio::sync::mpsc::unbounded_channel();
        let hub = DeviceHub::new(
            FrameSlot::default(),
            crate::browser_video::BrowserVideoSlot::default(),
            InputSink::default(),
            OrientationSlot::default(),
            DeviceListSlot::default(),
            ActiveSlot::default(),
            ErrorSlot::default(),
            StatusSlot::default(),
            LocationStatusSlot::default(),
            observability.clone(),
            control,
        );

        let existing = hub
            .wait_for_device_event(Parameters(DeviceEventParams {
                after_sequence: Some(0),
                timeout_ms: Some(0),
            }))
            .await
            .unwrap();
        assert!(existing.content.iter().any(|content| {
            content
                .as_text()
                .is_some_and(|text| text.text.contains("app_installed"))
        }));

        let wait_hub = hub.clone();
        let waiter = tokio::spawn(async move {
            wait_hub
                .wait_for_device_event(Parameters(DeviceEventParams {
                    after_sequence: Some(1),
                    timeout_ms: Some(500),
                }))
                .await
                .unwrap()
        });
        tokio::task::yield_now().await;
        observability
            .device_events
            .publish(crate::device_events::DeviceEventKind::DeviceNameChanged);
        let future = waiter.await.unwrap();
        assert!(future.content.iter().any(|content| {
            content
                .as_text()
                .is_some_and(|text| text.text.contains("device_name_changed"))
        }));

        let timed_out = hub
            .wait_for_device_event(Parameters(DeviceEventParams {
                after_sequence: None,
                timeout_ms: Some(0),
            }))
            .await
            .unwrap();
        assert!(timed_out.content.iter().any(|content| {
            content
                .as_text()
                .is_some_and(|text| text.text.contains(r#""changed":false"#))
        }));
    }

    #[tokio::test]
    async fn device_condition_tools_use_the_enumerated_supervised_service() {
        let input = InputSink::default();
        let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel();
        input.set(Some(input_tx));
        let observability = McpObservability::default();
        observability
            .device_conditions
            .set(crate::device_conditions::DeviceConditionStatus {
                available: true,
                groups: vec![crate::device_conditions::DeviceConditionGroup {
                    identifier: "Network".into(),
                    profiles: vec![crate::device_conditions::DeviceConditionProfile {
                        identifier: "LTE".into(),
                        description: "LTE profile".into(),
                    }],
                }],
                active: None,
                cleanup_pending: false,
                error: None,
            });
        let (control, _control_rx) = tokio::sync::mpsc::unbounded_channel();
        let hub = DeviceHub::new(
            FrameSlot::default(),
            crate::browser_video::BrowserVideoSlot::default(),
            input,
            OrientationSlot::default(),
            DeviceListSlot::default(),
            ActiveSlot::default(),
            ErrorSlot::default(),
            StatusSlot::default(),
            LocationStatusSlot::default(),
            observability,
            control,
        );

        let listed = hub.list_device_conditions().await.unwrap();
        assert!(listed.content.iter().any(|content| {
            content
                .as_text()
                .is_some_and(|text| text.text.contains(r#""identifier":"LTE""#))
        }));
        assert!(
            hub.apply_device_condition(Parameters(DeviceConditionParams {
                group_identifier: "bad\nidentifier".into(),
                profile_identifier: "LTE".into(),
            }))
            .await
            .is_err()
        );
        assert!(input_rx.try_recv().is_err());

        let apply_hub = hub.clone();
        let apply = tokio::spawn(async move {
            apply_hub
                .apply_device_condition(Parameters(DeviceConditionParams {
                    group_identifier: "Network".into(),
                    profile_identifier: "LTE".into(),
                }))
                .await
                .unwrap()
        });
        let InputCmd::DeviceCondition(crate::device_conditions::DeviceConditionCommand::Apply {
            group_identifier,
            profile_identifier,
            expires_at,
            reply,
        }) = input_rx.recv().await.unwrap()
        else {
            panic!("expected device condition apply command");
        };
        assert_eq!(group_identifier, "Network");
        assert_eq!(profile_identifier, "LTE");
        assert!(expires_at > tokio::time::Instant::now());
        reply.send(Ok(())).unwrap();
        apply.await.unwrap();

        let clear = tokio::spawn(async move { hub.clear_device_condition().await.unwrap() });
        let InputCmd::DeviceCondition(crate::device_conditions::DeviceConditionCommand::Clear {
            expires_at,
            reply,
        }) = input_rx.recv().await.unwrap()
        else {
            panic!("expected device condition clear command");
        };
        assert!(expires_at > tokio::time::Instant::now());
        reply.send(Ok(())).unwrap();
        clear.await.unwrap();
    }

    #[tokio::test]
    async fn observability_tools_filter_logs_and_release_temporary_demand() {
        let observability = McpObservability::default();
        observability
            .device_logs
            .publish("network connection failed".into());
        observability
            .device_logs
            .publish("application ready".into());
        let (control, _control_rx) = tokio::sync::mpsc::unbounded_channel();
        let hub = DeviceHub::new(
            FrameSlot::default(),
            crate::browser_video::BrowserVideoSlot::default(),
            InputSink::default(),
            OrientationSlot::default(),
            DeviceListSlot::default(),
            ActiveSlot::default(),
            ErrorSlot::default(),
            StatusSlot::default(),
            LocationStatusSlot::default(),
            observability.clone(),
            control,
        );

        let logs = hub
            .recent_device_logs(Parameters(DeviceLogParams {
                after: None,
                limit: Some(10),
                wait_ms: Some(0),
                level: None,
                query: Some("NETWORK".into()),
            }))
            .await
            .unwrap();
        let logs = logs
            .content
            .iter()
            .find_map(|content| content.as_text().map(|text| text.text.as_str()))
            .unwrap();
        assert!(logs.contains("network connection failed"));
        assert!(!logs.contains("application ready"));
        assert!(!observability.device_log_demand.enabled());

        let performance = hub
            .performance_snapshot(Parameters(PerformanceSnapshotParams { wait_ms: Some(0) }))
            .await
            .unwrap();
        assert!(performance.content.iter().any(|content| {
            content
                .as_text()
                .is_some_and(|text| text.text.contains(r#""available":false"#))
        }));
        assert!(!observability.performance_demand.enabled());
    }

    #[tokio::test]
    async fn crash_report_tools_use_bounded_active_session_commands() {
        let input = InputSink::default();
        let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel();
        input.set(Some(input_tx));
        let (control, _control_rx) = tokio::sync::mpsc::unbounded_channel();
        let hub = DeviceHub::new(
            FrameSlot::default(),
            crate::browser_video::BrowserVideoSlot::default(),
            input,
            OrientationSlot::default(),
            DeviceListSlot::default(),
            ActiveSlot::default(),
            ErrorSlot::default(),
            StatusSlot::default(),
            LocationStatusSlot::default(),
            McpObservability::default(),
            control,
        );

        let list_hub = hub.clone();
        let list_task = tokio::spawn(async move {
            list_hub
                .list_crash_reports(Parameters(CrashReportListParams {
                    query: Some("game".into()),
                    limit: Some(10),
                }))
                .await
                .unwrap()
        });
        let InputCmd::ListCrashReports(reply) = input_rx.recv().await.unwrap() else {
            panic!("expected crash report list command");
        };
        reply
            .send(Ok(crate::protocol::DeviceCrashReportList {
                reports: vec![
                    crate::protocol::DeviceCrashReport {
                        path: "/Game-2026-07-24.ips".into(),
                        name: "Game-2026-07-24.ips".into(),
                        size_bytes: 4096,
                        modified: "2026-07-24T10:00:00Z".into(),
                    },
                    crate::protocol::DeviceCrashReport {
                        path: "/Other-2026-07-24.ips".into(),
                        name: "Other-2026-07-24.ips".into(),
                        size_bytes: 2048,
                        modified: "2026-07-24T09:00:00Z".into(),
                    },
                ],
                truncated: false,
            }))
            .unwrap();
        let list_result = list_task.await.unwrap();
        let list_text = list_result
            .content
            .iter()
            .find_map(|content| content.as_text().map(|text| text.text.as_str()))
            .unwrap();
        assert!(list_text.contains("Game-2026-07-24.ips"));
        assert!(!list_text.contains("Other-2026-07-24.ips"));

        let read_hub = hub.clone();
        let read_task = tokio::spawn(async move {
            read_hub
                .read_crash_report(Parameters(CrashReportReadParams {
                    device_path: "/Game-2026-07-24.ips".into(),
                    max_bytes: Some(4096),
                }))
                .await
                .unwrap()
        });
        let InputCmd::ReadCrashReport {
            device_path,
            max_bytes,
            reply,
        } = input_rx.recv().await.unwrap()
        else {
            panic!("expected crash report read command");
        };
        assert_eq!(device_path, "/Game-2026-07-24.ips");
        assert_eq!(max_bytes, 4096);
        reply
            .send(Ok(crate::protocol::DeviceCrashReportContent {
                device_path,
                size_bytes: 4096,
                bytes_read: 24,
                truncated: true,
                lossy_utf8: false,
                summary: crate::protocol::DeviceCrashReportSummary {
                    format: crate::protocol::CrashReportFormat::LegacyText,
                    kind: crate::protocol::CrashReportKind::AppCrash,
                    process_name: Some("Game".into()),
                    bundle_id: Some("com.example.game".into()),
                    app_version: None,
                    build_version: None,
                    os_version: None,
                    timestamp: None,
                    bug_type: None,
                    exception_type: Some("SIGABRT".into()),
                    exception_signal: None,
                    termination_namespace: None,
                    termination_code: None,
                    faulting_thread: None,
                    details_parsed: true,
                    source_truncated: true,
                },
                content: "Exception Type: SIGABRT".into(),
            }))
            .unwrap();
        let read_result = read_task.await.unwrap();
        assert!(read_result.content.iter().any(|content| {
            content.as_text().is_some_and(|text| {
                text.text.contains("Exception Type: SIGABRT")
                    && text.text.contains("com.example.game")
                    && text.text.contains("source_truncated")
            })
        }));

        assert!(
            hub.read_crash_report(Parameters(CrashReportReadParams {
                device_path: "/../private/file".into(),
                max_bytes: None,
            }))
            .await
            .is_err()
        );
        assert!(input_rx.try_recv().is_err());
    }

    #[test]
    fn device_log_level_filter_rejects_unknown_values() {
        assert_eq!(
            parse_log_level(Some("ERROR")).unwrap(),
            Some(DeviceLogLevel::Error)
        );
        assert!(parse_log_level(Some("warning")).is_err());
    }

    #[tokio::test]
    async fn multi_touch_sends_simultaneous_down_and_release_frames() {
        let frames = FrameSlot::default();
        frames.publish(Arc::new(Frame {
            width: 100,
            height: 200,
            format: FrameFormat::Rgb24,
            pixels: vec![0; 100 * 200 * 3],
            decoded_at: Instant::now(),
            jpeg: std::sync::OnceLock::new(),
        }));
        let input = InputSink::default();
        let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel();
        input.set(Some(input_tx));
        let (control, _control_rx) = tokio::sync::mpsc::unbounded_channel();
        let hub = DeviceHub::new(
            frames,
            crate::browser_video::BrowserVideoSlot::default(),
            input,
            OrientationSlot::default(),
            DeviceListSlot::default(),
            ActiveSlot::default(),
            ErrorSlot::default(),
            StatusSlot::default(),
            LocationStatusSlot::default(),
            McpObservability::default(),
            control,
        );
        hub.multi_touch(Parameters(MultiTouchParams {
            contacts: vec![
                TouchPathParams {
                    x1: 10.0,
                    y1: 20.0,
                    x2: 20.0,
                    y2: 30.0,
                },
                TouchPathParams {
                    x1: 80.0,
                    y1: 160.0,
                    x2: 80.0,
                    y2: 160.0,
                },
            ],
            duration_ms: Some(25),
            image_width: Some(100),
            image_height: Some(200),
            wait_for_settle: Some(false),
        }))
        .await
        .unwrap();

        let sent = std::iter::from_fn(|| input_rx.try_recv().ok()).collect::<Vec<_>>();
        let InputCmd::MultiTouchFrame(first) = &sent[0] else {
            panic!("first command must be a multi-touch frame");
        };
        let InputCmd::MultiTouchFrame(last) = sent.last().unwrap() else {
            panic!("last command must be a multi-touch frame");
        };
        assert_eq!(first.len(), 2);
        assert!(first.iter().all(|contact| contact.touching));
        assert!(last.iter().all(|contact| !contact.touching));
    }

    #[tokio::test]
    async fn wait_for_frame_observes_a_newer_published_version() {
        let frames = FrameSlot::default();
        let (control, _control_rx) = tokio::sync::mpsc::unbounded_channel();
        let hub = DeviceHub::new(
            frames.clone(),
            crate::browser_video::BrowserVideoSlot::default(),
            InputSink::default(),
            OrientationSlot::default(),
            DeviceListSlot::default(),
            ActiveSlot::default(),
            ErrorSlot::default(),
            StatusSlot::default(),
            LocationStatusSlot::default(),
            McpObservability::default(),
            control,
        );
        let waiter = tokio::spawn(async move {
            hub.wait_for_frame(Parameters(WaitFrameParams {
                after_version: Some(0),
                timeout_ms: Some(500),
            }))
            .await
            .unwrap()
        });
        tokio::task::yield_now().await;
        frames.publish(Arc::new(Frame {
            width: 1,
            height: 1,
            format: FrameFormat::Rgb24,
            pixels: vec![0, 0, 0],
            decoded_at: Instant::now(),
            jpeg: std::sync::OnceLock::new(),
        }));
        let result = waiter.await.unwrap();
        assert!(result.content.iter().any(|content| {
            content
                .as_text()
                .is_some_and(|text| text.text.contains(r#""changed":true"#))
        }));
    }

    #[tokio::test]
    async fn wait_for_frame_observes_browser_video_without_native_decode() {
        let browser_frames = crate::browser_video::BrowserVideoSlot::default();
        let (control, _control_rx) = tokio::sync::mpsc::unbounded_channel();
        let hub = DeviceHub::new(
            FrameSlot::default(),
            browser_frames.clone(),
            InputSink::default(),
            OrientationSlot::default(),
            DeviceListSlot::default(),
            ActiveSlot::default(),
            ErrorSlot::default(),
            StatusSlot::default(),
            LocationStatusSlot::default(),
            McpObservability::default(),
            control,
        );
        let waiter = tokio::spawn(async move {
            hub.wait_for_frame(Parameters(WaitFrameParams {
                after_version: Some(0),
                timeout_ms: Some(500),
            }))
            .await
            .unwrap()
        });
        tokio::task::yield_now().await;
        browser_frames.publish(0, true, 100, 200, vec![0, 0, 0, 1, 0x26]);
        let result = waiter.await.unwrap();
        assert!(result.content.iter().any(|content| {
            content
                .as_text()
                .is_some_and(|text| text.text.contains(r#""changed":true"#))
        }));
    }

    #[test]
    fn converts_rgb_and_yuv_pixels() {
        let rgb = Frame {
            width: 1,
            height: 1,
            format: FrameFormat::Rgb24,
            pixels: vec![1, 2, 3],
            decoded_at: Instant::now(),
            jpeg: std::sync::OnceLock::new(),
        };
        assert_eq!(rgb_at(&rgb, 0, 0), [1, 2, 3]);
        let yuv = Frame {
            width: 2,
            height: 2,
            format: FrameFormat::Yuv420p,
            pixels: vec![235, 235, 235, 235, 128, 128],
            decoded_at: Instant::now(),
            jpeg: std::sync::OnceLock::new(),
        };
        let pixel = rgb_at(&yuv, 1, 1);
        assert!(pixel.iter().all(|channel| *channel >= 250));
    }

    #[test]
    fn key_and_button_aliases_are_supported() {
        assert_eq!(key_usage("PageDown"), Some(0x4e));
        assert_eq!(button_label("volume_up"), Some("volume-up"));
        assert_eq!(button_label("power"), Some("lock"));
    }

    #[tokio::test]
    async fn streamable_http_negotiates_and_calls_status() {
        let (control, _control_rx) = tokio::sync::mpsc::unbounded_channel();
        let hub = DeviceHub::new(
            FrameSlot::default(),
            crate::browser_video::BrowserVideoSlot::default(),
            InputSink::default(),
            OrientationSlot::default(),
            DeviceListSlot::default(),
            ActiveSlot::default(),
            ErrorSlot::default(),
            StatusSlot::default(),
            LocationStatusSlot::default(),
            McpObservability::default(),
            control,
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, service_router(hub)).await;
        });

        let transport = StreamableHttpClientTransport::from_uri(format!("http://{address}/mcp"));
        let client = ClientInfo::default().serve(transport).await.unwrap();
        let tools = client.peer().list_tools(None).await.unwrap();
        assert!(tools.tools.iter().any(|tool| tool.name == "screenshot"));
        assert!(
            tools
                .tools
                .iter()
                .any(|tool| tool.name == "performance_snapshot")
        );
        assert!(
            tools
                .tools
                .iter()
                .any(|tool| tool.name == "recent_device_logs")
        );
        assert!(
            tools
                .tools
                .iter()
                .any(|tool| tool.name == "list_crash_reports")
        );
        assert!(
            tools
                .tools
                .iter()
                .any(|tool| tool.name == "read_crash_report")
        );
        assert!(tools.tools.iter().any(|tool| tool.name == "device_details"));
        assert!(
            tools
                .tools
                .iter()
                .any(|tool| tool.name == "wait_for_device_event")
        );
        let performance = client
            .call_tool(
                CallToolRequestParams::new("performance_snapshot")
                    .with_arguments(serde_json::Map::from_iter([("wait_ms".into(), json!(0))])),
            )
            .await
            .unwrap();
        assert_ne!(performance.is_error, Some(true));
        let status = client
            .call_tool(CallToolRequestParams::new("status"))
            .await
            .unwrap();
        assert_ne!(status.is_error, Some(true));
        assert!(status.content.iter().any(|content| {
            content
                .as_text()
                .is_some_and(|text| text.text.contains("active_udid"))
        }));
        let _ = client.cancel().await;
        server.abort();
    }
}
