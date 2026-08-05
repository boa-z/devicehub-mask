//! Private real-time WebSocket transport.
//!
//! Axum routing only constructs this module's narrow state. The transport owns
//! subscriptions, serialization, WebCodecs frame delivery, and connection
//! cleanup; HTTP handlers and unrelated device services are not reachable.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use axum::extract::WebSocketUpgrade;
use axum::extract::ws::{Message, WebSocket};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::json;
use tokio::sync::broadcast;

use super::control_lease::BrowserControlLeases;
use super::input::{
    ClientConnectionState, ClientMessageContext, ClientVideoFeedback,
    handle_client_message_with_keymap, send_all_up,
};
use super::keymap::BrowserKeymapSession;
use crate::status;
use devicehub_core::VideoCounters;
use devicehub_runtime::{
    BrowserFrameDecision, BrowserVideoSlot, ClipboardSlot, Demand, DemandLease,
    DeviceSessionClient, FrameCredit, FramePacer, RuntimeClient, SessionCommandSlot as InputSink,
    browser_frame_decision, duration_average_ms, encode_packet,
};

const DEFAULT_MAX_IN_FLIGHT_FRAMES: usize = 8;
const MAX_IN_FLIGHT_FRAMES: usize = 8;
const AUDIO_CHANNEL_CAPACITY: usize = 16;
const AUDIO_PACKET_HEADER_BYTES: usize = 12;
const AUDIO_PACKET_MAGIC: &[u8; 4] = b"DHA1";
const MOBILE_PROTOCOL_VERSION: u16 = 1;

#[derive(Clone, Default)]
pub struct BrowserAudioSlot(Arc<Mutex<HashMap<String, broadcast::Sender<bytes::Bytes>>>>);

impl BrowserAudioSlot {
    pub fn publish(&self, selection_id: &str, pcm: bytes::Bytes) {
        let sender = self.sender(selection_id);
        let _ = sender.send(pcm);
    }

    fn subscribe(&self, selection_id: &str) -> broadcast::Receiver<bytes::Bytes> {
        self.sender(selection_id).subscribe()
    }

    fn sender(&self, selection_id: &str) -> broadcast::Sender<bytes::Bytes> {
        self.0
            .lock()
            .unwrap()
            .entry(selection_id.to_string())
            .or_insert_with(|| broadcast::channel(AUDIO_CHANNEL_CAPACITY).0)
            .clone()
    }
}

fn encode_audio_packet(pcm: &[u8]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(AUDIO_PACKET_HEADER_BYTES + pcm.len());
    packet.extend_from_slice(AUDIO_PACKET_MAGIC);
    packet.extend_from_slice(&devicehub_core::AUDIO_SAMPLE_RATE.to_be_bytes());
    packet.extend_from_slice(&u16::from(devicehub_core::AUDIO_CHANNELS).to_be_bytes());
    packet.extend_from_slice(&0_u16.to_be_bytes());
    packet.extend_from_slice(pcm);
    packet
}

/// Explicit host configuration for one WebSocket transport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WebSocketConfig {
    max_in_flight_frames: usize,
}

impl WebSocketConfig {
    pub fn new(max_in_flight_frames: usize) -> Self {
        Self {
            max_in_flight_frames: max_in_flight_frames.clamp(1, MAX_IN_FLIGHT_FRAMES),
        }
    }
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_IN_FLIGHT_FRAMES)
    }
}

#[derive(Clone)]
pub struct WebSocketState {
    application: RuntimeClient<std::path::PathBuf>,
    selection_id: Arc<str>,
    session: DeviceSessionClient<std::path::PathBuf>,
    browser_frames: BrowserVideoSlot,
    clipboard: ClipboardSlot,
    video_counters: VideoCounters,
    input: InputSink<std::path::PathBuf>,
    config: WebSocketConfig,
    browser_audio: Option<BrowserAudioSlot>,
    control_leases: BrowserControlLeases,
}

impl WebSocketState {
    pub fn new(
        application: RuntimeClient<std::path::PathBuf>,
        selection_id: String,
        session: DeviceSessionClient<std::path::PathBuf>,
        config: WebSocketConfig,
        browser_audio: Option<BrowserAudioSlot>,
        control_leases: BrowserControlLeases,
    ) -> Self {
        let browser_frames = session.browser_frames.clone();
        let clipboard = session.clipboard.clone();
        let video_counters = session.video_counters.clone();
        let input = session.commands.clone();
        Self {
            application,
            selection_id: Arc::from(selection_id),
            session,
            browser_frames,
            clipboard,
            video_counters,
            input,
            config,
            browser_audio,
            control_leases,
        }
    }
}

