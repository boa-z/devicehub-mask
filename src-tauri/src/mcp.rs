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

use crate::protocol::{
    ActiveSlot, ControlCmd, DeviceListSlot, ErrorSlot, Frame, FrameFormat, FrameSlot, InputCmd,
    InputSink, LocationStatusSlot, Orientation, OrientationSlot, RotateDir, StatusSlot, norm,
    unrotate_norm,
};

const DEFAULT_ADDR: &str = "127.0.0.1:8009";
const DEFAULT_MAX_DIM: u32 = 1024;
const MAX_SCREENSHOT_DIM: u32 = 4096;
const TAP_HOLD_SAMPLES: u32 = 3;
const TAP_SAMPLE_MS: u64 = 25;
const SETTLE_MIN: Duration = Duration::from_millis(200);
const SETTLE_MAX: Duration = Duration::from_millis(2600);
const SETTLE_POLL: Duration = Duration::from_millis(110);
const SETTLE_DIFF: f32 = 2.5;
const SETTLE_STABLE_SAMPLES: u32 = 3;
const GRID_STEP: u32 = 100;
const GRID_LABEL_EVERY: u32 = 2;
const DEVICE_WAIT: Duration = Duration::from_secs(20);
const LOCATION_WAIT: Duration = Duration::from_secs(10);

#[derive(Clone)]
struct DeviceHub {
    frames: FrameSlot,
    input: InputSink,
    orientation: OrientationSlot,
    devices: DeviceListSlot,
    active: ActiveSlot,
    error: ErrorSlot,
    status: StatusSlot,
    location: LocationStatusSlot,
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
struct LocationParams {
    latitude: f64,
    longitude: f64,
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

#[tool_router]
impl DeviceHub {
    #[allow(clippy::too_many_arguments)]
    fn new(
        frames: FrameSlot,
        input: InputSink,
        orientation: OrientationSlot,
        devices: DeviceListSlot,
        active: ActiveSlot,
        error: ErrorSlot,
        status: StatusSlot,
        location: LocationStatusSlot,
        control: UnboundedSender<ControlCmd>,
    ) -> Self {
        Self {
            frames,
            input,
            orientation,
            devices,
            active,
            error,
            status,
            location,
            control,
            last_image: Arc::new(Mutex::new(None)),
            gesture_lock: Arc::new(tokio::sync::Mutex::new(())),
            tool_router: Self::tool_router(),
        }
    }

    fn to_device(&self, x: f32, y: f32, size: Option<(u32, u32)>) -> Option<(u16, u16)> {
        let (_, frame) = self.frames.latest()?;
        let turns = self.orientation.get().quarter_turns_cw();
        let (width, height) = size
            .or_else(|| *self.last_image.lock().unwrap())
            .unwrap_or_else(|| display_dims(&frame, turns));
        let dx = ((x + 0.5) / width as f32).clamp(0.0, 1.0);
        let dy = ((y + 0.5) / height as f32).clamp(0.0, 1.0);
        let (nx, ny) = unrotate_norm(dx, dy, turns);
        Some((norm(nx), norm(ny)))
    }

    fn send(&self, command: InputCmd) -> Result<(), McpError> {
        self.input
            .try_send(command)
            .then_some(())
            .ok_or_else(|| McpError::internal_error("no active device session", None))
    }

