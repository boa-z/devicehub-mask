//! Validated client commands for the private control WebSocket.
//!
//! JSON parsing and HID-facing validation live here instead of in the Axum
//! router. The adapter supplies only the current orientation, input sink, and
//! browser video slot; this module cannot reach unrelated HTTP or device APIs.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Deserialize;

use crate::browser_video::BrowserVideoSlot;
use crate::domain::hardware_button;
use crate::protocol::{
    HARDWARE_BUTTON_NAMES, InputCmd, InputSink, Orientation, RotateDir, norm, unrotate_norm,
};
use devicehub_runtime::{DeviceInputCommand, TouchContact};

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    MultiTouch {
        contacts: Vec<WebContact>,
    },
    Button {
        name: String,
    },
    ButtonDown {
        name: String,
    },
    ButtonUp {
        name: String,
    },
    KeyboardDown {
        usage: u64,
    },
    KeyboardUp {
        usage: u64,
    },
    Text {
        text: String,
    },
    Rotate {
        direction: RotateRequest,
    },
    VideoDemand {
        active: bool,
    },
    BrowserFrameAccepted {
        sequence: String,
    },
    FramePresented {
        sequence: String,
    },
    BrowserVideoKeyframe,
    BrowserDecoderError {
        message: String,
    },
    FrontendMetrics {
        window_ms: f64,
        received_frames: u64,
        replaced_frames: u64,
        presented_frames: u64,
        decoder_output_ms: f64,
        canvas_draw_ms: f64,
        decoder_congestions: u64,
        decode_errors: u64,
    },
}