#[derive(Serialize)]
struct StreamMetricsView {
    transport_active: bool,
    source_fps: f64,
    decoded_fps: f64,
    published_fps: f64,
    sent_fps: f64,
    backend_dropped_fps: f64,
    frame_age_ms: f64,
    websocket_send_ms: f64,
    decoder_accept_ms: f64,
    presentation_ack_ms: f64,
    megabits_per_second: f64,
}

pub async fn upgrade(ws: WebSocketUpgrade, state: WebSocketState) -> impl IntoResponse {
    ws.protocols(["devicehub-mask"])
        .on_upgrade(move |socket| run(socket, state))
}

fn synchronize_browser_generation(
    observed: &mut u64,
    incoming: u64,
    pacer: &FramePacer,
    resync: &AtomicBool,
) -> bool {
    if *observed == incoming {
        return false;
    }
    *observed = incoming;
    pacer.clear_browser();
    resync.store(true, Ordering::Release);
    true
}

fn synchronize_demand_lease(active: bool, demand: &Demand, lease: &mut Option<DemandLease>) {
    match (active, lease.is_some()) {
        (true, false) => *lease = Some(demand.acquire()),
        (false, true) => *lease = None,
        _ => {}
    }
}

async fn run(socket: WebSocket, state: WebSocketState) {
    let mut control_notifications = state.control_leases.subscribe();
    let mut control_lease = state.control_leases.try_acquire(&state.selection_id);
    let control_granted = control_lease.is_some();
    let (control_tx, mut control_rx) = tokio::sync::watch::channel(control_granted);
    let (keymap_event_tx, mut keymap_event_rx) =
        tokio::sync::mpsc::channel::<serde_json::Value>(32);
    let (mut sender, mut receiver) = socket.split();
    let send_state = state.clone();
    let max_in_flight_frames = state.config.max_in_flight_frames;
    tracing::debug!(max_in_flight_frames, "configured video frame pipeline");
    let frame_pacer = Arc::new(FramePacer::new(max_in_flight_frames));
    // A newly connected WebView must opt into video. Control/status messages
    // remain available on pages that do not render the device stream.
    let connection = Arc::new(ClientConnectionState::new(control_granted));
    let browser_resync = Arc::new(AtomicBool::new(true));
    let send_pacer = frame_pacer.clone();
    let send_connection = connection.clone();
    let send_browser_resync = browser_resync.clone();
    let send_task = tokio::spawn(async move {
        let lease_message = json!({
            "type": "control_lease",
            "payload": { "granted": control_granted },
        });
        if sender
            .send(Message::Text(lease_message.to_string().into()))
            .await
            .is_err()
        {
            return;
        }
        let server_hello = json!({
            "type": "server_hello",
            "payload": {
                "protocol_version": MOBILE_PROTOCOL_VERSION,
                "target_platforms": ["ios"],
                "video": { "codec": "hevc", "packet": "DHV2" },
                "audio": { "codec": "pcm_s16le", "packet": "DHA1" },
                "input": ["multi_touch", "button", "system_action", "keyboard", "text", "rotate"],
                "control_lease": true,
            },
        });
        if sender
            .send(Message::Text(server_hello.to_string().into()))
            .await
            .is_err()
        {
            return;
        }
        let mut last_status = String::new();
        let mut browser_frame_rx = send_state.browser_frames.subscribe();
        let mut clipboard_rx = send_state.clipboard.subscribe();
        let mut device_event_rx = send_state.session.device_events.subscribe();
        let mut browser_audio_rx = send_state
            .browser_audio
            .as_ref()
            .map(|audio| audio.subscribe(&send_state.selection_id));
        let mut status_tick = tokio::time::interval(Duration::from_millis(250));
        status_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut metrics_tick = tokio::time::interval(Duration::from_secs(1));
        metrics_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut browser_resync_tick = tokio::time::interval(Duration::from_secs(1));
        browser_resync_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        browser_resync_tick.tick().await;
        let mut metrics_started = Instant::now();
        let mut metrics_counters = send_state.video_counters.snapshot();
        let mut metrics_browser_frame_version = send_state.browser_frames.version();
        let mut sent_frames = 0_u64;
        let mut sent_bytes = 0_u64;
        let mut frame_age = Duration::ZERO;
        let mut websocket_send_time = Duration::ZERO;
        let mut skipped_for_backpressure = 0_u64;
        let mut metrics_log_windows = 0_u8;
        let mut browser_generation = send_state.browser_frames.generation();
        loop {
            tokio::select! {
                changed = control_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let text = json!({
                        "type": "control_lease",
                        "payload": { "granted": *control_rx.borrow_and_update() },
                    }).to_string();
                    if sender.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                _ = status_tick.tick() => {
                    let snapshot = status::snapshot_for_session(
                        &send_state.application,
                        &send_state.selection_id,
                        &send_state.session,
                    );
                    if let Ok(text) = serde_json::to_string(
                        &json!({"type": "status", "payload": snapshot}),
                    ) && text != last_status {
                        last_status = text.clone();
                        if sender.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                }
                browser_frame = browser_frame_rx.recv() => {
                    match browser_frame {
                        Ok(frame) => {
                            if !send_connection.video_active() {
                                continue;
                            }
                            if synchronize_browser_generation(
                                &mut browser_generation,
                                frame.generation,
                                &send_pacer,
                                &send_browser_resync,
                            ) {
                                tracing::debug!(
                                    generation = frame.generation,
                                    "resetting browser video transport for a new device stream"
                                );
                            }
                            let completes_resync = match browser_frame_decision(
                                frame.key,
                                frame.sequence,
                                &send_browser_resync,
                                &send_pacer,
                            ) {
                                BrowserFrameDecision::Send { completes_resync } => completes_resync,
                                BrowserFrameDecision::SkipForResync => continue,
                                BrowserFrameDecision::Backpressured { entered_resync } => {
                                    skipped_for_backpressure += 1;
                                    if entered_resync {
                                        tracing::warn!(
                                            max_in_flight_frames,
                                            "browser decoder ingress credits exhausted; resyncing from a keyframe"
                                        );
                                        send_state.browser_frames.request_keyframe();
                                    }
                                    continue;
                                }
                            };
                            let packet = encode_packet(&frame);
                            frame_age += Instant::now()
                                .saturating_duration_since(frame.published_at);
                            sent_frames += 1;
                            sent_bytes += packet.len() as u64;
                            let send_started = Instant::now();
                            if sender.send(Message::Binary(packet.into())).await.is_err() {
                                send_pacer.release(FrameCredit::BrowserAccepted(frame.sequence));
                                break;
                            }
                            if completes_resync {
                                send_browser_resync.store(false, Ordering::Release);
                            }
                            websocket_send_time += send_started.elapsed();
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::warn!(skipped, "browser video client lagged; requesting keyframe");
                            send_browser_resync.store(true, Ordering::Release);
                            browser_frame_rx = browser_frame_rx.resubscribe();
                            send_state.browser_frames.request_keyframe();
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = browser_resync_tick.tick(), if send_connection.video_active()
                    && send_browser_resync.load(Ordering::Acquire) => {
                    tracing::debug!("browser video resync still waiting; requesting another keyframe");
                    send_state.browser_frames.request_keyframe();
                }
                clipboard = clipboard_rx.recv() => {
                    match clipboard {
                        Ok(event) => {
                            let Ok(text) = serde_json::to_string(
                                &json!({"type": "clipboard", "payload": event}),
                            ) else {
                                continue;
                            };
                            if sender.send(Message::Text(text.into())).await.is_err() {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::debug!(skipped, "WebSocket clipboard receiver skipped stale events");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                device_event = device_event_rx.recv() => {
                    match device_event {
                        Ok(event) => {
                            let Ok(text) = serde_json::to_string(
                                &json!({"type": "device_event", "payload": event}),
                            ) else {
                                continue;
                            };
                            if sender.send(Message::Text(text.into())).await.is_err() {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::debug!(skipped, "WebSocket device event receiver skipped stale events");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                event = keymap_event_rx.recv() => {
                    let Some(event) = event else {
                        break;
                    };
                    if sender.send(Message::Text(event.to_string().into())).await.is_err() {
                        break;
                    }
                }
                audio = async {
                    match browser_audio_rx.as_mut() {
                        Some(receiver) => receiver.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    match audio {
                        Ok(pcm) => {
                            if !send_connection.audio_active() {
                                continue;
                            }
                            if sender.send(Message::Binary(encode_audio_packet(&pcm).into())).await.is_err() {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::debug!(skipped, "browser audio client skipped stale PCM chunks");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            browser_audio_rx = None;
                        }
                    }
                }
                _ = metrics_tick.tick() => {
                    let elapsed = metrics_started.elapsed().as_secs_f64().max(f64::EPSILON);
                    let counters = send_state.video_counters.snapshot();
                    let browser_version = send_state.browser_frames.version();
                    let source_frames = counters.source_frames
                        .wrapping_sub(metrics_counters.source_frames);
                    let decoded_frames = counters.decoded_frames
                        .wrapping_sub(metrics_counters.decoded_frames);
                    let transport_active = counters.transport_events
                        != metrics_counters.transport_events;
                    let published_frames = browser_version
                        .wrapping_sub(metrics_browser_frame_version);
                    let pacer = send_pacer.take_metrics();
                    let metrics = StreamMetricsView {
                        transport_active,
                        source_fps: source_frames as f64 / elapsed,
                        decoded_fps: decoded_frames as f64 / elapsed,
                        published_fps: published_frames as f64 / elapsed,
                        sent_fps: sent_frames as f64 / elapsed,
                        backend_dropped_fps: published_frames.saturating_sub(sent_frames) as f64
                            / elapsed,
                        frame_age_ms: duration_average_ms(frame_age, sent_frames),
                        websocket_send_ms: duration_average_ms(websocket_send_time, sent_frames),
                        decoder_accept_ms: pacer.decoder_accept_average_ms,
                        presentation_ack_ms: pacer.presentation_average_ms,
                        megabits_per_second: sent_bytes as f64 * 8.0 / elapsed / 1_000_000.0,
                    };
                    metrics_log_windows += 1;
                    if metrics_log_windows >= 5 {
                        tracing::debug!(
                            target: "devicehub_mask::perf",
                            decoded_fps = metrics.decoded_fps,
                            source_fps = metrics.source_fps,
                            transport_active = metrics.transport_active,
                            published_fps = metrics.published_fps,
                            sent_fps = metrics.sent_fps,
                            backend_dropped_fps = metrics.backend_dropped_fps,
                            skipped_for_backpressure,
                            frame_age_ms = metrics.frame_age_ms,
                            websocket_send_ms = metrics.websocket_send_ms,
                            decoder_accept_ms = metrics.decoder_accept_ms,
                            decoder_accept_max_ms = pacer.decoder_accept_max_ms,
                            presentation_ack_ms = metrics.presentation_ack_ms,
                            presentation_ack_max_ms = pacer.presentation_max_ms,
                            expired_frame_credits = pacer.expired_credits,
                            megabits_per_second = metrics.megabits_per_second,
                            "video output performance"
                        );
                        metrics_log_windows = 0;
                    }
                    let Ok(text) = serde_json::to_string(
                        &json!({"type": "metrics", "payload": metrics}),
                    ) else {
                        continue;
                    };
                    if sender.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                    metrics_started = Instant::now();
                    metrics_counters = counters;
                    metrics_browser_frame_version = browser_version;
                    sent_frames = 0;
                    sent_bytes = 0;
                    frame_age = Duration::ZERO;
                    websocket_send_time = Duration::ZERO;
                    skipped_for_backpressure = 0;
                }
            }
        }
    });

    let mut pressed_keyboard = HashSet::new();
    let mut keymap = BrowserKeymapSession::default();
    let mut keymap_tick = tokio::time::interval(Duration::from_millis(16));
    keymap_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut video_lease = None;
    let mut audio_lease = None;
    loop {
        let message = tokio::select! {
            message = receiver.next() => message,
            _ = keymap_tick.tick(), if connection.control_granted() => {
                if let Some(event) = keymap.tick(&state.input, state.session.orientation.get())
                    && keymap_event_tx.send(event).await.is_err()
                {
                    break;
                }
                continue;
            }
            released = control_notifications.recv(), if control_lease.is_none() => {
                let should_retry = match released {
                    Ok(selection_id) => selection_id == state.selection_id.as_ref(),
                    Err(broadcast::error::RecvError::Lagged(_)) => true,
                    Err(broadcast::error::RecvError::Closed) => false,
                };
                if should_retry
                    && let Some(lease) = state.control_leases.try_acquire(&state.selection_id)
                {
                    control_lease = Some(lease);
                    connection.grant_control();
                    control_tx.send_replace(true);
                }
                continue;
            }
        };
        let Some(Ok(message)) = message else {
            break;
        };
        match message {
            Message::Text(text) => {
                match handle_client_message_with_keymap(
                    ClientMessageContext {
                        input: &state.input,
                        orientation: state.session.orientation.get(),
                        browser_frames: &state.browser_frames,
                        connection: &connection,
                        browser_resync: &browser_resync,
                    },
                    &text,
                    &mut pressed_keyboard,
                    &mut keymap,
                ) {
                    ClientVideoFeedback::None => {}
                    ClientVideoFeedback::ProtocolError(message) => {
                        tracing::warn!(%message, "closing WebSocket after invalid client handshake");
                        break;
                    }
                    ClientVideoFeedback::BrowserAccepted(sequence) => {
                        frame_pacer.release(FrameCredit::BrowserAccepted(sequence));
                    }
                    ClientVideoFeedback::FramePresented(sequence) => {
                        frame_pacer.presented(sequence);
                    }
                    ClientVideoFeedback::ResetBrowser => frame_pacer.clear_browser(),
                    ClientVideoFeedback::ResetAll => frame_pacer.clear(),
                    ClientVideoFeedback::KeymapEvent(event) => {
                        if keymap_event_tx.send(event).await.is_err() {
                            break;
                        }
                    }
                }
                synchronize_demand_lease(
                    connection.video_active(),
                    &state.session.media_demand.video,
                    &mut video_lease,
                );
                synchronize_demand_lease(
                    connection.audio_active(),
                    &state.session.media_demand.audio,
                    &mut audio_lease,
                );
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    send_task.abort();
    if connection.control_granted() {
        keymap.release(&state.input, state.session.orientation.get());
        send_all_up(&state.input, &pressed_keyboard);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use devicehub_runtime::{FrameCredit, FramePacer};

    use super::{
        BrowserAudioSlot, WebSocketConfig, encode_audio_packet, synchronize_browser_generation,
        synchronize_demand_lease,
    };

    #[test]
    fn transport_bounds_host_supplied_frame_credits() {
        assert_eq!(WebSocketConfig::new(0).max_in_flight_frames, 1);
        assert_eq!(WebSocketConfig::new(2).max_in_flight_frames, 2);
        assert_eq!(WebSocketConfig::new(usize::MAX).max_in_flight_frames, 8);
    }

    #[test]
    fn websocket_demand_lease_tracks_client_state() {
        let demand = devicehub_runtime::Demand::default();
        let mut lease = None;

        synchronize_demand_lease(true, &demand, &mut lease);
        assert!(demand.enabled());
        synchronize_demand_lease(true, &demand, &mut lease);
        assert!(demand.enabled());
        synchronize_demand_lease(false, &demand, &mut lease);
        assert!(!demand.enabled());
    }

    #[test]
    fn stream_generation_change_clears_stale_credits_and_requires_keyframe() {
        let pacer = FramePacer::new(1);
        assert!(pacer.try_acquire(FrameCredit::BrowserAccepted(7)));
        let resync = AtomicBool::new(false);
        let mut generation = 1;

        assert!(synchronize_browser_generation(
            &mut generation,
            2,
            &pacer,
            &resync,
        ));
        assert_eq!(generation, 2);
        assert!(resync.load(Ordering::Acquire));
        assert!(pacer.try_acquire(FrameCredit::BrowserAccepted(8)));
    }

    #[test]
    fn audio_packet_has_stable_header_and_preserves_pcm() {
        let packet = encode_audio_packet(&[1, 2, 3, 4]);
        assert_eq!(&packet[..4], b"DHA1");
        assert_eq!(u32::from_be_bytes(packet[4..8].try_into().unwrap()), 48_000);
        assert_eq!(u16::from_be_bytes(packet[8..10].try_into().unwrap()), 2);
        assert_eq!(&packet[12..], &[1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn browser_audio_subscriptions_are_device_scoped() {
        let audio = BrowserAudioSlot::default();
        let mut phone = audio.subscribe("phone::usb");
        let mut tablet = audio.subscribe("tablet::usb");

        audio.publish("phone::usb", bytes::Bytes::from_static(b"phone"));

        assert_eq!(
            phone.try_recv().unwrap(),
            bytes::Bytes::from_static(b"phone")
        );
        assert!(tablet.try_recv().is_err());
    }
}