    async fn settle(&self) {
        tokio::time::sleep(SETTLE_MIN).await;
        let started = Instant::now();
        let mut previous = self
            .frames
            .latest()
            .map(|(_, frame)| frame_signature(&frame));
        let mut stable = 0;
        while started.elapsed() < SETTLE_MAX {
            tokio::time::sleep(SETTLE_POLL).await;
            let current = self
                .frames
                .latest()
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
        description = "Capture the current iPhone screen as a PNG. The returned image is the coordinate space for tap/swipe. A labeled 100px grid is enabled by default; max_dim defaults to 1024 and 0 keeps native resolution."
    )]
    async fn screenshot(
        &self,
        Parameters(params): Parameters<ScreenshotParams>,
    ) -> Result<CallToolResult, McpError> {
        let Some((_, frame)) = self.frames.latest() else {
            return ok_text("No frame available. Connect a device and wait for streaming.");
        };
        let (width, height, rgb) =
            render_upright(&frame, self.orientation.get().quarter_turns_cw());
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
            Content::text(format!(
                "Image is {width}x{height} pixels; origin is top-left. Pass these dimensions with tap/swipe when coordinating across MCP clients."
            )),
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
        let _gesture = self.gesture_lock.lock().await;
        self.send(InputCmd::TouchDown { x, y })?;
        for _ in 0..TAP_HOLD_SAMPLES {
            tokio::time::sleep(Duration::from_millis(TAP_SAMPLE_MS)).await;
            self.send(InputCmd::TouchMove { x, y })?;
        }
        tokio::time::sleep(Duration::from_millis(TAP_SAMPLE_MS)).await;
        self.send(InputCmd::TouchUp { x, y })?;
        self.settle().await;
        ok_text(format!("Tapped ({}, {}).", params.x, params.y))
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
        let duration = params.duration_ms.unwrap_or(300).clamp(50, 5000);
        let steps = (duration / 16).clamp(2, 150);
        let _gesture = self.gesture_lock.lock().await;
        self.send(InputCmd::TouchDown {
            x: start_x,
            y: start_y,
        })?;
        for step in 1..=steps {
            let progress = step as f32 / steps as f32;
            let x = params.x1 + (params.x2 - params.x1) * progress;
            let y = params.y1 + (params.y2 - params.y1) * progress;
            if let Some((x, y)) = self.to_device(x, y, size) {
                self.send(InputCmd::TouchMove { x, y })?;
            }
            tokio::time::sleep(Duration::from_millis((duration / steps).max(1))).await;
        }
        if let Some((x, y)) = self.to_device(params.x2, params.y2, size) {
            self.send(InputCmd::TouchUp { x, y })?;
        }
        self.settle().await;
        ok_text(format!(
            "Swiped ({}, {}) to ({}, {}) over {duration}ms.",
            params.x1, params.y1, params.x2, params.y2
        ))
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
        description = "List attached iOS devices with UDID, name, connection type and active state."
    )]
    async fn list_devices(&self) -> Result<CallToolResult, McpError> {
        let active = self.active.get();
        let devices: Vec<_> = self
            .devices
            .get()
            .into_iter()
            .map(|device| {
                json!({
                    "udid": device.udid,
                    "name": device.name,
                    "connection": device.connection.label(),
                    "active": active.as_deref() == Some(device.udid.as_str()),
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
        if !reconnect
            && self.active.get().as_deref() == Some(udid.as_str())
            && self.frames.latest().is_some()
        {
            return ok_text(format!("Already connected to {udid}; screen is streaming."));
        }
        let previous_version = self.frames.version();
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
                && self.frames.version() > previous_version
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

    #[tool(description = "Connect to an attached device by UDID and wait for a new screen frame.")]
    async fn connect_device(
        &self,
        Parameters(params): Parameters<DeviceParams>,
    ) -> Result<CallToolResult, McpError> {
        self.switch_device(params.udid, false).await
    }

    #[tool(
        description = "Tear down and reconnect the selected device by UDID, then wait for a new screen frame."
    )]
    async fn reconnect_device(
        &self,
        Parameters(params): Parameters<DeviceParams>,
    ) -> Result<CallToolResult, McpError> {
        self.switch_device(params.udid, true).await
    }

    #[tool(
        description = "Set a fixed simulated GPS location through the active iOS 17+ DVT session."
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
        description = "Report active device, stream state, screen size, orientation and virtual-location state."
    )]
    async fn status(&self) -> Result<CallToolResult, McpError> {
        let screen_size = self
            .frames
            .latest()
            .map(|(_, frame)| {
                let (width, height) =
                    display_dims(&frame, self.orientation.get().quarter_turns_cw());
                json!([width, height])
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
                "streaming": self.frames.latest().is_some(),
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
            .with_instructions("Control the connected iPhone by calling screenshot, then tap/swipe/type_text/press_key/press_button, and screenshot again. Coordinates are pixels in the screenshot; include its image_width and image_height in actions. Use list_devices and connect_device when no device is active. Actions wait for the screen to settle before returning.")
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn serve(
    frames: FrameSlot,
    input: InputSink,
    orientation: OrientationSlot,
    devices: DeviceListSlot,
    active: ActiveSlot,
    error: ErrorSlot,
    status: StatusSlot,
    location: LocationStatusSlot,
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
    let hub = DeviceHub::new(
        frames,
        input,
        orientation,
        devices,
        active,
        error,
        status,
        location,
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
            "type_text",
            "press_key",
            "press_button",
            "rotate",
            "list_devices",
            "connect_device",
            "reconnect_device",
            "set_location",
            "clear_location",
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
            InputSink::default(),
            OrientationSlot::default(),
            DeviceListSlot::default(),
            ActiveSlot::default(),
            ErrorSlot::default(),
            StatusSlot::default(),
            LocationStatusSlot::default(),
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