#[derive(Deserialize)]
pub(crate) struct WebContact {
    pub(crate) identity: u8,
    pub(crate) touching: bool,
    pub(crate) x: f32,
    pub(crate) y: f32,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RotateRequest {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClientVideoFeedback {
    None,
    BrowserAccepted(u64),
    FramePresented(u64),
    ResetBrowser,
    ResetAll,
}

/// Separates decoder ingress acknowledgements from presentation telemetry.
/// A browser credit is released only by the matching sequence, so a late
/// acknowledgement cannot accidentally admit a newer frame.
pub(crate) fn handle_client_message(
    input: &InputSink,
    orientation: Orientation,
    browser_frames: &BrowserVideoSlot,
    text: &str,
    pressed_keyboard: &mut HashSet<u64>,
    video_active: &AtomicBool,
    browser_resync: &AtomicBool,
) -> ClientVideoFeedback {
    let Ok(message) = serde_json::from_str::<ClientMessage>(text) else {
        return ClientVideoFeedback::None;
    };
    match message {
        ClientMessage::BrowserFrameAccepted { sequence } => {
            return sequence
                .parse::<u64>()
                .map(ClientVideoFeedback::BrowserAccepted)
                .unwrap_or(ClientVideoFeedback::None);
        }
        ClientMessage::FramePresented { sequence } => {
            return sequence
                .parse::<u64>()
                .map(ClientVideoFeedback::FramePresented)
                .unwrap_or(ClientVideoFeedback::None);
        }
        ClientMessage::VideoDemand { active } => {
            let was_active = video_active.load(Ordering::Relaxed);
            if active != was_active {
                if active {
                    browser_resync.store(true, Ordering::Release);
                    video_active.store(true, Ordering::Release);
                    browser_frames.request_keyframe();
                } else {
                    video_active.store(false, Ordering::Release);
                    return ClientVideoFeedback::ResetAll;
                }
            }
            tracing::debug!(active, "updated WebView video demand");
        }
        ClientMessage::BrowserVideoKeyframe => {
            browser_resync.store(true, Ordering::Release);
            browser_frames.request_keyframe();
            return ClientVideoFeedback::ResetBrowser;
        }
        ClientMessage::BrowserDecoderError { message } => {
            let message = message.chars().take(256).collect::<String>();
            tracing::error!(%message, "WebCodecs video decoder stopped; no native video fallback is configured");
        }
        ClientMessage::FrontendMetrics {
            window_ms,
            received_frames,
            replaced_frames,
            presented_frames,
            decoder_output_ms,
            canvas_draw_ms,
            decoder_congestions,
            decode_errors,
        } => {
            if valid_frontend_metrics(
                window_ms,
                received_frames,
                replaced_frames,
                presented_frames,
                decoder_output_ms,
                canvas_draw_ms,
                decoder_congestions,
                decode_errors,
            ) {
                let elapsed = (window_ms / 1000.0).max(f64::EPSILON);
                tracing::debug!(
                    target: "devicehub_mask::perf",
                    received_fps = received_frames as f64 / elapsed,
                    presented_fps = presented_frames as f64 / elapsed,
                    received_frames,
                    replaced_frames,
                    presented_frames,
                    decoder_output_ms = decoder_output_ms / received_frames.max(1) as f64,
                    canvas_draw_ms = canvas_draw_ms / presented_frames.max(1) as f64,
                    decoder_congestions,
                    decode_errors,
                    "frontend video performance"
                );
            }
        }
        ClientMessage::MultiTouch { contacts } => {
            if let Some(contacts) = validate_contacts(contacts, orientation) {
                input.send(InputCmd::DeviceInput(DeviceInputCommand::MultiTouchFrame(
                    contacts,
                )));
            }
        }
        ClientMessage::Button { name } => {
            if let Some(button) = hardware_button(&name) {
                input.send(InputCmd::DeviceInput(DeviceInputCommand::Button(button)));
            }
        }
        ClientMessage::ButtonDown { name } => {
            if let Some(button) = hardware_button(&name) {
                input.send(InputCmd::DeviceInput(DeviceInputCommand::ButtonDown(
                    button,
                )));
            }
        }
        ClientMessage::ButtonUp { name } => {
            if let Some(button) = hardware_button(&name) {
                input.send(InputCmd::DeviceInput(DeviceInputCommand::ButtonUp(button)));
            }
        }
        ClientMessage::KeyboardDown { usage } => {
            if valid_keyboard_usage(usage) && pressed_keyboard.insert(usage) {
                input.send(InputCmd::DeviceInput(DeviceInputCommand::KeyboardDown(
                    usage,
                )));
            }
        }
        ClientMessage::KeyboardUp { usage } => {
            if valid_keyboard_usage(usage) && pressed_keyboard.remove(&usage) {
                input.send(InputCmd::DeviceInput(DeviceInputCommand::KeyboardUp(usage)));
            }
        }
        ClientMessage::Text { text } => {
            if !text.is_empty() && text.len() <= 512 && text.chars().count() <= 128 {
                input.send(InputCmd::DeviceInput(DeviceInputCommand::Text(text)));
            }
        }
        ClientMessage::Rotate { direction } => {
            input.send(InputCmd::DeviceInput(DeviceInputCommand::Rotate(
                match direction {
                    RotateRequest::Left => RotateDir::Left,
                    RotateRequest::Right => RotateDir::Right,
                },
            )));
        }
    }
    ClientVideoFeedback::None
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn valid_frontend_metrics(
    window_ms: f64,
    received_frames: u64,
    replaced_frames: u64,
    presented_frames: u64,
    decoder_output_ms: f64,
    canvas_draw_ms: f64,
    decoder_congestions: u64,
    decode_errors: u64,
) -> bool {
    (500.0..=60_000.0).contains(&window_ms)
        && decoder_output_ms.is_finite()
        && canvas_draw_ms.is_finite()
        && (0.0..=window_ms * 10.0).contains(&decoder_output_ms)
        && (0.0..=window_ms * 10.0).contains(&canvas_draw_ms)
        && received_frames <= 10_000
        && replaced_frames <= received_frames
        && presented_frames <= received_frames
        && decoder_congestions <= 10_000
        && decode_errors <= received_frames
}

pub(crate) fn validate_contacts(
    contacts: Vec<WebContact>,
    orientation: Orientation,
) -> Option<Vec<TouchContact>> {
    if contacts.len() > 5 {
        return None;
    }
    let mut identities = HashSet::new();
    let turns = orientation.quarter_turns_cw();
    contacts
        .into_iter()
        .map(|contact| {
            if contact.identity >= 5
                || !identities.insert(contact.identity)
                || !contact.x.is_finite()
                || !contact.y.is_finite()
                || !(0.0..=1.0).contains(&contact.x)
                || !(0.0..=1.0).contains(&contact.y)
            {
                return None;
            }
            let (x, y) = unrotate_norm(contact.x, contact.y, turns);
            Some(TouchContact {
                identity: contact.identity,
                touching: contact.touching,
                x: norm(x),
                y: norm(y),
            })
        })
        .collect()
}

pub(crate) fn send_all_up(input: &InputSink, pressed_keyboard: &HashSet<u64>) {
    input.send(InputCmd::DeviceInput(DeviceInputCommand::MultiTouchFrame(
        (0..5)
            .map(|identity| TouchContact {
                identity,
                touching: false,
                x: 0,
                y: 0,
            })
            .collect(),
    )));
    for name in HARDWARE_BUTTON_NAMES {
        input.send(InputCmd::DeviceInput(DeviceInputCommand::ButtonUp(
            hardware_button(name).expect("known hardware button must resolve"),
        )));
    }
    for usage in pressed_keyboard {
        input.send(InputCmd::DeviceInput(DeviceInputCommand::KeyboardUp(
            *usage,
        )));
    }
}

pub(crate) fn valid_keyboard_usage(usage: u64) -> bool {
    matches!(usage, 0x04..=0x73 | 0x85 | 0x87 | 0x89 | 0xe0..=0xe7)
}
