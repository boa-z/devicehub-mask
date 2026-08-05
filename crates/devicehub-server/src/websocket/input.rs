//! Validated client commands for the private control WebSocket.
//!
//! JSON parsing and HID-facing validation live here instead of in the Axum
//! router. The adapter supplies only the current orientation, input sink, and
//! browser video slot; this module cannot reach unrelated HTTP or device APIs.

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Deserialize;
use serde_json::Value;

use devicehub_core::hardware_button;
use devicehub_core::{
    DeviceInputCommand, HARDWARE_BUTTON_NAMES, KeyMappingProfile, Orientation, RotateDir,
    TouchContact, norm, system_action, unrotate_norm,
};
use devicehub_runtime::{
    BrowserVideoSlot, DeviceSessionCommand as InputCmd, SessionCommandSlot as InputSink,
};

use super::keymap::{
    BrowserDirectContact, BrowserKeymapPointerDelta, BrowserKeymapResolution, BrowserKeymapSession,
};

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    ClientHello {
        protocol_version: u16,
        platform: String,
        client_version: String,
        #[serde(default)]
        capabilities: Vec<String>,
    },
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
    SystemAction {
        action: String,
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
    KeymapConfigure {
        profile: KeyMappingProfile,
        frame: BrowserKeymapResolution,
        #[serde(default)]
        allow_scripts: bool,
    },
    KeymapInput {
        keys: Vec<String>,
        #[serde(default)]
        pointer_deltas: Vec<BrowserKeymapPointerDelta>,
        #[serde(default)]
        gamepad_axes: BTreeMap<String, f32>,
    },
    KeymapDirectTouches {
        contacts: Vec<BrowserDirectContact>,
    },
    KeymapDebug {
        enabled: bool,
    },
    KeymapStop,
    Rotate {
        direction: RotateRequest,
    },
    VideoDemand {
        active: bool,
    },
    AudioDemand {
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
pub(super) struct WebContact {
    pub(super) identity: u8,
    pub(super) touching: bool,
    pub(super) x: f32,
    pub(super) y: f32,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RotateRequest {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ClientVideoFeedback {
    None,
    ProtocolError(String),
    BrowserAccepted(u64),
    FramePresented(u64),
    ResetBrowser,
    ResetAll,
    KeymapEvent(Value),
}

pub(super) struct ClientConnectionState {
    control_granted: AtomicBool,
    video: AtomicBool,
    audio: AtomicBool,
}

impl ClientConnectionState {
    pub(super) fn new(control_granted: bool) -> Self {
        Self {
            control_granted: AtomicBool::new(control_granted),
            video: AtomicBool::new(false),
            audio: AtomicBool::new(false),
        }
    }

    pub(super) fn video_active(&self) -> bool {
        self.video.load(Ordering::Acquire)
    }

    pub(super) fn control_granted(&self) -> bool {
        self.control_granted.load(Ordering::Acquire)
    }

    pub(super) fn grant_control(&self) {
        self.control_granted.store(true, Ordering::Release);
    }

    pub(super) fn audio_active(&self) -> bool {
        self.audio.load(Ordering::Acquire)
    }
}

/// Per-connection adapters used while validating one client message.
pub(super) struct ClientMessageContext<'a, HostPath> {
    pub(super) input: &'a InputSink<HostPath>,
    pub(super) orientation: Orientation,
    pub(super) browser_frames: &'a BrowserVideoSlot,
    pub(super) connection: &'a ClientConnectionState,
    pub(super) browser_resync: &'a AtomicBool,
}

/// Separates decoder ingress acknowledgements from presentation telemetry.
/// A browser credit is released only by the matching sequence, so a late
/// acknowledgement cannot accidentally admit a newer frame.
#[cfg(test)]
pub(super) fn handle_client_message<HostPath>(
    input: &InputSink<HostPath>,
    orientation: Orientation,
    browser_frames: &BrowserVideoSlot,
    text: &str,
    pressed_keyboard: &mut HashSet<u64>,
    connection: &ClientConnectionState,
    browser_resync: &AtomicBool,
) -> ClientVideoFeedback {
    handle_client_message_with_keymap(
        ClientMessageContext {
            input,
            orientation,
            browser_frames,
            connection,
            browser_resync,
        },
        text,
        pressed_keyboard,
        &mut BrowserKeymapSession::default(),
    )
}

pub(super) fn handle_client_message_with_keymap<HostPath>(
    context: ClientMessageContext<'_, HostPath>,
    text: &str,
    pressed_keyboard: &mut HashSet<u64>,
    keymap: &mut BrowserKeymapSession,
) -> ClientVideoFeedback {
    let ClientMessageContext {
        input,
        orientation,
        browser_frames,
        connection,
        browser_resync,
    } = context;
    let Ok(message) = serde_json::from_str::<ClientMessage>(text) else {
        return ClientVideoFeedback::None;
    };
    if !connection.control_granted()
        && matches!(
            message,
            ClientMessage::MultiTouch { .. }
                | ClientMessage::Button { .. }
                | ClientMessage::ButtonDown { .. }
                | ClientMessage::ButtonUp { .. }
                | ClientMessage::SystemAction { .. }
                | ClientMessage::KeyboardDown { .. }
                | ClientMessage::KeyboardUp { .. }
                | ClientMessage::Text { .. }
                | ClientMessage::KeymapConfigure { .. }
                | ClientMessage::KeymapInput { .. }
                | ClientMessage::KeymapDirectTouches { .. }
                | ClientMessage::KeymapStop
                | ClientMessage::Rotate { .. }
        )
    {
        return ClientVideoFeedback::None;
    }
    match message {
        ClientMessage::ClientHello {
            protocol_version,
            platform,
            client_version,
            capabilities,
        } => {
            let platform = platform.chars().take(32).collect::<String>();
            let client_version = client_version.chars().take(64).collect::<String>();
            let capabilities = capabilities
                .into_iter()
                .take(32)
                .map(|capability| capability.chars().take(64).collect::<String>())
                .collect::<Vec<_>>();
            tracing::info!(
                protocol_version,
                %platform,
                %client_version,
                ?capabilities,
                "mobile client handshake received"
            );
            if protocol_version != 1 {
                return ClientVideoFeedback::ProtocolError(format!(
                    "unsupported client protocol version {protocol_version}"
                ));
            }
            if !matches!(platform.as_str(), "ios" | "android" | "web" | "desktop") {
                return ClientVideoFeedback::ProtocolError("unsupported client platform".into());
            }
            if client_version.is_empty() {
                return ClientVideoFeedback::ProtocolError(
                    "client version must not be empty".into(),
                );
            }
        }
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
            let was_active = connection.video.load(Ordering::Relaxed);
            if active != was_active {
                if active {
                    browser_resync.store(true, Ordering::Release);
                    connection.video.store(true, Ordering::Release);
                    browser_frames.request_keyframe();
                } else {
                    connection.video.store(false, Ordering::Release);
                    return ClientVideoFeedback::ResetAll;
                }
            }
            tracing::debug!(active, "updated WebView video demand");
        }
        ClientMessage::AudioDemand { active } => {
            connection.audio.store(active, Ordering::Release);
            tracing::debug!(active, "updated WebView audio demand");
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
        ClientMessage::SystemAction { action } => {
            if let Some(action) = system_action(&action.to_ascii_lowercase()) {
                input.send(InputCmd::DeviceInput(DeviceInputCommand::System(action)));
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
        ClientMessage::KeymapConfigure {
            profile,
            frame,
            allow_scripts,
        } => {
            return ClientVideoFeedback::KeymapEvent(keymap.configure(
                input,
                orientation,
                profile,
                frame,
                allow_scripts,
            ));
        }
        ClientMessage::KeymapInput {
            keys,
            pointer_deltas,
            gamepad_axes,
        } => {
            return ClientVideoFeedback::KeymapEvent(keymap.set_input(
                input,
                orientation,
                keys,
                pointer_deltas,
                gamepad_axes,
            ));
        }
        ClientMessage::KeymapDirectTouches { contacts } => {
            return ClientVideoFeedback::KeymapEvent(keymap.set_direct_contacts(
                input,
                orientation,
                contacts,
            ));
        }
        ClientMessage::KeymapDebug { enabled } => {
            return ClientVideoFeedback::KeymapEvent(keymap.set_debug_enabled(enabled));
        }
        ClientMessage::KeymapStop => {
            return ClientVideoFeedback::KeymapEvent(keymap.stop(input, orientation));
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
fn valid_frontend_metrics(
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

fn validate_contacts(
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

pub(super) fn send_all_up<HostPath>(input: &InputSink<HostPath>, pressed_keyboard: &HashSet<u64>) {
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

fn valid_keyboard_usage(usage: u64) -> bool {
    matches!(usage, 0x04..=0x73 | 0x85 | 0x87 | 0x89 | 0xe0..=0xe7)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    use serde_json::json;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    use super::*;
    use devicehub_core::{DeviceInputCommand, Orientation, SystemAction, norm};

    fn test_state() -> (
        InputSink<PathBuf>,
        BrowserVideoSlot,
        UnboundedReceiver<InputCmd<PathBuf>>,
    ) {
        let input = InputSink::default();
        let (input_tx, input_rx) = unbounded_channel();
        input.set(Some(input_tx));
        (input, BrowserVideoSlot::default(), input_rx)
    }

    fn handle_test_client_message(
        input: &InputSink<PathBuf>,
        browser_frames: &BrowserVideoSlot,
        text: &str,
        pressed_keyboard: &mut HashSet<u64>,
    ) -> ClientVideoFeedback {
        handle_client_message(
            input,
            Orientation::Portrait,
            browser_frames,
            text,
            pressed_keyboard,
            &ClientConnectionState::new(true),
            &AtomicBool::new(false),
        )
    }

    #[test]
    fn browser_feedback_messages_keep_acceptance_and_presentation_distinct() {
        let (input, browser_frames, _input_rx) = test_state();
        let mut pressed = HashSet::new();
        assert_eq!(
            handle_test_client_message(
                &input,
                &browser_frames,
                r#"{"type":"browser_frame_accepted","sequence":"42"}"#,
                &mut pressed,
            ),
            ClientVideoFeedback::BrowserAccepted(42)
        );
        assert_eq!(
            handle_test_client_message(
                &input,
                &browser_frames,
                r#"{"type":"frame_presented","sequence":"42"}"#,
                &mut pressed,
            ),
            ClientVideoFeedback::FramePresented(42)
        );
    }

    #[test]
    fn client_hello_rejects_unknown_protocol_or_platform() {
        let (input, browser_frames, _input_rx) = test_state();
        let mut pressed = HashSet::new();
        let invalid_version = handle_test_client_message(
            &input,
            &browser_frames,
            r#"{"type":"client_hello","protocol_version":99,"platform":"android","client_version":"test"}"#,
            &mut pressed,
        );
        assert!(matches!(
            invalid_version,
            ClientVideoFeedback::ProtocolError(_)
        ));

        let invalid_platform = handle_test_client_message(
            &input,
            &browser_frames,
            r#"{"type":"client_hello","protocol_version":1,"platform":"android-target","client_version":"test"}"#,
            &mut pressed,
        );
        assert!(matches!(
            invalid_platform,
            ClientVideoFeedback::ProtocolError(_)
        ));
    }

    #[test]
    fn view_only_clients_keep_media_messages_but_cannot_dispatch_input() {
        let (input, browser_frames, mut input_rx) = test_state();
        let connection = ClientConnectionState::new(false);
        let resync = AtomicBool::new(false);
        let mut pressed = HashSet::new();

        handle_client_message(
            &input,
            Orientation::Portrait,
            &browser_frames,
            r#"{"type":"button","name":"home"}"#,
            &mut pressed,
            &connection,
            &resync,
        );
        assert!(input_rx.try_recv().is_err());

        handle_client_message(
            &input,
            Orientation::Portrait,
            &browser_frames,
            r#"{"type":"video_demand","active":true}"#,
            &mut pressed,
            &connection,
            &resync,
        );
        assert!(connection.video_active());
    }

    #[tokio::test]
    async fn video_demand_resumes_with_a_keyframe_request() {
        let (input, browser_frames, _input_rx) = test_state();
        let demand = ClientConnectionState::new(true);
        demand.video.store(true, Ordering::Relaxed);
        let resync = AtomicBool::new(false);
        let keyframes = browser_frames.clone();
        let mut pressed = HashSet::new();

        assert_eq!(
            handle_client_message(
                &input,
                Orientation::Portrait,
                &browser_frames,
                r#"{"type":"video_demand","active":false}"#,
                &mut pressed,
                &demand,
                &resync,
            ),
            ClientVideoFeedback::ResetAll
        );
        assert!(!demand.video_active());
        assert_eq!(
            handle_client_message(
                &input,
                Orientation::Portrait,
                &browser_frames,
                r#"{"type":"video_demand","active":true}"#,
                &mut pressed,
                &demand,
                &resync,
            ),
            ClientVideoFeedback::None
        );
        assert!(demand.video_active());
        assert!(resync.load(Ordering::Relaxed));
        tokio::time::timeout(Duration::from_millis(10), keyframes.keyframe_requested())
            .await
            .expect("video demand resume should request a keyframe");
    }

    #[tokio::test]
    async fn browser_decoder_keyframe_request_enters_resync() {
        let (input, browser_frames, _input_rx) = test_state();
        let demand = ClientConnectionState::new(true);
        demand.video.store(true, Ordering::Relaxed);
        let resync = AtomicBool::new(false);
        let keyframes = browser_frames.clone();

        assert_eq!(
            handle_client_message(
                &input,
                Orientation::Portrait,
                &browser_frames,
                r#"{"type":"browser_video_keyframe"}"#,
                &mut HashSet::new(),
                &demand,
                &resync,
            ),
            ClientVideoFeedback::ResetBrowser
        );
        assert!(resync.load(Ordering::Acquire));
        tokio::time::timeout(Duration::from_millis(10), keyframes.keyframe_requested())
            .await
            .expect("browser decoder recovery should request a keyframe");
    }

    #[test]
    fn frontend_metrics_reject_impossible_or_unbounded_values() {
        assert!(valid_frontend_metrics(
            5_000.0, 300, 0, 299, 600.0, 100.0, 2, 1
        ));
        assert!(!valid_frontend_metrics(
            5_000.0, 300, 301, 299, 600.0, 100.0, 2, 1,
        ));
        assert!(!valid_frontend_metrics(f64::NAN, 0, 0, 0, 0.0, 0.0, 0, 0,));
    }

    #[test]
    fn contact_validation_rejects_duplicate_ids() {
        let contacts = vec![
            WebContact {
                identity: 1,
                touching: true,
                x: 0.2,
                y: 0.3,
            },
            WebContact {
                identity: 1,
                touching: true,
                x: 0.4,
                y: 0.5,
            },
        ];
        assert!(validate_contacts(contacts, Orientation::Portrait).is_none());
    }

    #[test]
    fn contact_validation_unrotates_landscape() {
        let contacts = vec![WebContact {
            identity: 2,
            touching: true,
            x: 0.25,
            y: 0.75,
        }];
        let result = validate_contacts(contacts, Orientation::LandscapeRight).unwrap();
        assert_eq!(result[0].x, norm(0.75));
        assert_eq!(result[0].y, norm(0.75));
    }

    #[test]
    fn keyboard_messages_validate_and_track_pressed_usages() {
        let (input, browser_frames, mut input_rx) = test_state();
        let mut pressed = HashSet::new();

        for message in [
            r#"{"type":"keyboard_down","usage":4}"#,
            r#"{"type":"keyboard_down","usage":4}"#,
            r#"{"type":"keyboard_down","usage":65535}"#,
        ] {
            handle_test_client_message(&input, &browser_frames, message, &mut pressed);
        }

        assert!(matches!(
            input_rx.try_recv(),
            Ok(InputCmd::DeviceInput(DeviceInputCommand::KeyboardDown(4)))
        ));
        assert!(input_rx.try_recv().is_err());
        assert_eq!(pressed, HashSet::from([4]));

        handle_test_client_message(
            &input,
            &browser_frames,
            r#"{"type":"keyboard_up","usage":4}"#,
            &mut pressed,
        );
        assert!(matches!(
            input_rx.try_recv(),
            Ok(InputCmd::DeviceInput(DeviceInputCommand::KeyboardUp(4)))
        ));
        assert!(pressed.is_empty());
    }

    #[test]
    fn text_messages_are_bounded_before_dispatch() {
        let (input, browser_frames, mut input_rx) = test_state();
        let mut pressed = HashSet::new();

        handle_test_client_message(
            &input,
            &browser_frames,
            r#"{"type":"text","text":"Hello, iPhone!"}"#,
            &mut pressed,
        );
        handle_test_client_message(
            &input,
            &browser_frames,
            r#"{"type":"text","text":""}"#,
            &mut pressed,
        );
        let oversized =
            serde_json::to_string(&json!({ "type": "text", "text": "x".repeat(129) })).unwrap();
        handle_test_client_message(&input, &browser_frames, &oversized, &mut pressed);

        assert!(matches!(
            input_rx.try_recv(),
            Ok(InputCmd::DeviceInput(DeviceInputCommand::Text(text)))
                if text == "Hello, iPhone!"
        ));
        assert!(input_rx.try_recv().is_err());
    }

    #[test]
    fn system_action_messages_dispatch_only_known_actions() {
        let (input, browser_frames, mut input_rx) = test_state();
        let mut pressed = HashSet::new();

        handle_test_client_message(
            &input,
            &browser_frames,
            r#"{"type":"system_action","action":"APP-SWITCHER"}"#,
            &mut pressed,
        );
        handle_test_client_message(
            &input,
            &browser_frames,
            r#"{"type":"system_action","action":"shake"}"#,
            &mut pressed,
        );

        assert!(matches!(
            input_rx.try_recv(),
            Ok(InputCmd::DeviceInput(DeviceInputCommand::System(
                SystemAction::AppSwitcher
            )))
        ));
        assert!(input_rx.try_recv().is_err());
    }

    #[test]
    fn websocket_cleanup_releases_pressed_keyboard_usages() {
        let (input, _browser_frames, mut input_rx) = test_state();
        send_all_up(&input, &HashSet::from([0x04, 0xe1]));

        let commands: Vec<_> = std::iter::from_fn(|| input_rx.try_recv().ok()).collect();
        assert!(commands.iter().any(|command| matches!(
            command,
            InputCmd::DeviceInput(DeviceInputCommand::KeyboardUp(0x04))
        )));
        assert!(commands.iter().any(|command| matches!(
            command,
            InputCmd::DeviceInput(DeviceInputCommand::KeyboardUp(0xe1))
        )));
    }

    #[test]
    fn keyboard_usage_validation_matches_frontend_ranges() {
        for usage in [0x04, 0x65, 0x67, 0x73, 0x85, 0x87, 0x89, 0xe0, 0xe7] {
            assert!(valid_keyboard_usage(usage));
        }
        for usage in [0x00, 0x03, 0x74, 0x84, 0x86, 0x88, 0x8a, 0xdf, 0xe8] {
            assert!(!valid_keyboard_usage(usage));
        }
    }
}
