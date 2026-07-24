// The async device session: connect over the tunnel, bring up the screen media
// stream (which both sources the video AND holds open the HID auth gate), then
// run the video pipeline and dispatch input commands to the device's HID surfaces.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::ChildStderr;
use tokio::sync::Notify;
use tokio::sync::mpsc::{Receiver, Sender, UnboundedReceiver};

use idevice::{
    IdeviceError, IdeviceService, ReadWrite, RemoteXpcClient, RsdService,
    core_device::{
        AppServiceClient, CallInfoBlob, CoreDeviceError, DataInclusionPolicy, DisplayServiceClient,
        GENERAL_PASTEBOARD, HevcDepacketizer, Orientation as DevOrientation,
        OrientationServiceClient, PasteboardServiceClient, PasteboardSnapshot, ReportBlock,
        RotationDirection, RtpPacket, SenderReport, UTI_PNG, build_frame_ack,
        build_keyframe_request, build_liveness, build_rctl, build_screen_audio_offer,
        build_screen_video_offer, build_start_audio_parameters, build_start_video_parameters,
        hid::{
            ButtonState, DIGITIZER_SURFACE_MAIN_TOUCHSCREEN, IndigoHidClient,
            TOUCHSCREEN_STATE_CONTACT, TOUCHSCREEN_STATE_RELEASE,
        },
        is_rtcp, parse_answer_media_blob,
    },
    core_device_proxy::CoreDeviceProxy,
    diagnostics_relay::DiagnosticsRelayClient,
    installation_proxy::InstallationProxyClient,
    lockdown::LockdownClient,
    mobile_image_mounter::ImageMounter,
    mobileactivationd::MobileActivationdClient,
    provider::{IdeviceProvider, TcpProvider},
    remote_pairing::{
        RemotePairingClient, RpPairingFile, RpPairingSocket, connect_tls_psk_tunnel_native,
    },
    rsd::RsdHandshake,
    springboardservices::{InterfaceOrientation, SpringBoardServicesClient},
    tcp::handle::{AdapterHandle, UdpSocketHandle},
    usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdDevice},
    utils::installation::{install_package_with_callback, upgrade_package_with_callback},
};
use tokio::process::ChildStdin;

use crate::audio_output::AudioOutput;
use crate::decode;
use crate::developer_mode;
use crate::hid::{UniversalHidClient, build_multitouch_report};
use crate::ipa::{
    InstalledAppMatch, IpaArchiveMetadata, IpaCompatibility, IpaOperation, IpaPreflight,
    IpaPreflightIssue,
};
use crate::protocol::{
    ActiveSlot, AppOperationKind, AppOperationSlot, ClipboardContentKind, ClipboardEvent,
    ClipboardSlot, ConnKind, ControlCmd, DeviceActivationState, DeviceApp, DeviceBattery,
    DeviceDetails, DeviceInfo, DeviceListSlot, DevicePairingState, DeviceRegionalSettings,
    DeviceStorage, ErrorSlot, ForgetDeviceOutcome, ForgetDeviceResult, FrameFormat, FrameSlot,
    InputCmd, InputSink, KeyMods, LocationStatus, LocationStatusSlot, Orientation, OrientationSlot,
    PairDeviceOutcome, PairDeviceResult, RotateDir, StatusSlot, VideoCounters, clipboard_preview,
    device_selector,
};
use crate::wifi_devices::{WifiDiscovery, WifiEndpoint};
use crate::{location, location::LocationCommand};
use crate::{performance, supervisor};

/// `clientSupportedFeatures` the controller advertises for screen sharing.
const CLIENT_SUPPORTED_FEATURES: u64 = 140;

/// Named iOS hardware buttons -> (usage_page, usage_code, hold_ms). Consumer-page
/// (`0x0C`) codes come from CoreDevice's `HIDUsageCode<ConsumerPage>` table; the
/// action button (iPhone 15 Pro+) lives on the telephony page (`0x0B`) usage `0x2D`.
pub const NAMED_BUTTONS: &[(&str, u64, u64, u64)] = &[
    ("home", 0x0C, 0x40, 80),
    ("lock", 0x0C, 0x30, 200),
    ("volume-up", 0x0C, 0xE9, 80),
    ("volume-down", 0x0C, 0xEA, 80),
    ("mute", 0x0C, 0xE2, 80),
    ("siri", 0x0C, 0xCF, 1200),
    ("action", 0x0B, 0x2D, 80),
];

/// HID Keyboard/Keypad usages for the left-hand modifier keys.
const KEY_LEFT_CTRL: u64 = 0xE0;
const KEY_LEFT_SHIFT: u64 = 0xE1;
const KEY_LEFT_ALT: u64 = 0xE2;
const KEY_LEFT_CMD: u64 = 0xE3;
const KEY_V: u64 = 0x19;

/// The device's encoder sends a single IDR then only P-frames, so a dropped
/// packet corrupts the picture permanently; recovery is an RTCP keyframe request
/// (PLI + FIR) that makes the encoder emit a fresh IDR on the same stream.
///
/// After requesting a keyframe, ignore further triggers for this long so a burst
/// of decode errors yields a single request, not a storm.
const KEYFRAME_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(1500);
/// If no decoded frame arrives for this long, treat the stream as silently stalled
/// (no packets, so no frames and no decode errors - e.g. macOS App Nap on a
/// backgrounded window) and request a fresh keyframe.
const STALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
/// How long the locked stream must go silent before we migrate to a different
/// SSRC: long enough to ignore stray packets from a competing/leaked sender,
/// short enough to pick up a real stream restart promptly.
const SSRC_TAKEOVER_GRACE: std::time::Duration = std::time::Duration::from_millis(250);
const AUDIO_DECODER_STABLE_RUNTIME: Duration = Duration::from_secs(10);
/// RTCP Receiver Report interval. AVConference uses RTCP for liveness; if reports
/// stop, the device's sender eventually stops too and the screen freezes.
const RTCP_REPORT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
/// The local UDP port we tell the device to send video RTP/RTCP *from*. Used as
/// the default RTCP destination until we observe where the device's RTCP originates.
const VIDEO_SENDER_PORT: u16 = 50001;
/// Keep the display rotation in sync when an app switches between portrait and
/// landscape. The screen stream itself is always native portrait.
const ORIENTATION_POLL_INTERVAL: Duration = Duration::from_millis(500);
/// Bound compressed video waiting for ffmpeg. This is deliberately byte-based:
/// access-unit sizes vary dramatically between static P-frames and an IRAP.
const HEVC_QUEUE_MAX_BYTES: usize = 16 * 1024 * 1024;
const HEVC_AUD: &[u8] = b"\0\0\0\x01\x46\x01\x50";

#[derive(Debug, Clone, Copy)]
struct RunningStats {
    count: u64,
    mean: f64,
    squared_deviations: f64,
    min: f64,
    max: f64,
}

impl Default for RunningStats {
    fn default() -> Self {
        Self {
            count: 0,
            mean: 0.0,
            squared_deviations: 0.0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
        }
    }
}

impl RunningStats {
    fn push(&mut self, value: f64) {
        self.count += 1;
        let delta = value - self.mean;
        self.mean += delta / self.count as f64;
        self.squared_deviations += delta * (value - self.mean);
        self.min = self.min.min(value);
        self.max = self.max.max(value);
    }

    fn mean(&self) -> Option<f64> {
        (self.count > 0).then_some(self.mean)
    }

    fn min(&self) -> Option<f64> {
        (self.count > 0).then_some(self.min)
    }

    fn max(&self) -> Option<f64> {
        (self.count > 0).then_some(self.max)
    }

    fn standard_deviation(&self) -> Option<f64> {
        (self.count > 0).then(|| (self.squared_deviations / self.count as f64).sqrt())
    }
}

#[derive(Debug)]
struct HevcAccessUnit {
    bytes: Vec<u8>,
    is_irap: bool,
    rtp_timestamp: u32,
}

#[derive(Debug)]
struct QueuedHevcAccessUnit {
    access_unit: HevcAccessUnit,
    enqueued_at: Instant,
}

#[derive(Debug, Default)]
struct AccessUnitAssembler {
    pending: Vec<u8>,
    pending_timestamp: Option<u32>,
}

impl AccessUnitAssembler {
    fn push(&mut self, bytes: &[u8], rtp_timestamp: u32) -> Vec<HevcAccessUnit> {
        if self.pending.is_empty() {
            self.pending_timestamp = Some(rtp_timestamp);
        }
        self.pending.extend_from_slice(bytes);
        let mut completed = Vec::new();
        loop {
            // The depacketizer inserts an AUD before each new RTP timestamp. If
            // pending already starts with one, search for the following AUD.
            let search_from = usize::from(self.pending.starts_with(HEVC_AUD)) * HEVC_AUD.len();
            let Some(relative_boundary) = find_subslice(&self.pending[search_from..], HEVC_AUD)
            else {
                break;
            };
            let boundary = search_from + relative_boundary;
            let remaining = self.pending.split_off(boundary);
            let access_unit = std::mem::replace(&mut self.pending, remaining);
            if !access_unit.is_empty() {
                completed.push(HevcAccessUnit {
                    is_irap: annexb_contains_irap(&access_unit),
                    bytes: access_unit,
                    rtp_timestamp: self.pending_timestamp.unwrap_or(rtp_timestamp),
                });
            }
            self.pending_timestamp = Some(rtp_timestamp);
        }
        completed
    }

    fn finish(&mut self) -> Option<HevcAccessUnit> {
        if self.pending.is_empty() {
            return None;
        }
        let bytes = std::mem::take(&mut self.pending);
        Some(HevcAccessUnit {
            is_irap: annexb_contains_irap(&bytes),
            bytes,
            rtp_timestamp: self.pending_timestamp.take()?,
        })
    }

    fn clear(&mut self) {
        self.pending.clear();
        self.pending_timestamp = None;
    }
}

#[derive(Debug, Default)]
struct RtpVideoClock {
    last_timestamp: Option<u32>,
    elapsed_ticks: u64,
}

impl RtpVideoClock {
    fn timestamp_us(&mut self, timestamp: u32) -> u64 {
        if let Some(previous) = self.last_timestamp {
            let delta = timestamp.wrapping_sub(previous);
            if delta < (1 << 31) {
                self.elapsed_ticks = self.elapsed_ticks.saturating_add(u64::from(delta));
            }
        }
        self.last_timestamp = Some(timestamp);
        self.elapsed_ticks.saturating_mul(1_000_000) / 90_000
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn annexb_contains_irap(bytes: &[u8]) -> bool {
    bytes
        .windows(5)
        .any(|window| window[..4] == [0, 0, 0, 1] && (16..=23).contains(&((window[4] >> 1) & 0x3f)))
}

#[derive(Debug)]
enum HevcQueuePush {
    Enqueued,
    Dropped,
    NeedsKeyframe {
        queued_bytes: usize,
        incoming_bytes: usize,
    },
    Recovered {
        dropped_access_units: u64,
        dropped_bytes: u64,
    },
}

#[derive(Debug)]
struct HevcQueueState {
    access_units: VecDeque<QueuedHevcAccessUnit>,
    queued_bytes: usize,
    peak_bytes: usize,
    waiting_for_irap: bool,
    dropped_access_units: u64,
    dropped_bytes: u64,
    wait_samples: u64,
    wait_total_micros: u64,
    wait_max_micros: u64,
    closed: bool,
}

#[derive(Debug)]
struct HevcQueue {
    max_bytes: usize,
    state: Mutex<HevcQueueState>,
    ready: Notify,
}

impl HevcQueue {
    fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            state: Mutex::new(HevcQueueState {
                access_units: VecDeque::new(),
                queued_bytes: 0,
                peak_bytes: 0,
                waiting_for_irap: false,
                dropped_access_units: 0,
                dropped_bytes: 0,
                wait_samples: 0,
                wait_total_micros: 0,
                wait_max_micros: 0,
                closed: false,
            }),
            ready: Notify::new(),
        }
    }

    fn push(&self, access_unit: HevcAccessUnit) -> HevcQueuePush {
        let incoming_bytes = access_unit.bytes.len();
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return HevcQueuePush::Dropped;
        }

        if state.waiting_for_irap {
            if !access_unit.is_irap || incoming_bytes > self.max_bytes {
                state.dropped_access_units = state.dropped_access_units.saturating_add(1);
                state.dropped_bytes = state.dropped_bytes.saturating_add(incoming_bytes as u64);
                return HevcQueuePush::Dropped;
            }
            state.waiting_for_irap = false;
            let dropped_access_units = std::mem::take(&mut state.dropped_access_units);
            let dropped_bytes = std::mem::take(&mut state.dropped_bytes);
            state.queued_bytes = incoming_bytes;
            state.peak_bytes = state.peak_bytes.max(state.queued_bytes);
            state.access_units.push_back(QueuedHevcAccessUnit {
                access_unit,
                enqueued_at: Instant::now(),
            });
            drop(state);
            self.ready.notify_one();
            return HevcQueuePush::Recovered {
                dropped_access_units,
                dropped_bytes,
            };
        }

        if incoming_bytes > self.max_bytes
            || state.queued_bytes.saturating_add(incoming_bytes) > self.max_bytes
        {
            let queued_bytes = state.queued_bytes;
            state.dropped_access_units = state
                .dropped_access_units
                .saturating_add(state.access_units.len() as u64);
            state.dropped_bytes = state.dropped_bytes.saturating_add(queued_bytes as u64);
            state.access_units.clear();
            state.queued_bytes = 0;

            if access_unit.is_irap && incoming_bytes <= self.max_bytes {
                let dropped_access_units = std::mem::take(&mut state.dropped_access_units);
                let dropped_bytes = std::mem::take(&mut state.dropped_bytes);
                state.access_units.push_back(QueuedHevcAccessUnit {
                    access_unit,
                    enqueued_at: Instant::now(),
                });
                state.queued_bytes = incoming_bytes;
                state.peak_bytes = state.peak_bytes.max(state.queued_bytes);
                drop(state);
                self.ready.notify_one();
                return HevcQueuePush::Recovered {
                    dropped_access_units,
                    dropped_bytes,
                };
            }

            state.waiting_for_irap = true;
            state.dropped_access_units = state.dropped_access_units.saturating_add(1);
            state.dropped_bytes = state.dropped_bytes.saturating_add(incoming_bytes as u64);
            return HevcQueuePush::NeedsKeyframe {
                queued_bytes,
                incoming_bytes,
            };
        }

        state.queued_bytes += incoming_bytes;
        state.peak_bytes = state.peak_bytes.max(state.queued_bytes);
        state.access_units.push_back(QueuedHevcAccessUnit {
            access_unit,
            enqueued_at: Instant::now(),
        });
        drop(state);
        self.ready.notify_one();
        HevcQueuePush::Enqueued
    }

    fn force_resync(&self) -> (u64, u64) {
        let mut state = self.state.lock().unwrap();
        state.dropped_access_units = state
            .dropped_access_units
            .saturating_add(state.access_units.len() as u64);
        state.dropped_bytes = state
            .dropped_bytes
            .saturating_add(state.queued_bytes as u64);
        state.access_units.clear();
        state.queued_bytes = 0;
        state.waiting_for_irap = true;
        (state.dropped_access_units, state.dropped_bytes)
    }

    async fn pop(&self) -> Option<HevcAccessUnit> {
        loop {
            let notified = self.ready.notified();
            {
                let mut state = self.state.lock().unwrap();
                if let Some(queued) = state.access_units.pop_front() {
                    state.queued_bytes -= queued.access_unit.bytes.len();
                    let wait_micros = queued.enqueued_at.elapsed().as_micros() as u64;
                    state.wait_samples = state.wait_samples.saturating_add(1);
                    state.wait_total_micros = state.wait_total_micros.saturating_add(wait_micros);
                    state.wait_max_micros = state.wait_max_micros.max(wait_micros);
                    return Some(queued.access_unit);
                }
                if state.closed {
                    return None;
                }
            }
            notified.await;
        }
    }

    fn take_snapshot(&self) -> HevcQueueSnapshot {
        let mut state = self.state.lock().unwrap();
        let snapshot = HevcQueueSnapshot {
            queued_access_units: state.access_units.len(),
            queued_bytes: state.queued_bytes,
            peak_bytes: state.peak_bytes,
            waiting_for_irap: state.waiting_for_irap,
            wait_ms: if state.wait_samples == 0 {
                0.0
            } else {
                state.wait_total_micros as f64 / state.wait_samples as f64 / 1000.0
            },
            wait_max_ms: state.wait_max_micros as f64 / 1000.0,
        };
        state.peak_bytes = state.queued_bytes;
        state.wait_samples = 0;
        state.wait_total_micros = 0;
        state.wait_max_micros = 0;
        snapshot
    }

    fn close(&self) {
        self.state.lock().unwrap().closed = true;
        self.ready.notify_waiters();
    }
}

#[derive(Debug, Clone, Copy)]
struct HevcQueueSnapshot {
    queued_access_units: usize,
    queued_bytes: usize,
    peak_bytes: usize,
    waiting_for_irap: bool,
    wait_ms: f64,
    wait_max_ms: f64,
}

/// Where the device's RTCP arrives, learned at runtime (transport isn't negotiated
/// explicitly). Until we've seen any, we send to both candidates.
#[derive(Debug, Clone, Copy, Default)]
enum RtcpPeer {
    #[default]
    Unknown,
    /// rtcp-mux: device sends RTCP on the RTP port; we reply over the RTP socket
    /// to this (the device's source) port.
    Mux(u16),
    /// Separate RTCP port (RFC 3550): we reply over the dedicated RTCP socket.
    Separate(u16),
}

fn orientation_from_interface(orientation: InterfaceOrientation) -> Option<Orientation> {
    match orientation {
        InterfaceOrientation::Portrait => Some(Orientation::Portrait),
        InterfaceOrientation::PortraitUpsideDown => Some(Orientation::PortraitUpsideDown),
        // SpringBoard's interface labels describe the opposite landscape edge
        // from CoreDevice's screen-stream orientation labels.
        InterfaceOrientation::LandscapeLeft => Some(Orientation::LandscapeRight),
        InterfaceOrientation::LandscapeRight => Some(Orientation::LandscapeLeft),
        InterfaceOrientation::Unknown => None,
    }
}

async fn refresh_interface_orientation(
    springboard: &mut SpringBoardServicesClient,
    orientation_view: &OrientationSlot,
) -> Result<(), IdeviceError> {
    let Some(orientation) =
        orientation_from_interface(springboard.get_interface_orientation().await?)
    else {
        return Ok(());
    };

    if orientation_view.get() != orientation {
        tracing::info!(?orientation, "device interface orientation changed");
        orientation_view.set(orientation);
    }
    Ok(())
}

async fn watch_interface_orientation(
    mut springboard: SpringBoardServicesClient,
    orientation_view: OrientationSlot,
) {
    let mut reported_error = false;
    loop {
        match refresh_interface_orientation(&mut springboard, &orientation_view).await {
            Ok(()) => reported_error = false,
            Err(error) if !reported_error => {
                tracing::warn!("could not refresh device interface orientation: {error:?}");
                reported_error = true;
            }
            Err(_) => {}
        }
        tokio::time::sleep(ORIENTATION_POLL_INTERVAL).await;
    }
}

/// The last Sender Report we received, so a Receiver Report can echo `LSR`/`DLSR`.
#[derive(Debug, Clone, Copy)]
struct SrEcho {
    /// Middle 32 bits of the SR's NTP timestamp.
    ntp_middle: u32,
    received_at: Instant,
}

/// RTP reception statistics for a single source, enough to fill in a Receiver
/// Report block (RFC 3550, simplified - jitter is not tracked).
#[derive(Debug, Default)]
struct RtpStats {
    initialized: bool,
    /// Extended sequence number of the first packet seen.
    base_seq: u32,
    /// Extended highest sequence number seen (`cycles << 16 | seq`).
    ext_max: u32,
    received: u32,
    /// Snapshots from the previous report, for the per-interval loss fraction.
    expected_prior: u32,
    received_prior: u32,
}

impl RtpStats {
    /// Fold one packet's 16-bit sequence number into the running stats,
    /// maintaining the extended (cycle-aware) highest sequence number.
    fn on_packet(&mut self, seq: u16) {
        let seq = seq as u32;
        if !self.initialized {
            self.initialized = true;
            self.base_seq = seq;
            self.ext_max = seq;
            self.received = 1;
            return;
        }
        let cycles = self.ext_max & !0xffff;
        let max_lo = self.ext_max & 0xffff;
        // Resolve `seq` to an extended number nearest the current max, treating a
        // forward distance ≥ 0x8000 as the short way around the 16-bit wrap.
        let ext = if seq >= max_lo {
            if seq - max_lo < 0x8000 {
                cycles | seq
            } else {
                cycles.wrapping_sub(0x10000) | seq
            }
        } else if max_lo - seq < 0x8000 {
            cycles | seq
        } else {
            (cycles + 0x10000) | seq
        };
        if ext > self.ext_max {
            self.ext_max = ext;
        }
        self.received += 1;
    }

    /// Produce a Receiver Report block for this source, advancing the per-interval
    /// loss bookkeeping. `lsr`/`dlsr` come from the last Sender Report (0 if none).
    fn report_block(&mut self, source_ssrc: u32, lsr: u32, dlsr: u32) -> ReportBlock {
        let expected = self.ext_max.wrapping_sub(self.base_seq).wrapping_add(1);
        let cumulative_lost = expected.saturating_sub(self.received);
        let expected_interval = expected.wrapping_sub(self.expected_prior);
        let received_interval = self.received.wrapping_sub(self.received_prior);
        self.expected_prior = expected;
        self.received_prior = self.received;
        let lost_interval = expected_interval.saturating_sub(received_interval);
        let fraction_lost = if expected_interval == 0 || lost_interval == 0 {
            0
        } else {
            ((lost_interval << 8) / expected_interval) as u8
        };
        ReportBlock {
            source_ssrc,
            fraction_lost,
            cumulative_lost: cumulative_lost & 0x00ff_ffff,
            highest_seq: self.ext_max,
            jitter: 0,
            lsr,
            dlsr,
        }
    }
}

/// State shared between the RTP receive loop, the RTCP receive loop(s), and the
/// RTCP send loop.
#[derive(Default)]
struct RtcpShared {
    /// The device's video SSRC, once we've locked onto the stream.
    media_ssrc: Option<u32>,
    stats: RtpStats,
    sr_echo: Option<SrEcho>,
    peer: RtcpPeer,
    /// Count of complete frames received (marker-bit terminated).
    frames: u32,
}

impl RtcpShared {
    /// Highest RTP sequence number received, relative to the first packet's
    /// sequence number (the form Apple's `RCTL` carries). 0 until any packet.
    fn highest_seq_rel(&self) -> u16 {
        if self.stats.initialized {
            self.stats.ext_max.wrapping_sub(self.stats.base_seq) as u16
        } else {
            0
        }
    }
}

impl RtcpShared {
    /// Record an inbound RTCP datagram: where it came from (so replies go to the
    /// right place) and, if it's a Sender Report, the echo data.
    fn note_inbound(&mut self, buf: &[u8], source_port: u16, separate: bool, now: Instant) {
        self.peer = if separate {
            RtcpPeer::Separate(source_port)
        } else {
            RtcpPeer::Mux(source_port)
        };
        if let Some(sr) = SenderReport::parse_first(buf) {
            self.sr_echo = Some(SrEcho {
                ntp_middle: sr.ntp_middle,
                received_at: now,
            });
            self.media_ssrc.get_or_insert(sr.ssrc);
        }
    }

    /// Report blocks for a Receiver Report (empty until we know the source SSRC).
    fn report_blocks(&mut self, now: Instant) -> Vec<ReportBlock> {
        let Some(ssrc) = self.media_ssrc else {
            return Vec::new();
        };
        let (lsr, dlsr) = match self.sr_echo {
            Some(e) => {
                let delay = now.saturating_duration_since(e.received_at);
                (e.ntp_middle, (delay.as_secs_f64() * 65536.0) as u32)
            }
            None => (0, 0),
        };
        vec![self.stats.report_block(ssrc, lsr, dlsr)]
    }
}

/// How often to re-scan for attached devices while idle, so the picker reflects
/// devices coming and going without a manual refresh.
const IDLE_RESCAN: Duration = Duration::from_secs(2);
/// Cap on how long we wait for a session to tear down when switching/quitting, so
/// a wedged session can't hang the transition forever.
const SWITCH_GRACE: Duration = Duration::from_secs(3);
/// Briefly yield after a Wi-Fi transport failure before rebuilding the complete
/// RemotePairing tunnel. Child services cannot repair a dead parent tunnel.
const WIFI_RECONNECT_DELAY: Duration = Duration::from_secs(1);
/// Per-device budget for resolving `DeviceName` over lockdown; on timeout we fall
/// back to the UDID so a flaky/locked device doesn't stall the picker.
const NAME_TIMEOUT: Duration = Duration::from_secs(2);
/// Pairing includes a user confirmation on the device, but must not wait forever
/// when the prompt is ignored or the USB transport disappears.
const PAIRING_TIMEOUT: Duration = Duration::from_secs(90);
/// Removing trust is local/device I/O only and never waits for a user dialog.
const FORGET_DEVICE_TIMEOUT: Duration = Duration::from_secs(10);

/// What the manager should do once the current session is no longer running.
enum Next {
    /// Connect to this UDID.
    Switch(String),
    /// Rebuild a dropped Wi-Fi session while preserving the selected transport.
    RetryWifi(String),
    /// Stop the active session, then pair this USB transport.
    Pair {
        selection_id: String,
        reply: tokio::sync::oneshot::Sender<PairDeviceResult>,
    },
    /// Stop the active session, then revoke this USB trust relationship.
    Forget {
        selection_id: String,
        reply: tokio::sync::oneshot::Sender<ForgetDeviceResult>,
    },
    /// Go idle (no device); wait for the user to pick one.
    Idle,
    /// The UI is gone - exit the manager entirely.
    Quit,
}

#[derive(Debug, Clone, Copy)]
enum DevicePowerAction {
    Lock,
    Restart,
    Shutdown,
}

#[derive(Clone, Default)]
struct DevicePowerSlot(Arc<AtomicBool>);

impl DevicePowerSlot {
    fn try_start(&self) -> Result<DevicePowerLease, String> {
        self.0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| DevicePowerLease(self.clone()))
            .map_err(|_| "another device power command is already running".into())
    }
}

struct DevicePowerLease(DevicePowerSlot);

impl Drop for DevicePowerLease {
    fn drop(&mut self) {
        self.0.0.store(false, Ordering::Release);
    }
}

#[derive(Clone)]
struct SessionViews {
    status: StatusSlot,
    orientation: OrientationSlot,
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
    device_events: crate::device_events::DeviceEventSlot,
    network_capture: crate::network_capture::NetworkCaptureSlot,
    bluetooth_capture: crate::bluetooth_capture::BluetoothCaptureSlot,
    device_backup: crate::device_backup::DeviceBackupSlot,
    sysdiagnose: crate::sysdiagnose::SysdiagnoseSlot,
    log_archive: crate::log_archive::LogArchiveSlot,
    developer_image: crate::developer_image::DeveloperImageMountSlot,
    device_conditions: crate::device_conditions::DeviceConditionSlot,
}

#[derive(Clone)]
struct SessionVideo {
    frame_format: FrameFormat,
    decoder_backend: crate::settings::VideoDecoderBackend,
    counters: VideoCounters,
    frames: FrameSlot,
    browser_frames: crate::browser_video::BrowserVideoSlot,
    audio_enabled: bool,
    clipboard_sync_enabled: bool,
    audio: AudioOutput,
}

#[derive(Clone, Debug)]
struct UsbmuxdEndpoint {
    device: UsbmuxdDevice,
    address: UsbmuxdAddr,
}

#[derive(Clone, Debug)]
enum SessionEndpoint {
    Usbmuxd(Box<UsbmuxdEndpoint>),
    Wifi(Box<WifiEndpoint>),
}

impl SessionEndpoint {
    fn udid(&self) -> &str {
        match self {
            Self::Usbmuxd(endpoint) => &endpoint.device.udid,
            Self::Wifi(endpoint) => &endpoint.udid,
        }
    }

    fn connection(&self) -> ConnKind {
        match self {
            Self::Usbmuxd(endpoint) => connection_kind(&endpoint.device.connection_type),
            Self::Wifi(_) => ConnKind::Network,
        }
    }
}

fn pairing_failure(error: IdeviceError) -> PairDeviceResult {
    let outcome = match error {
        IdeviceError::UserDeniedPairing => PairDeviceOutcome::Denied,
        IdeviceError::PasswordProtected | IdeviceError::DeviceLocked => PairDeviceOutcome::Locked,
        _ => PairDeviceOutcome::Failed,
    };
    PairDeviceResult {
        outcome,
        error: Some(error.to_string()),
    }
}

async fn pair_usb_endpoint(endpoint: &UsbmuxdEndpoint) -> PairDeviceResult {
    if !matches!(endpoint.device.connection_type, Connection::Usb) {
        return PairDeviceResult {
            outcome: PairDeviceOutcome::Failed,
            error: Some("pairing is available only for a USB device".into()),
        };
    }

    let device_id = crate::diagnostics::device_id_fingerprint(&endpoint.device.udid);
    tracing::info!(%device_id, "USB pairing requested by user");
    let operation = async {
        let mut usbmuxd = endpoint.address.connect(0).await?;
        let system_buid = usbmuxd.get_buid().await?;
        let provider = endpoint
            .device
            .to_provider(endpoint.address.clone(), "devicehub-mask-pairing");
        let mut lockdown = LockdownClient::connect(&provider).await?;
        let host_id = uuid::Uuid::new_v4().to_string().to_uppercase();
        let mut pairing_file = lockdown
            .pair(host_id, system_buid, Some("DeviceHub Mask"))
            .await?;

        // Do not persist credentials until the device accepts them and a secure
        // Lockdown session proves the generated record is usable.
        lockdown.start_session(&pairing_file).await?;
        pairing_file.udid = Some(endpoint.device.udid.clone());
        let serialized = pairing_file.serialize()?;
        usbmuxd
            .save_pair_record(&endpoint.device.udid, serialized)
            .await?;
        Ok::<(), IdeviceError>(())
    };

    match tokio::time::timeout(PAIRING_TIMEOUT, operation).await {
        Ok(Ok(())) => {
            tracing::info!(%device_id, "USB pairing completed");
            PairDeviceResult {
                outcome: PairDeviceOutcome::Paired,
                error: None,
            }
        }
        Ok(Err(error)) => {
            tracing::warn!(%device_id, ?error, "USB pairing failed");
            pairing_failure(error)
        }
        Err(_) => {
            tracing::warn!(%device_id, timeout_ms = PAIRING_TIMEOUT.as_millis(), "USB pairing timed out");
            PairDeviceResult {
                outcome: PairDeviceOutcome::TimedOut,
                error: Some("timed out waiting for the device trust confirmation".into()),
            }
        }
    }
}

async fn execute_pair_command(
    selection_id: String,
    reply: tokio::sync::oneshot::Sender<PairDeviceResult>,
    endpoints: &HashMap<String, SessionEndpoint>,
    status: &StatusSlot,
) -> bool {
    let result = match endpoints.get(&selection_id) {
        Some(SessionEndpoint::Usbmuxd(endpoint)) => {
            status.set("waiting for device trust confirmation...");
            pair_usb_endpoint(endpoint).await
        }
        Some(SessionEndpoint::Wifi(_)) => PairDeviceResult {
            outcome: PairDeviceOutcome::Failed,
            error: Some("pairing is available only for a USB device".into()),
        },
        None => PairDeviceResult {
            outcome: PairDeviceOutcome::Failed,
            error: Some("the selected USB device is no longer available".into()),
        },
    };
    let paired = result.outcome == PairDeviceOutcome::Paired;
    let _ = reply.send(result);
    paired
}

fn forget_device_result(
    device_error: Option<String>,
    host_error: Option<String>,
) -> ForgetDeviceResult {
    let outcome = match (device_error.is_some(), host_error.is_some()) {
        (false, false) => ForgetDeviceOutcome::Forgotten,
        (true, false) => ForgetDeviceOutcome::HostRecordRemoved,
        (false, true) => ForgetDeviceOutcome::DeviceForgottenHostCleanupFailed,
        (true, true) => ForgetDeviceOutcome::Failed,
    };
    let error = match (device_error, host_error) {
        (Some(device), Some(host)) => Some(format!(
            "device did not confirm revocation: {device}; host record cleanup failed: {host}"
        )),
        (Some(device), None) => Some(format!("device did not confirm revocation: {device}")),
        (None, Some(host)) => Some(format!("host record cleanup failed: {host}")),
        (None, None) => None,
    };
    ForgetDeviceResult { outcome, error }
}

async fn delete_host_pair_record(endpoint: &UsbmuxdEndpoint) -> Result<(), IdeviceError> {
    let delete = async {
        let mut usbmuxd = endpoint.address.connect(0).await?;
        usbmuxd.delete_pair_record(&endpoint.device.udid).await
    };
    match tokio::time::timeout(FORGET_DEVICE_TIMEOUT, delete).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(first_error)) => {
            tracing::debug!(?first_error, "retrying host pairing record removal");
            let retry = async {
                let mut usbmuxd = endpoint.address.connect(0).await?;
                usbmuxd.delete_pair_record(&endpoint.device.udid).await
            };
            tokio::time::timeout(FORGET_DEVICE_TIMEOUT, retry)
                .await
                .map_err(|_| IdeviceError::Timeout)?
        }
        Err(_) => Err(IdeviceError::Timeout),
    }
}

async fn forget_usb_endpoint(
    endpoint: &UsbmuxdEndpoint,
    wifi: Option<&mut WifiDiscovery>,
    pairing_dir: &Path,
) -> ForgetDeviceResult {
    if !matches!(endpoint.device.connection_type, Connection::Usb) {
        return ForgetDeviceResult {
            outcome: ForgetDeviceOutcome::Failed,
            error: Some("removing trust is available only for a USB device".into()),
        };
    }

    let device_id = crate::diagnostics::device_id_fingerprint(&endpoint.device.udid);
    tracing::info!(%device_id, "USB trust removal requested by user");
    let pairing_record = tokio::time::timeout(FORGET_DEVICE_TIMEOUT, async {
        let mut usbmuxd = endpoint.address.connect(0).await?;
        usbmuxd.get_pair_record(&endpoint.device.udid).await
    })
    .await
    .map_err(|_| IdeviceError::Timeout)
    .and_then(|result| result);

    let device_error = match pairing_record {
        Ok(pairing_file) => {
            let revoke = async {
                let provider = endpoint
                    .device
                    .to_provider(endpoint.address.clone(), "devicehub-mask-unpairing");
                let mut lockdown = LockdownClient::connect(&provider).await?;
                lockdown.unpair(pairing_file.host_id).await
            };
            match tokio::time::timeout(FORGET_DEVICE_TIMEOUT, revoke).await {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(error.to_string()),
                Err(_) => Some(IdeviceError::Timeout.to_string()),
            }
        }
        Err(error) => Some(error.to_string()),
    };

    // Always remove the local private-key record after an explicit forget
    // request, even when the device response was lost or already revoked.
    let pair_record_error = delete_host_pair_record(endpoint)
        .await
        .err()
        .map(|error| error.to_string());
    let cache_error = match wifi {
        Some(discovery) => discovery.remove_pairing(&endpoint.device.udid),
        None => crate::wifi_devices::remove_cached_pairing(pairing_dir, &endpoint.device.udid),
    }
    .err();
    let host_error = match (pair_record_error, cache_error) {
        (Some(pair_record), Some(cache)) => Some(format!(
            "usbmuxd record removal failed: {pair_record}; cached record removal failed: {cache}"
        )),
        (Some(pair_record), None) => Some(format!("usbmuxd record removal failed: {pair_record}")),
        (None, Some(cache)) => Some(format!("cached record removal failed: {cache}")),
        (None, None) => None,
    };
    let result = forget_device_result(device_error, host_error);
    if result.outcome == ForgetDeviceOutcome::Forgotten {
        tracing::info!(%device_id, "USB trust relationship removed");
    } else {
        tracing::warn!(%device_id, outcome = ?result.outcome, error = ?result.error, "USB trust removal completed with an incomplete result");
    }
    result
}

async fn execute_forget_command(
    selection_id: String,
    reply: tokio::sync::oneshot::Sender<ForgetDeviceResult>,
    endpoints: &HashMap<String, SessionEndpoint>,
    status: &StatusSlot,
    wifi: &mut Option<WifiDiscovery>,
    pairing_dir: &Path,
) {
    let result = match endpoints.get(&selection_id) {
        Some(SessionEndpoint::Usbmuxd(endpoint)) => {
            status.set("removing device trust...");
            forget_usb_endpoint(endpoint, wifi.as_mut(), pairing_dir).await
        }
        Some(SessionEndpoint::Wifi(_)) => ForgetDeviceResult {
            outcome: ForgetDeviceOutcome::Failed,
            error: Some("removing trust is available only for a USB device".into()),
        },
        None => ForgetDeviceResult {
            outcome: ForgetDeviceOutcome::Failed,
            error: Some("the selected USB device is no longer available".into()),
        },
    };
    let _ = reply.send(result);
}

/// Supervise the device session: enumerate attached devices for the picker,
/// connect to one, and tear down / reconnect when the selection changes.
#[allow(clippy::too_many_arguments)]
pub async fn manage(
    initial_udid: Option<String>,
    pairing_dir: PathBuf,
    resource_dir: Option<PathBuf>,
    settings: Arc<crate::settings::AppSettings>,
    video_counters: VideoCounters,
    repaint: impl Fn() + Send + Clone + 'static,
    frames: FrameSlot,
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
    // Cache of UDID -> DeviceName so a refresh doesn't re-query lockdown.
    let mut names: HashMap<String, String> = HashMap::new();
    let mut netmuxd = crate::netmuxd::NetmuxdSupervisor::new(pairing_dir.clone(), resource_dir);
    let prefer_netmuxd = netmuxd.is_forced();
    let mut wifi = start_wifi_discovery(&pairing_dir);
    // Auto-pick the first device only when no UDID was given, and only until we've
    // connected once: after a session ends we drop to idle rather than hot-loop.
    let mut auto_pick = initial_udid.is_none();
    let mut target = initial_udid;

    loop {
        let (devices, endpoints) = enumerate_devices(
            &mut names,
            &mut netmuxd,
            &mut wifi,
            &pairing_dir,
            prefer_netmuxd,
        )
        .await;
        device_list.set(devices);
        let wifi_setup_required = wifi
            .as_ref()
            .is_some_and(|discovery| discovery.requires_pairing());

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
                    Some(ControlCmd::Connect(u) | ControlCmd::Reconnect(u)) => target = Some(u),
                    Some(ControlCmd::Refresh) => names.clear(),
                    Some(ControlCmd::Pair { selection_id, reply }) => {
                        let requested = selection_id.clone();
                        if execute_pair_command(selection_id, reply, &endpoints, &status).await {
                            target = Some(requested);
                        }
                        names.clear();
                    }
                    Some(ControlCmd::Forget { selection_id, reply }) => {
                        execute_forget_command(
                            selection_id,
                            reply,
                            &endpoints,
                            &status,
                            &mut wifi,
                            &pairing_dir,
                        ).await;
                        names.clear();
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
                    Some(ControlCmd::Connect(u) | ControlCmd::Reconnect(u)) => target = Some(u),
                    Some(ControlCmd::Refresh) => names.clear(),
                    Some(ControlCmd::Pair { selection_id, reply }) => {
                        let requested = selection_id.clone();
                        if execute_pair_command(selection_id, reply, &endpoints, &status).await {
                            target = Some(requested);
                        }
                        names.clear();
                    }
                    Some(ControlCmd::Forget { selection_id, reply }) => {
                        execute_forget_command(
                            selection_id,
                            reply,
                            &endpoints,
                            &status,
                            &mut wifi,
                            &pairing_dir,
                        ).await;
                        target = None;
                        names.clear();
                    }
                    Some(ControlCmd::Quit) | None => return,
                },
                _ = tokio::time::sleep(IDLE_RESCAN) => {}
            }
            continue;
        };
        let udid = endpoint.udid().to_owned();
        let connection = endpoint.connection();

        // Per-session input channel, published so the UI's input reaches it.
        let (in_tx, in_rx) = tokio::sync::mpsc::unbounded_channel();
        input_sink.set(Some(in_tx.clone()));
        active.set_selected(udid.clone(), selection_id.clone());
        error.set(None);

        let session = run(
            endpoint,
            pairing_dir.clone(),
            SessionVideo {
                frame_format: settings.video_pixel_format(),
                decoder_backend: settings.video_decoder_backend(),
                counters: video_counters.clone(),
                frames: frames.clone(),
                browser_frames: browser_frames.clone(),
                audio_enabled: settings.audio_enabled(),
                clipboard_sync_enabled: settings.clipboard_sync_enabled(),
                audio: audio.clone(),
            },
            repaint.clone(),
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
        let mut active_rescan = tokio::time::interval(IDLE_RESCAN);
        active_rescan.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Consume the immediate first tick; the initial list was just populated.
        active_rescan.tick().await;

        // Run until the session ends on its own or the UI redirects us.
        let outcome = loop {
            tokio::select! {
                res = &mut session => {
                    match res {
                        Ok(()) => break Next::Idle,
                        Err(e) => {
                            tracing::error!(connection = connection.label(), "session ended: {e}");
                            error.set(Some(e));
                            if connection == ConnKind::Network {
                                break Next::RetryWifi(selection_id.clone());
                            }
                            break Next::Idle;
                        }
                    }
                }
                cmd = control_rx.recv() => match cmd {
                    Some(ControlCmd::Connect(u)) if u != selection_id && u != udid => break Next::Switch(u),
                    Some(ControlCmd::Connect(_)) => {} // already on this device
                    Some(ControlCmd::Reconnect(u)) => break Next::Switch(u),
                    Some(ControlCmd::Refresh) => {
                        names.clear();
                        let (devices, _) = enumerate_devices(&mut names, &mut netmuxd, &mut wifi, &pairing_dir, prefer_netmuxd).await;
                        device_list.set(devices);
                    }
                    Some(ControlCmd::Pair { selection_id, reply }) => break Next::Pair { selection_id, reply },
                    Some(ControlCmd::Forget { selection_id, reply }) => break Next::Forget { selection_id, reply },
                    Some(ControlCmd::Quit) | None => break Next::Quit,
                },
                _ = active_rescan.tick() => {
                    let (devices, _) = enumerate_devices(&mut names, &mut netmuxd, &mut wifi, &pairing_dir, prefer_netmuxd).await;
                    device_list.set(devices);
                }
            }
        };

        // For user-initiated transitions the session is still live: stop it and
        // wait for teardown so two sessions never fight over the same media stream.
        if matches!(
            outcome,
            Next::Switch(_) | Next::Pair { .. } | Next::Forget { .. } | Next::Quit
        ) {
            let _ = in_tx.send(InputCmd::Shutdown);
            let _ = tokio::time::timeout(SWITCH_GRACE, &mut session).await;
        }
        input_sink.set(None);
        active.set(None);
        location.set(LocationStatus::default());

        match outcome {
            Next::Switch(u) => target = Some(u),
            Next::RetryWifi(u) => {
                tracing::info!(
                    retry_ms = WIFI_RECONNECT_DELAY.as_millis(),
                    "Wi-Fi session transport dropped; rebuilding the complete tunnel"
                );
                target = Some(u);
                tokio::time::sleep(WIFI_RECONNECT_DELAY).await;
            }
            Next::Pair {
                selection_id,
                reply,
            } => {
                let requested = selection_id.clone();
                target = execute_pair_command(selection_id, reply, &endpoints, &status)
                    .await
                    .then_some(requested);
                names.clear();
            }
            Next::Forget {
                selection_id,
                reply,
            } => {
                execute_forget_command(
                    selection_id,
                    reply,
                    &endpoints,
                    &status,
                    &mut wifi,
                    &pairing_dir,
                )
                .await;
                target = None;
                names.clear();
            }
            Next::Idle => target = None,
            Next::Quit => return,
        }
    }
}

/// Enumerate the devices usbmuxd currently knows about, resolving (and caching)
/// each one's `DeviceName`. Best-effort: any failure yields an empty list rather
/// than erroring, and an un-nameable device falls back to its UDID.
async fn enumerate_devices(
    names: &mut HashMap<String, String>,
    netmuxd: &mut crate::netmuxd::NetmuxdSupervisor,
    wifi: &mut Option<WifiDiscovery>,
    pairing_dir: &Path,
    prefer_netmuxd: bool,
) -> (Vec<DeviceInfo>, HashMap<String, SessionEndpoint>) {
    let netmuxd_addr = if prefer_netmuxd || wifi.is_none() {
        netmuxd.ensure_ready().await
    } else {
        None
    };
    if wifi.is_none() {
        *wifi = start_wifi_discovery(pairing_dir);
    }
    let system_addr = UsbmuxdAddr::from_env_var().map_err(|error| {
        tracing::warn!(?error, "invalid usbmuxd address; USB discovery disabled");
    });
    let mut candidates = Vec::new();
    if let Some(address) = netmuxd_addr.clone() {
        candidates.push((address, true));
    }
    if let Ok(address) = system_addr {
        candidates.push((address, false));
    }
    let mut selected_mux = None;
    for (address, is_netmuxd) in candidates {
        match address.connect(0).await {
            Ok(mut connection) => match connection.get_devices().await {
                Ok(devices) => {
                    selected_mux = Some((address, connection, devices, is_netmuxd));
                    break;
                }
                Err(error) => tracing::warn!(
                    ?error,
                    is_netmuxd,
                    "unable to list usbmuxd devices; trying transport fallback"
                ),
            },
            Err(error) => tracing::warn!(
                ?error,
                is_netmuxd,
                "unable to connect to usbmuxd; trying transport fallback"
            ),
        }
    }
    let (addr, mut usbmuxd, devs, using_netmuxd) = match selected_mux {
        Some(selected) => (Some(selected.0), Some(selected.1), selected.2, selected.3),
        None => (None, None, Vec::new(), false),
    };

    let mut pairing_states = HashMap::new();
    if let Some(usbmuxd) = usbmuxd.as_mut() {
        for device in devs
            .iter()
            .filter(|device| matches!(device.connection_type, Connection::Usb))
        {
            match usbmuxd.get_pair_record(&device.udid).await {
                Ok(pairing_file) => {
                    pairing_states.insert(device.udid.clone(), DevicePairingState::Paired);
                    if let Some(discovery) = wifi.as_mut()
                        && discovery.pairing_needs_refresh(&device.udid)
                    {
                        if let Err(error) = discovery.cache_pairing(&device.udid, pairing_file) {
                            tracing::warn!(
                                device_id = %crate::diagnostics::device_id_fingerprint(&device.udid),
                                %error,
                                "unable to cache pairing record for Wi-Fi discovery"
                            );
                        } else {
                            discovery.mark_pairing_refreshed(&device.udid);
                        }
                    }
                }
                Err(error) => {
                    pairing_states.insert(device.udid.clone(), DevicePairingState::Unpaired);
                    tracing::debug!(
                        device_id = %crate::diagnostics::device_id_fingerprint(&device.udid),
                        ?error,
                        "USB pairing record unavailable"
                    );
                }
            }
        }
    }

    // A network device exposed through usbmuxd/netmuxd is a Lockdown transport,
    // not a USB CoreDevice proxy. Routing it through `connect_usb_core_tunnel`
    // makes the device close the TLS stream during CoreDeviceProxy negotiation.
    // Wi-Fi control is always represented by the RemotePairing endpoint below.
    let mut selected = Vec::with_capacity(devs.len());
    for device in devs {
        if uses_usbmuxd_core_proxy(&device.connection_type) {
            selected.push(device);
        } else if let Connection::Unknown(connection_type) = &device.connection_type {
            tracing::warn!(
                device_id = %crate::diagnostics::device_id_fingerprint(&device.udid),
                %connection_type,
                "ignoring usbmuxd device with an ambiguous transport"
            );
        }
    }
    selected.sort_by(|a, b| {
        a.udid.cmp(&b.udid).then_with(|| {
            connection_priority(&a.connection_type).cmp(&connection_priority(&b.connection_type))
        })
    });

    let mut out = Vec::with_capacity(selected.len());
    let mut endpoints = HashMap::new();
    for dev in selected {
        let connection = connection_kind(&dev.connection_type);
        let id = device_selector(&dev.udid, connection);
        let name = match names.get(&dev.udid) {
            Some(n) => n.clone(),
            None => {
                let n = match &addr {
                    Some(addr) => fetch_device_name(&dev, addr).await,
                    None => None,
                }
                .unwrap_or_else(|| dev.udid.clone());
                names.insert(dev.udid.clone(), n.clone());
                n
            }
        };
        out.push(DeviceInfo {
            id: id.clone(),
            udid: dev.udid.clone(),
            name,
            connection,
            pairing: pairing_states.get(&dev.udid).copied().unwrap_or_default(),
        });
        if let Some(address) = addr.clone() {
            endpoints
                .entry(id)
                .or_insert(SessionEndpoint::Usbmuxd(Box::new(UsbmuxdEndpoint {
                    device: dev,
                    address,
                })));
        }
    }

    if let Some(discovery) = wifi.as_mut() {
        for endpoint in discovery.refresh() {
            let id = device_selector(&endpoint.udid, ConnKind::Network);
            if endpoints.contains_key(&id) {
                continue;
            }
            let provider = wifi_provider(&endpoint);
            let name = match names.get(&endpoint.udid) {
                Some(name) => name.clone(),
                None => {
                    let name = fetch_device_name_from_provider(&provider)
                        .await
                        .unwrap_or_else(|| endpoint.udid.clone());
                    names.insert(endpoint.udid.clone(), name.clone());
                    name
                }
            };
            out.push(DeviceInfo {
                id: id.clone(),
                udid: endpoint.udid.clone(),
                name,
                connection: ConnKind::Network,
                pairing: DevicePairingState::NotApplicable,
            });
            endpoints.insert(id, SessionEndpoint::Wifi(Box::new(endpoint)));
        }
    }
    let usb_count = out
        .iter()
        .filter(|device| device.connection == ConnKind::Usb)
        .count();
    let wifi_count = out
        .iter()
        .filter(|device| device.connection == ConnKind::Network)
        .count();
    tracing::debug!(
        provider = if using_netmuxd {
            "netmuxd"
        } else {
            "system-usbmuxd"
        },
        usb_count,
        wifi_count,
        "device discovery refresh completed"
    );
    out.sort_by(|a, b| {
        a.udid.cmp(&b.udid).then_with(|| {
            connection_kind_priority(a.connection).cmp(&connection_kind_priority(b.connection))
        })
    });
    (out, endpoints)
}

fn start_wifi_discovery(pairing_dir: &Path) -> Option<WifiDiscovery> {
    match WifiDiscovery::start(pairing_dir.to_owned()) {
        Ok(discovery) => Some(discovery),
        Err(error) => {
            tracing::warn!(%error, "Wi-Fi discovery unavailable; continuing with usbmuxd");
            None
        }
    }
}

/// Resolve a device's `DeviceName` over lockdown, with a timeout. Returns `None`
/// (caller falls back to the UDID) if the device can't be reached or named.
async fn fetch_device_name(dev: &UsbmuxdDevice, addr: &UsbmuxdAddr) -> Option<String> {
    let provider = dev.to_provider(addr.clone(), "devicehub_rs");
    fetch_device_name_from_provider(&provider).await
}

async fn fetch_device_name_from_provider(provider: &dyn IdeviceProvider) -> Option<String> {
    let lookup = async {
        let mut lockdown = LockdownClient::connect(provider).await.ok()?;
        let value = lockdown.get_value(Some("DeviceName"), None).await.ok()?;
        value.as_string().map(|s| s.to_string())
    };
    tokio::time::timeout(NAME_TIMEOUT, lookup)
        .await
        .ok()
        .flatten()
}

async fn connect_core_tunnel(
    endpoint: &SessionEndpoint,
    provider: &dyn IdeviceProvider,
    pairing_dir: &Path,
    status: &StatusSlot,
) -> Result<(AdapterHandle, RsdHandshake), String> {
    match endpoint {
        SessionEndpoint::Usbmuxd(_) => connect_usb_core_tunnel(provider).await,
        SessionEndpoint::Wifi(endpoint) => {
            connect_wifi_core_tunnel(endpoint, pairing_dir, status).await
        }
    }
}

async fn connect_usb_core_tunnel(
    provider: &dyn IdeviceProvider,
) -> Result<(AdapterHandle, RsdHandshake), String> {
    let proxy = CoreDeviceProxy::connect(provider)
        .await
        .map_err(|error| format!("no core device proxy: {error:?}"))?;
    let rsd_port = proxy.tunnel_info().server_rsd_port;
    let adapter = proxy
        .create_software_tunnel()
        .map_err(|error| format!("no software tunnel: {error:?}"))?;
    let mut adapter = adapter.to_async_handle();
    let stream = adapter
        .connect(rsd_port)
        .await
        .map_err(|error| format!("RSD connect failed: {error:?}"))?;
    let handshake = RsdHandshake::new(stream)
        .await
        .map_err(|error| format!("RSD handshake failed: {error:?}"))?;
    Ok((adapter, handshake))
}

async fn connect_wifi_core_tunnel(
    endpoint: &WifiEndpoint,
    pairing_dir: &Path,
    status: &StatusSlot,
) -> Result<(AdapterHandle, RsdHandshake), String> {
    let pairing_path = remote_pairing_path(pairing_dir, &endpoint.udid)?;
    let mut pairing_file = match RpPairingFile::read_from_file(&pairing_path).await {
        Ok(pairing_file) => pairing_file,
        Err(_) => {
            status.set("unlock the device and approve Wi-Fi control...");
            tracing::info!(
                device_id = %crate::diagnostics::device_id_fingerprint(&endpoint.udid),
                "remote pairing credentials missing; authorizing over USB"
            );
            tokio::time::timeout(
                Duration::from_secs(120),
                pair_remote_via_usb(&endpoint.udid, &pairing_path),
            )
            .await
            .map_err(|_| {
                "initial Wi-Fi authorization timed out; unlock the device and accept its trust prompt"
                    .to_string()
            })??
        }
    };
    status.set("verifying Wi-Fi control authorization...");
    let address = scoped_socket_addr(
        endpoint.remote_pairing_address,
        endpoint.remote_pairing_scope_id,
        endpoint.remote_pairing_port,
    );
    let stream = tokio::time::timeout(NAME_TIMEOUT, tokio::net::TcpStream::connect(address))
        .await
        .map_err(|_| "remote pairing connection timed out".to_string())?
        .map_err(|error| format!("remote pairing connection failed: {error}"))?;
    let socket = RpPairingSocket::new(stream);
    let mut client = RemotePairingClient::new(socket, "devicehub-mask");
    client
        .connect(&mut pairing_file, async || "000000".to_string())
        .await
        .map_err(|error| format!("remote pairing verification failed: {error:?}"))?;

    let tunnel_port = client
        .create_tcp_listener()
        .await
        .map_err(|error| format!("remote tunnel listener failed: {error:?}"))?;
    status.set("establishing secure Wi-Fi tunnel...");
    let tunnel_address = scoped_socket_addr(
        endpoint.remote_pairing_address,
        endpoint.remote_pairing_scope_id,
        tunnel_port,
    );
    let tunnel_stream =
        tokio::time::timeout(NAME_TIMEOUT, tokio::net::TcpStream::connect(tunnel_address))
            .await
            .map_err(|_| "remote tunnel connection timed out".to_string())?
            .map_err(|error| format!("remote tunnel connection failed: {error}"))?;
    let tunnel = connect_tls_psk_tunnel_native(tunnel_stream, client.encryption_key())
        .await
        .map_err(|error| format!("remote TLS-PSK tunnel failed: {error:?}"))?;
    let client_ip = tunnel
        .info
        .client_address
        .parse()
        .map_err(|error| format!("invalid remote tunnel client address: {error}"))?;
    let server_ip = tunnel
        .info
        .server_address
        .parse()
        .map_err(|error| format!("invalid remote tunnel server address: {error}"))?;
    let rsd_port = tunnel.info.server_rsd_port;
    let mtu = tunnel.info.mtu as usize;
    let mut adapter =
        idevice::tcp::adapter::Adapter::new(Box::new(tunnel.into_inner()), client_ip, server_ip);
    adapter.set_mss(mtu.saturating_sub(60));
    let mut adapter = adapter.to_async_handle();
    let rsd_stream = adapter
        .connect(rsd_port)
        .await
        .map_err(|error| format!("remote RSD connect failed: {error:?}"))?;
    let handshake = RsdHandshake::new(rsd_stream)
        .await
        .map_err(|error| format!("remote RSD handshake failed: {error:?}"))?;
    tracing::info!(
        device_id = %crate::diagnostics::device_id_fingerprint(&endpoint.udid),
        "remote pairing CoreDevice tunnel established"
    );
    Ok((adapter, handshake))
}

async fn pair_remote_via_usb(udid: &str, path: &Path) -> Result<RpPairingFile, String> {
    let address = UsbmuxdAddr::from_env_var()
        .map_err(|error| format!("USB transport unavailable for remote pairing: {error:?}"))?;
    let mut mux = address
        .connect(0)
        .await
        .map_err(|error| format!("USB connection required for initial Wi-Fi pairing: {error:?}"))?;
    let device = mux
        .get_devices()
        .await
        .map_err(|error| format!("cannot list USB devices for remote pairing: {error:?}"))?
        .into_iter()
        .find(|device| device.udid == udid && matches!(device.connection_type, Connection::Usb))
        .ok_or_else(|| "connect this device by USB once to authorize Wi-Fi control".to_string())?;
    let provider = device.to_provider(address, "devicehub-mask-remote-pairing");
    tracing::debug!(
        device_id = %crate::diagnostics::device_id_fingerprint(udid),
        "opening USB CoreDevice tunnel for remote pairing"
    );
    let (mut adapter, handshake) = connect_usb_core_tunnel(&provider).await?;
    let service = handshake
        .services
        .get("com.apple.internal.dt.coredevice.untrusted.tunnelservice")
        .ok_or_else(|| "device does not expose the remote pairing service".to_string())?;
    let stream = adapter
        .connect(service.port)
        .await
        .map_err(|error| format!("remote pairing service connect failed: {error:?}"))?;
    let mut connection = RemoteXpcClient::new(stream)
        .await
        .map_err(|error| format!("remote pairing XPC connection failed: {error:?}"))?;
    connection
        .do_handshake()
        .await
        .map_err(|error| format!("remote pairing XPC handshake failed: {error:?}"))?;
    connection
        .recv_root()
        .await
        .map_err(|error| format!("remote pairing XPC root failed: {error:?}"))?;
    tracing::info!(
        device_id = %crate::diagnostics::device_id_fingerprint(udid),
        "waiting for device to authorize remote pairing"
    );
    let mut pairing_file = RpPairingFile::generate("devicehub-mask");
    let mut client = RemotePairingClient::new(connection, "devicehub-mask");
    client
        .connect(&mut pairing_file, async || "000000".to_string())
        .await
        .map_err(|error| format!("USB remote pairing failed: {error:?}"))?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| format!("cannot create remote pairing directory: {error}"))?;
    }
    pairing_file
        .write_to_file(path)
        .await
        .map_err(|error| format!("cannot save remote pairing credentials: {error:?}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("cannot secure remote pairing credentials: {error}"))?;
    }
    tracing::info!(
        device_id = %crate::diagnostics::device_id_fingerprint(udid),
        "created remote pairing credentials over USB"
    );
    Ok(pairing_file)
}

fn remote_pairing_path(pairing_dir: &Path, udid: &str) -> Result<PathBuf, String> {
    if udid.is_empty()
        || !udid
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("device UDID contains unsupported characters".into());
    }
    let base = pairing_dir.parent().unwrap_or(pairing_dir);
    Ok(base.join("remote-pairings").join(format!("{udid}.plist")))
}

fn scoped_socket_addr(
    address: std::net::IpAddr,
    scope_id: Option<u32>,
    port: u16,
) -> std::net::SocketAddr {
    match address {
        std::net::IpAddr::V4(_) => std::net::SocketAddr::new(address, port),
        std::net::IpAddr::V6(address) => std::net::SocketAddr::V6(std::net::SocketAddrV6::new(
            address,
            port,
            0,
            scope_id.unwrap_or(0),
        )),
    }
}

/// Run the whole session to completion. Returns an error string suitable for the
/// status bar if setup fails; otherwise runs until a [`InputCmd::Shutdown`] (or
/// the UI dropping the input channel).
async fn run(
    endpoint: SessionEndpoint,
    pairing_dir: PathBuf,
    video: SessionVideo,
    repaint: impl Fn() + Send + 'static,
    clipboard: ClipboardSlot,
    views: SessionViews,
    mut input_rx: UnboundedReceiver<InputCmd>,
) -> Result<(), String> {
    views.status.set("connecting to device...");
    let requested_udid = endpoint.udid().to_owned();
    let (provider, connection) = connect_provider(endpoint.clone()).await?;
    let device_details = read_device_details(&*provider, requested_udid.clone()).await;
    if let Some(details) = &device_details {
        tracing::info!(
            product_type = %details.product_type,
            product_version = %details.product_version,
            "connected device identity"
        );
    }

    let installation_proxy = match InstallationProxyClient::connect(&*provider).await {
        Ok(client) => Some(client),
        Err(error) => {
            tracing::warn!("installation proxy unavailable; app list fallback disabled: {error:?}");
            None
        }
    };
    let (mut adapter, mut handshake) =
        connect_core_tunnel(&endpoint, &*provider, &pairing_dir, &views.status).await?;

    views.performance.reset();
    views.device_logs.reset();
    views.device_events.reset();
    let mut supervisor = supervisor::ServiceSupervisor::new(views.services.clone());
    supervisor.spawn(crate::heartbeat::supervise(
        provider.clone(),
        supervisor.reporter("device.heartbeat"),
        supervisor.shutdown_receiver(),
    ));
    supervisor.spawn(crate::device_logs::supervise(
        adapter.clone(),
        handshake.clone(),
        views.device_logs.clone(),
        supervisor.reporter("device.logs"),
        views.device_log_demand.subscribe(),
        supervisor.shutdown_receiver(),
    ));
    supervisor.spawn(crate::device_events::supervise(
        adapter.clone(),
        handshake.clone(),
        views.device_events.clone(),
        supervisor.reporter("device.notifications"),
        supervisor.shutdown_receiver(),
    ));
    supervisor.spawn(performance::supervise_system(
        adapter.clone(),
        handshake.clone(),
        views.performance.clone(),
        supervisor.reporter("performance.system"),
        views.performance_demand.subscribe(),
        supervisor.shutdown_receiver(),
    ));
    supervisor.spawn(performance::supervise_graphics(
        adapter.clone(),
        handshake.clone(),
        views.performance.clone(),
        supervisor.reporter("performance.graphics"),
        views.performance_demand.subscribe(),
        supervisor.shutdown_receiver(),
    ));
    supervisor.spawn(performance::supervise_network(
        adapter.clone(),
        handshake.clone(),
        views.performance.clone(),
        supervisor.reporter("performance.network"),
        views.performance_demand.subscribe(),
        supervisor.shutdown_receiver(),
    ));
    supervisor.spawn(performance::supervise_energy(
        adapter.clone(),
        handshake.clone(),
        views.performance.clone(),
        supervisor.reporter("performance.energy"),
        views.performance_demand.subscribe(),
        supervisor.shutdown_receiver(),
    ));
    supervisor.spawn(performance::supervise_app_activity(
        adapter.clone(),
        handshake.clone(),
        views.performance.clone(),
        supervisor.reporter("performance.app_activity"),
        views.performance_demand.subscribe(),
        supervisor.shutdown_receiver(),
    ));

    views.location.set(LocationStatus::default());
    let (location_sender, location_receiver) = tokio::sync::mpsc::channel(8);
    supervisor.spawn(location::supervise(
        adapter.clone(),
        handshake.clone(),
        provider.clone(),
        location_receiver,
        views.location.clone(),
        supervisor.reporter("location"),
        supervisor.shutdown_receiver(),
    ));
    let location = LocationBridge {
        sender: location_sender,
        status: views.location.clone(),
    };
    let (app_icon_sender, app_icon_receiver) = tokio::sync::mpsc::channel(16);
    supervisor.spawn(crate::app_icons::serve(
        adapter.clone(),
        handshake.clone(),
        app_icon_receiver,
        supervisor.shutdown_receiver(),
    ));
    let (companion_sender, companion_receiver) = tokio::sync::mpsc::channel(2);
    supervisor.spawn(crate::companion_devices::serve(
        adapter.clone(),
        handshake.clone(),
        companion_receiver,
        supervisor.reporter("device.companions"),
        supervisor.shutdown_receiver(),
    ));
    let (home_screen_sender, home_screen_receiver) = tokio::sync::mpsc::channel(2);
    supervisor.spawn(crate::home_screen::serve(
        adapter.clone(),
        handshake.clone(),
        home_screen_receiver,
        supervisor.reporter("device.home_screen"),
        supervisor.shutdown_receiver(),
    ));
    let (running_process_sender, running_process_receiver) = tokio::sync::mpsc::channel(2);
    supervisor.spawn(crate::running_processes::serve(
        adapter.clone(),
        handshake.clone(),
        running_process_receiver,
        supervisor.reporter("performance.process_inventory"),
        supervisor.shutdown_receiver(),
    ));
    let (app_lifecycle_sender, app_lifecycle_receiver) = tokio::sync::mpsc::channel(2);
    supervisor.spawn(crate::app_lifecycle::serve(
        adapter.clone(),
        handshake.clone(),
        app_lifecycle_receiver,
        supervisor.reporter("device.app_lifecycle"),
        supervisor.shutdown_receiver(),
    ));
    let (wda_sender, wda_receiver) = tokio::sync::mpsc::channel(4);
    supervisor.spawn(crate::wda_automation::serve(
        provider.clone(),
        wda_receiver,
        supervisor.reporter("device.wda"),
        supervisor.shutdown_receiver(),
    ));
    let (wda_runner_sender, wda_runner_receiver) = tokio::sync::mpsc::channel(2);
    supervisor.spawn(crate::wda_runner::serve(
        provider.clone(),
        wda_runner_receiver,
        supervisor.reporter("device.wda_runner"),
        supervisor.shutdown_receiver(),
    ));
    let (app_console_sender, app_console_receiver) = tokio::sync::mpsc::channel(4);
    supervisor.spawn(crate::app_console::serve(
        adapter.clone(),
        handshake.clone(),
        app_console_receiver,
        supervisor.reporter("device.app_console"),
        supervisor.shutdown_receiver(),
    ));
    let (app_documents_sender, app_documents_receiver) = tokio::sync::mpsc::channel(8);
    supervisor.spawn(crate::app_documents::serve(
        crate::app_documents::AppStorageTransport::new(
            provider.clone(),
            connection,
            adapter.clone(),
            handshake.clone(),
        ),
        app_documents_receiver,
        views.app_document_activity.clone(),
        supervisor.shutdown_receiver(),
    ));
    let (device_files_sender, device_files_receiver) = tokio::sync::mpsc::channel(8);
    supervisor.spawn(crate::device_files::serve(
        crate::device_files::DeviceFileTransport::new(
            provider.clone(),
            connection,
            adapter.clone(),
            handshake.clone(),
        ),
        device_files_receiver,
        views.device_file_activity.clone(),
        supervisor.reporter("device.files"),
        supervisor.shutdown_receiver(),
    ));
    let (screen_capture_sender, screen_capture_receiver) = tokio::sync::mpsc::channel(1);
    supervisor.spawn(crate::screen_capture::serve(
        crate::screen_capture::ScreenCaptureTransport::new(
            provider.clone(),
            connection,
            adapter.clone(),
            handshake.clone(),
        ),
        screen_capture_receiver,
        supervisor.shutdown_receiver(),
    ));
    let (network_capture_sender, network_capture_receiver) = tokio::sync::mpsc::channel(4);
    supervisor.spawn(crate::network_capture::serve(
        crate::network_capture::NetworkCaptureTransport::new(
            provider.clone(),
            connection,
            adapter.clone(),
            handshake.clone(),
        ),
        network_capture_receiver,
        views.network_capture.clone(),
        supervisor.reporter("network.capture"),
        supervisor.shutdown_receiver(),
    ));
    let (bluetooth_capture_sender, bluetooth_capture_receiver) = tokio::sync::mpsc::channel(4);
    supervisor.spawn(crate::bluetooth_capture::serve(
        adapter.clone(),
        handshake.clone(),
        bluetooth_capture_receiver,
        views.bluetooth_capture.clone(),
        supervisor.reporter("bluetooth.capture"),
        supervisor.shutdown_receiver(),
    ));
    let (device_backup_sender, device_backup_receiver) = tokio::sync::mpsc::channel(4);
    supervisor.spawn(crate::device_backup::serve(
        crate::device_backup::DeviceBackupTransport::new(
            provider.clone(),
            connection,
            adapter.clone(),
            handshake.clone(),
            requested_udid,
        ),
        device_backup_receiver,
        views.device_backup.clone(),
        supervisor.reporter("device.backup"),
        supervisor.shutdown_receiver(),
    ));
    let (sysdiagnose_sender, sysdiagnose_receiver) = tokio::sync::mpsc::channel(4);
    supervisor.spawn(crate::sysdiagnose::serve(
        adapter.clone(),
        handshake.clone(),
        sysdiagnose_receiver,
        views.sysdiagnose.clone(),
        supervisor.reporter("device.sysdiagnose"),
        supervisor.shutdown_receiver(),
    ));
    let (log_archive_sender, log_archive_receiver) = tokio::sync::mpsc::channel(4);
    supervisor.spawn(crate::log_archive::serve(
        adapter.clone(),
        handshake.clone(),
        log_archive_receiver,
        views.log_archive.clone(),
        supervisor.reporter("device.log_archive"),
        supervisor.shutdown_receiver(),
    ));
    let (developer_image_sender, developer_image_receiver) = tokio::sync::mpsc::channel(4);
    supervisor.spawn(crate::developer_image::serve(
        provider.clone(),
        developer_image_receiver,
        views.developer_image.clone(),
        supervisor.reporter("device.developer_image"),
        supervisor.shutdown_receiver(),
    ));
    let (device_condition_sender, device_condition_receiver) = tokio::sync::mpsc::channel(4);
    supervisor.spawn(crate::device_conditions::supervise(
        adapter.clone(),
        handshake.clone(),
        device_condition_receiver,
        views.device_conditions.clone(),
        supervisor.reporter("device.conditions"),
        supervisor.shutdown_receiver(),
    ));
    let (provisioning_sender, provisioning_receiver) = tokio::sync::mpsc::channel(4);
    supervisor.spawn(crate::provisioning::supervise(
        adapter.clone(),
        handshake.clone(),
        provider.clone(),
        provisioning_receiver,
        supervisor.reporter("device.provisioning"),
        supervisor.shutdown_receiver(),
    ));
    let device_management_services = DeviceManagementServices {
        icons: app_icon_sender,
        companions: companion_sender,
        home_screen: home_screen_sender,
        running_processes: running_process_sender,
        app_lifecycle: app_lifecycle_sender,
        wda: wda_sender,
        wda_runner: wda_runner_sender,
        app_console: app_console_sender,
        documents: app_documents_sender,
        device_files: device_files_sender,
        screen_capture: screen_capture_sender,
        network_capture: network_capture_sender,
        bluetooth_capture: bluetooth_capture_sender,
        device_backup: device_backup_sender,
        sysdiagnose: sysdiagnose_sender,
        log_archive: log_archive_sender,
        developer_image: developer_image_sender,
        device_conditions: device_condition_sender,
        provisioning: provisioning_sender,
    };

    // Our RTCP SSRC. MUST be declared in the video offer (field 5.1) so the device
    // associates our RTCP feedback with the stream; otherwise it's ignored.
    let our_ssrc = uuid::Uuid::new_v4().as_u128() as u32;

    views.status.set("starting screen media stream...");
    let media = match start_screen_media_stream(
        &mut adapter,
        &mut handshake,
        our_ssrc,
        device_details.as_ref(),
        connection,
    )
    .await
    {
        Ok(media) => media,
        Err(error) => {
            tracing::warn!("screen control unavailable; keeping device management session alive");
            views.error.set(Some(error));
            views.status.set("device management connected");
            management_input_loop(
                DeviceManagement::fallback(
                    provider,
                    views.app_operation.clone(),
                    device_details,
                    installation_proxy,
                    AppServiceTransport {
                        adapter: adapter.clone(),
                        handshake: handshake.clone(),
                    },
                    device_management_services,
                ),
                &mut input_rx,
                &location,
            )
            .await;
            drop(location);
            supervisor.shutdown().await;
            views.status.set("stopping...");
            return Ok(());
        }
    };

    // HID surfaces only authenticate once the media stream is up; give backboardd
    // a moment to re-match them before connecting.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    views.status.set("connecting HID...");
    let mut touch = UniversalHidClient::connect_rsd(&mut adapter, &mut handshake)
        .await
        .map_err(|e| format!("no universalhidservice: {e:?}"))?;
    touch.dump_services_from_env().await;
    let mut indigo = IndigoHidClient::connect_rsd(&mut adapter, &mut handshake)
        .await
        .map_err(|e| format!("no hid.indigo: {e:?}"))?;

    // Clipboard access is opt-in because synchronization reads and replaces the
    // host and device clipboards. Run without it when disabled or unavailable.
    let pasteboard = if video.clipboard_sync_enabled {
        match PasteboardServiceClient::connect_rsd(&mut adapter, &mut handshake).await {
            Ok(client) => {
                tracing::info!("clipboard sync enabled for this device session");
                Some(client)
            }
            Err(error) => {
                tracing::warn!(?error, "no pasteboardservice; clipboard sync unavailable");
                None
            }
        }
    } else {
        tracing::info!("clipboard sync disabled for this device session");
        None
    };

    // Orientation control is best-effort too: run without rotate if unavailable.
    let mut orientation =
        match OrientationServiceClient::connect_rsd(&mut adapter, &mut handshake).await {
            Ok(c) => Some(c),
            Err(e) => {
                tracing::warn!("no orientation service; rotate disabled: {e:?}");
                None
            }
        };

    // The media stream always exposes a native portrait framebuffer, including
    // when a landscape-only game has rotated its content inside that frame.
    // SpringBoard provides the current interface orientation without changing it.
    let springboard = match SpringBoardServicesClient::connect_rsd(&mut adapter, &mut handshake)
        .await
    {
        Ok(mut client) => {
            if let Err(error) = refresh_interface_orientation(&mut client, &views.orientation).await
            {
                tracing::warn!("could not read initial device interface orientation: {error:?}");
            }
            Some(client)
        }
        Err(error) => {
            tracing::warn!(
                "no SpringBoard orientation service; using rotation command state: {error:?}"
            );
            None
        }
    };

    let app_service = match AppServiceClient::connect_rsd(&mut adapter, &mut handshake).await {
        Ok(client) => Some(client),
        Err(error) => {
            tracing::warn!("no CoreDevice AppService; app management disabled: {error:?}");
            None
        }
    };

    let frame_format = video.frame_format;
    let decoder_backend = video.decoder_backend;
    video.frames.reset();
    video.browser_frames.reset_dimensions();
    tracing::info!(?decoder_backend, "selected video decoder backend");

    views.status.set("connected");

    // A stable CNAME for our RTCP SDES (identifies this receiver endpoint).
    let cname = format!("devicehub@{}", adapter.host_ip());

    // Keep the display client to stop the stream on teardown.
    let mut display = media.client;

    // Shared between the RTP receive loop and the RTCP send loop (rtcp-mux feedback
    // goes back out the RTP socket).
    let video_udp = Arc::new(media.video_udp);
    let rtcp_udp = media.rtcp_udp.map(Arc::new);

    // Pulsed by the ffmpeg-stderr watcher and the stall watchdog; the RTCP send
    // loop reacts by requesting a fresh keyframe (PLI + FIR) on the same stream.
    let corruption = Arc::new(Notify::new());

    // Pulsed by the decode loop on every decoded frame; the stall watchdog watches
    // it to detect a silently wedged stream (no frames, no decode errors).
    let frame_beat = Arc::new(Notify::new());

    let rtcp = Arc::new(Mutex::new(RtcpShared::default()));

    // `udp.recv()` holds a non-Send MutexGuard across an await, so these loops
    // can't be spawned; we run them concurrently on this task via `select!`. The
    // input loop is the only one that returns normally (Shutdown / channel close);
    // when it does, the others drop, closing ffmpeg's stdin.
    //
    // Complete access units wait in a byte-bounded queue so ffmpeg backpressure
    // cannot stall RTP/RTCP or grow memory without limit.
    let hevc_queue = Arc::new(HevcQueue::new(HEVC_QUEUE_MAX_BYTES));
    let orientation_watch_view = views.orientation.clone();
    let orientation_task = async move {
        match springboard {
            Some(client) => watch_interface_orientation(client, orientation_watch_view).await,
            None => std::future::pending::<()>().await,
        }
    };
    let (clipboard_commands, clipboard_command_rx) = tokio::sync::mpsc::channel(4);
    let clipboard_bridge = ClipboardBridge(clipboard_commands);
    let decode_corruption = corruption.clone();
    let decode_frame_beat = frame_beat.clone();
    let decode_queue = hevc_queue.clone();
    let decode_counters = video.counters.clone();
    let browser_keyframes = video.browser_frames.clone();
    let browser_lifecycle = video.browser_frames.clone();
    let decode_pipeline = async move {
        match decoder_backend {
            crate::settings::VideoDecoderBackend::Native => {
                let (_child, ffmpeg_in, ffmpeg_out, ffmpeg_err) =
                    decode::spawn_ffmpeg(frame_format)
                        .map_err(|error| format!("failed to spawn ffmpeg: {error}"))?;
                tokio::select! {
                    _ = ffmpeg_writer(ffmpeg_in, decode_queue) => {
                        tracing::warn!("ffmpeg writer ended");
                    }
                    _ = decode::read_frames(
                        ffmpeg_out,
                        frame_format,
                        video.frames,
                        decode_counters,
                        decode_frame_beat,
                        repaint,
                    ) => {
                        tracing::warn!("decode task ended early");
                    }
                    _ = watch_decode_errors(ffmpeg_err, decode_corruption) => {
                        tracing::warn!("ffmpeg stderr watcher ended");
                    }
                }
            }
            crate::settings::VideoDecoderBackend::Browser => {
                browser_video_writer(
                    decode_queue,
                    video.browser_frames,
                    decode_counters,
                    decode_frame_beat,
                    decode_corruption,
                )
                .await;
            }
        }
        Ok::<(), String>(())
    };

    let management_app_adapter = adapter.clone();
    let management_app_handshake = handshake.clone();
    tokio::select! {
        _ = video_task(
            video_udp.clone(),
            hevc_queue.clone(),
            rtcp.clone(),
            corruption.clone(),
            video.counters.clone(),
            our_ssrc,
        ) => {
            tracing::warn!("video task ended early");
        }
        _ = audio_task(media.audio_udp, video.audio, video.audio_enabled) => {
            tracing::warn!("audio task ended early");
        }
        result = decode_pipeline => {
            if let Err(error) = result {
                tracing::warn!(%error, "video decoder pipeline ended");
            }
        }
        _ = stall_watchdog(frame_beat, &corruption) => {}
        _ = forward_browser_keyframes(browser_keyframes, corruption.clone()) => {}
        _ = rtcp_recv_task(rtcp_udp.clone(), rtcp.clone()) => {}
        _ = rtcp_send_task(
            video_udp, rtcp_udp, rtcp, our_ssrc, cname, &corruption,
        ) => {}
        _ = clipboard_task(
            pasteboard,
            video.clipboard_sync_enabled,
            clipboard,
            clipboard_command_rx,
            &mut adapter,
            &mut handshake,
        ) => {}
        _ = orientation_task => {}
        _ = input_loop(
            &mut touch,
            &mut indigo,
            &mut orientation,
            DeviceManagement::new(
                provider,
                views.app_operation.clone(),
                device_details,
                app_service,
                installation_proxy,
                AppServiceTransport {
                    adapter: management_app_adapter,
                    handshake: management_app_handshake,
                },
                device_management_services,
            ),
            &mut input_rx,
            InputBridges {
                orientation: &views.orientation,
                location: &location,
                clipboard: &clipboard_bridge,
            },
        ) => {}
    }

    drop(location);
    supervisor.shutdown().await;
    browser_lifecycle.reset_dimensions();
    views.status.set("stopping...");
    display.stop_media_stream().await.ok();
    // `proxy`, `adapter`, `handshake` drop here, tearing down the tunnel.
    Ok(())
}

/// Dispatch input until the UI shuts us down or the channel closes.
struct InputBridges<'a> {
    orientation: &'a OrientationSlot,
    location: &'a LocationBridge,
    clipboard: &'a ClipboardBridge,
}

async fn input_loop(
    touch: &mut UniversalHidClient<Box<dyn ReadWrite>>,
    indigo: &mut IndigoHidClient<Box<dyn ReadWrite>>,
    orientation: &mut Option<OrientationServiceClient<Box<dyn ReadWrite>>>,
    mut management: DeviceManagement,
    input_rx: &mut UnboundedReceiver<InputCmd>,
    bridges: InputBridges<'_>,
) {
    while let Some(cmd) = input_rx.recv().await {
        if matches!(cmd, InputCmd::Shutdown) {
            break;
        }
        let Some(cmd) = management.handle(cmd).await else {
            continue;
        };
        let Some(cmd) = forward_location_command(cmd, bridges.location) else {
            continue;
        };
        if let InputCmd::PasteText { text, reply } = cmd {
            let result = async {
                bridges.clipboard.set_text(text).await?;
                type_key(
                    indigo,
                    KEY_V,
                    KeyMods {
                        cmd: true,
                        ..KeyMods::default()
                    },
                )
                .await
                .map_err(|error| format!("unable to send paste shortcut: {error:?}"))
            }
            .await;
            let _ = reply.send(result);
            continue;
        }
        if let Err(e) = dispatch(touch, indigo, orientation, bridges.orientation, cmd).await {
            tracing::warn!("input dispatch failed: {e:?}");
        }
    }
}

async fn management_input_loop(
    mut management: DeviceManagement,
    input_rx: &mut UnboundedReceiver<InputCmd>,
    location: &LocationBridge,
) {
    while let Some(command) = input_rx.recv().await {
        if matches!(command, InputCmd::Shutdown) {
            break;
        }
        let Some(command) = management.handle(command).await else {
            continue;
        };
        if let InputCmd::PasteText { reply, .. } = command {
            let _ = reply.send(Err("device control is unavailable".into()));
            continue;
        }
        let _ = forward_location_command(command, location);
    }
}

fn forward_location_command(command: InputCmd, location: &LocationBridge) -> Option<InputCmd> {
    let command = match command {
        InputCmd::SetLocation {
            latitude,
            longitude,
            reply,
        } => LocationCommand::Set {
            latitude,
            longitude,
            reply,
        },
        InputCmd::ClearLocation { reply } => LocationCommand::Clear { reply },
        other => return Some(other),
    };

    let result = if location.status.get().available {
        location.sender.try_send(command)
    } else {
        Err(tokio::sync::mpsc::error::TrySendError::Closed(command))
    };
    if let Err(error) = result {
        let (reason, command) = match error {
            tokio::sync::mpsc::error::TrySendError::Full(command) => {
                ("location simulation is busy", command)
            }
            tokio::sync::mpsc::error::TrySendError::Closed(command) => {
                ("location simulation is unavailable", command)
            }
        };
        match command {
            LocationCommand::Set { reply, .. } | LocationCommand::Clear { reply } => {
                let _ = reply.send(Err(reason.into()));
            }
        }
    }
    None
}

struct LocationBridge {
    sender: tokio::sync::mpsc::Sender<LocationCommand>,
    status: LocationStatusSlot,
}

struct DeviceManagementServices {
    icons: tokio::sync::mpsc::Sender<crate::app_icons::AppIconCommand>,
    companions: tokio::sync::mpsc::Sender<crate::companion_devices::CompanionDeviceCommand>,
    home_screen: tokio::sync::mpsc::Sender<crate::home_screen::HomeScreenCommand>,
    running_processes: tokio::sync::mpsc::Sender<crate::running_processes::RunningProcessCommand>,
    app_lifecycle: tokio::sync::mpsc::Sender<crate::app_lifecycle::AppLifecycleCommand>,
    wda: tokio::sync::mpsc::Sender<crate::wda_automation::WdaAutomationCommand>,
    wda_runner: tokio::sync::mpsc::Sender<crate::wda_runner::WdaRunnerCommand>,
    app_console: tokio::sync::mpsc::Sender<crate::app_console::AppConsoleCommand>,
    documents: tokio::sync::mpsc::Sender<crate::app_documents::AppDocumentCommand>,
    device_files: tokio::sync::mpsc::Sender<crate::device_files::DeviceFileCommand>,
    screen_capture: tokio::sync::mpsc::Sender<crate::screen_capture::ScreenCaptureCommand>,
    network_capture: tokio::sync::mpsc::Sender<crate::network_capture::NetworkCaptureCommand>,
    bluetooth_capture: tokio::sync::mpsc::Sender<crate::bluetooth_capture::BluetoothCaptureCommand>,
    device_backup: tokio::sync::mpsc::Sender<crate::device_backup::DeviceBackupCommand>,
    sysdiagnose: tokio::sync::mpsc::Sender<crate::sysdiagnose::SysdiagnoseCommand>,
    log_archive: tokio::sync::mpsc::Sender<crate::log_archive::LogArchiveCommand>,
    developer_image: tokio::sync::mpsc::Sender<crate::developer_image::DeveloperImageMountCommand>,
    device_conditions: tokio::sync::mpsc::Sender<crate::device_conditions::DeviceConditionCommand>,
    provisioning: tokio::sync::mpsc::Sender<crate::provisioning::ProvisioningCommand>,
}

fn reject_provisioning_command(command: crate::provisioning::ProvisioningCommand, reason: &str) {
    use crate::provisioning::ProvisioningCommand;

    let failure = || crate::provisioning::ProvisioningFailure::Unavailable(reason.into());
    match command {
        ProvisioningCommand::List { reply, .. } => {
            let _ = reply.send(Err(failure()));
        }
        ProvisioningCommand::Install { reply, .. } => {
            let _ = reply.send(Err(failure()));
        }
        ProvisioningCommand::Remove { reply, .. } => {
            let _ = reply.send(Err(failure()));
        }
        ProvisioningCommand::TrustSigner { reply, .. } => {
            let _ = reply.send(Err(failure()));
        }
    }
}

fn reject_wda_command(command: crate::wda_automation::WdaAutomationCommand, reason: &str) {
    use crate::wda_automation::WdaAutomationCommand;

    match command {
        WdaAutomationCommand::Status { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        WdaAutomationCommand::Source { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        WdaAutomationCommand::DeviceState { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        WdaAutomationCommand::Find { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        WdaAutomationCommand::Inspect { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        WdaAutomationCommand::WaitForElement { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        WdaAutomationCommand::Click { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        WdaAutomationCommand::TypeText { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        WdaAutomationCommand::DoubleTap { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        WdaAutomationCommand::TouchAndHold { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        WdaAutomationCommand::Scroll { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
    }
}

fn reject_device_condition_command(
    command: crate::device_conditions::DeviceConditionCommand,
    reason: &str,
) {
    use crate::device_conditions::DeviceConditionCommand;

    match command {
        DeviceConditionCommand::Apply { reply, .. }
        | DeviceConditionCommand::Clear { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
    }
}

fn reject_network_capture_command(
    command: crate::network_capture::NetworkCaptureCommand,
    reason: &str,
) {
    use crate::network_capture::NetworkCaptureCommand;

    match command {
        NetworkCaptureCommand::Start { reply, .. } | NetworkCaptureCommand::Stop { reply } => {
            let _ = reply.send(Err(reason.into()));
        }
    }
}

fn reject_bluetooth_capture_command(
    command: crate::bluetooth_capture::BluetoothCaptureCommand,
    reason: &str,
) {
    use crate::bluetooth_capture::BluetoothCaptureCommand;

    match command {
        BluetoothCaptureCommand::Start { reply, .. } | BluetoothCaptureCommand::Stop { reply } => {
            let _ = reply.send(Err(reason.into()));
        }
    }
}

fn reject_device_backup_command(command: crate::device_backup::DeviceBackupCommand, reason: &str) {
    use crate::device_backup::DeviceBackupCommand;

    match command {
        DeviceBackupCommand::Start { reply, .. } | DeviceBackupCommand::Stop { reply } => {
            let _ = reply.send(Err(reason.into()));
        }
    }
}

fn reject_sysdiagnose_command(command: crate::sysdiagnose::SysdiagnoseCommand, reason: &str) {
    use crate::sysdiagnose::SysdiagnoseCommand;

    match command {
        SysdiagnoseCommand::Start { reply, .. } | SysdiagnoseCommand::Stop { reply } => {
            let _ = reply.send(Err(reason.into()));
        }
    }
}

fn reject_log_archive_command(command: crate::log_archive::LogArchiveCommand, reason: &str) {
    use crate::log_archive::LogArchiveCommand;

    match command {
        LogArchiveCommand::Start { reply, .. } | LogArchiveCommand::Stop { reply } => {
            let _ = reply.send(Err(reason.into()));
        }
    }
}

fn reject_developer_image_command(
    command: crate::developer_image::DeveloperImageMountCommand,
    reason: &str,
) {
    use crate::developer_image::DeveloperImageMountCommand;

    match command {
        DeveloperImageMountCommand::Start { reply, .. }
        | DeveloperImageMountCommand::Stop { reply }
        | DeveloperImageMountCommand::Unmount { reply } => {
            let _ = reply.send(Err(reason.into()));
        }
    }
}

fn reject_app_document_command(command: crate::app_documents::AppDocumentCommand, reason: &str) {
    use crate::app_documents::AppDocumentCommand;

    match command {
        AppDocumentCommand::List { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        AppDocumentCommand::Export { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        AppDocumentCommand::Import { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        AppDocumentCommand::CreateDirectory { reply, .. }
        | AppDocumentCommand::Rename { reply, .. }
        | AppDocumentCommand::Delete { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
    }
}

fn reject_device_file_command(command: crate::device_files::DeviceFileCommand, reason: &str) {
    use crate::device_files::DeviceFileCommand;

    match command {
        DeviceFileCommand::List { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        DeviceFileCommand::Export { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        DeviceFileCommand::Import { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
        DeviceFileCommand::CreateDirectory { reply, .. }
        | DeviceFileCommand::Rename { reply, .. }
        | DeviceFileCommand::Delete { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
    }
}

struct DeviceManagement {
    provider: Arc<dyn IdeviceProvider>,
    power: DevicePowerSlot,
    app_operation: AppOperationSlot,
    operation_task: Option<ActiveAppOperation>,
    details: Option<DeviceDetails>,
    app_service: Option<AppServiceClient<Box<dyn ReadWrite>>>,
    installation_proxy: Option<InstallationProxyClient>,
    app_service_transport: AppServiceTransport,
    services: DeviceManagementServices,
}

struct AppServiceTransport {
    adapter: AdapterHandle,
    handshake: RsdHandshake,
}

struct ActiveAppOperation {
    id: u64,
    handle: tokio::task::JoinHandle<()>,
}

impl Drop for DeviceManagement {
    fn drop(&mut self) {
        if let Some(operation) = self.operation_task.take() {
            if !operation.handle.is_finished() {
                operation.handle.abort();
            }
            self.app_operation.cancel(operation.id);
        }
    }
}

impl DeviceManagement {
    fn new(
        provider: Arc<dyn IdeviceProvider>,
        app_operation: AppOperationSlot,
        details: Option<DeviceDetails>,
        app_service: Option<AppServiceClient<Box<dyn ReadWrite>>>,
        installation_proxy: Option<InstallationProxyClient>,
        app_service_transport: AppServiceTransport,
        services: DeviceManagementServices,
    ) -> Self {
        Self {
            provider,
            power: DevicePowerSlot::default(),
            app_operation,
            operation_task: None,
            details,
            app_service,
            installation_proxy,
            app_service_transport,
            services,
        }
    }

    fn fallback(
        provider: Arc<dyn IdeviceProvider>,
        app_operation: AppOperationSlot,
        details: Option<DeviceDetails>,
        installation_proxy: Option<InstallationProxyClient>,
        app_service_transport: AppServiceTransport,
        services: DeviceManagementServices,
    ) -> Self {
        Self::new(
            provider,
            app_operation,
            details,
            None,
            installation_proxy,
            app_service_transport,
            services,
        )
    }

    async fn reconnect_app_clients(&mut self) -> Result<(), String> {
        self.app_service.take();
        self.installation_proxy.take();
        let mut adapter = self.app_service_transport.adapter.clone();
        let mut handshake = self.app_service_transport.handshake.clone();
        let provider = self.provider.clone();
        let (app_service, installation_proxy) = tokio::join!(
            tokio::time::timeout(
                Duration::from_secs(5),
                AppServiceClient::connect_rsd(&mut adapter, &mut handshake),
            ),
            tokio::time::timeout(
                Duration::from_secs(5),
                InstallationProxyClient::connect(&*provider),
            ),
        );
        let mut errors = Vec::new();
        match app_service {
            Ok(Ok(client)) => self.app_service = Some(client),
            Ok(Err(error)) => errors.push(format!("CoreDevice AppService: {error:?}")),
            Err(_) => errors.push("CoreDevice AppService connection timed out".into()),
        }
        match installation_proxy {
            Ok(Ok(client)) => self.installation_proxy = Some(client),
            Ok(Err(error)) => errors.push(format!("InstallationProxy: {error:?}")),
            Err(_) => errors.push("InstallationProxy connection timed out".into()),
        }
        if self.app_service.is_some() || self.installation_proxy.is_some() {
            if !errors.is_empty() {
                tracing::debug!(errors = ?errors, "some app listing services remain unavailable after reconnect");
            }
            Ok(())
        } else {
            Err(format!(
                "unable to reconnect app listing services: {}",
                errors.join("; ")
            ))
        }
    }

    async fn ensure_app_service(&mut self) -> Result<(), String> {
        if self.app_service.is_some() {
            return Ok(());
        }
        self.reconnect_app_clients().await?;
        self.app_service
            .is_some()
            .then_some(())
            .ok_or_else(|| "CoreDevice AppService is unavailable after reconnect".to_string())
    }

    fn clear_finished_operation(&mut self) {
        if self
            .operation_task
            .as_ref()
            .is_some_and(|operation| operation.handle.is_finished())
            && let Some(operation) = self.operation_task.take()
        {
            self.app_operation
                .fail(operation.id, "app operation ended unexpectedly".into());
        }
    }

    async fn install_app(&mut self, path: PathBuf, kind: AppOperationKind) -> Result<(), String> {
        self.clear_finished_operation();
        let metadata = crate::ipa::inspect(&path).await?;
        let id = self.app_operation.start(kind, metadata.file_name.clone())?;

        let provider = self.provider.clone();
        let details = self.details.clone();
        let operation = self.app_operation.clone();
        let task_operation = operation.clone();
        let handle = tokio::spawn(async move {
            let progress = |stage: &'static str| {
                move |(progress, (operation, operation_id)): (u64, (AppOperationSlot, u64))| async move {
                    operation.update(operation_id, stage, Some(progress.min(100) as u8));
                }
            };
            let result = async {
                let ipa_operation = match kind {
                    AppOperationKind::Install => IpaOperation::Install,
                    AppOperationKind::Upgrade => IpaOperation::Upgrade,
                    AppOperationKind::Uninstall => {
                        unreachable!("package operation cannot uninstall")
                    }
                };
                let preflight = build_ipa_preflight(
                    provider.as_ref(),
                    details.as_ref(),
                    metadata.clone(),
                    ipa_operation,
                )
                .await?;
                reject_blocked_ipa(&preflight)?;
                operation.update(id, "uploading", None);
                match kind {
                    AppOperationKind::Install => install_package_with_callback(
                        provider.as_ref(),
                        metadata.path,
                        None,
                        progress("installing"),
                        (task_operation, id),
                    )
                    .await
                    .map_err(|error| format!("unable to install IPA: {error:?}")),
                    AppOperationKind::Upgrade => upgrade_package_with_callback(
                        provider.as_ref(),
                        metadata.path,
                        None,
                        progress("upgrading"),
                        (task_operation, id),
                    )
                    .await
                    .map_err(|error| format!("unable to upgrade IPA: {error:?}")),
                    AppOperationKind::Uninstall => {
                        unreachable!("package operation cannot uninstall")
                    }
                }
            }
            .await;
            match result {
                Ok(()) => operation.succeed(id),
                Err(error) => operation.fail(id, error),
            }
        });
        self.operation_task = Some(ActiveAppOperation { id, handle });
        Ok(())
    }

    fn uninstall_app(&mut self, bundle_id: String) -> Result<(), String> {
        self.clear_finished_operation();
        let id = self
            .app_operation
            .start(AppOperationKind::Uninstall, bundle_id.clone())?;
        self.app_operation.update(id, "verifying", None);

        let provider = self.provider.clone();
        let operation = self.app_operation.clone();
        let task_operation = operation.clone();
        let handle = tokio::spawn(async move {
            let result =
                uninstall_user_app(provider.as_ref(), &bundle_id, task_operation.clone(), id).await;
            match result {
                Ok(()) => operation.succeed(id),
                Err(error) => operation.fail(id, error),
            }
        });
        self.operation_task = Some(ActiveAppOperation { id, handle });
        Ok(())
    }

    async fn handle(&mut self, command: InputCmd) -> Option<InputCmd> {
        match command {
            InputCmd::GetDeviceDetails(reply) => {
                let Some(mut details) = self.details.clone() else {
                    let _ = reply.send(Err("device metadata is unavailable".to_string()));
                    return None;
                };
                let provider = self.provider.clone();
                tokio::spawn(async move {
                    let requested_udid = details.udid.clone();
                    let (
                        details_result,
                        battery_result,
                        developer_mode_result,
                        developer_image_result,
                        activation_state_result,
                    ) = tokio::join!(
                        tokio::time::timeout(
                            Duration::from_secs(3),
                            read_device_details(provider.as_ref(), requested_udid),
                        ),
                        tokio::time::timeout(
                            Duration::from_secs(3),
                            read_device_battery(provider.as_ref()),
                        ),
                        tokio::time::timeout(
                            Duration::from_secs(3),
                            read_developer_mode_status(provider.as_ref()),
                        ),
                        tokio::time::timeout(
                            Duration::from_secs(3),
                            crate::developer_image::is_mounted(
                                provider.as_ref(),
                                &details.product_version,
                            ),
                        ),
                        tokio::time::timeout(
                            Duration::from_secs(3),
                            read_activation_state(provider.as_ref()),
                        ),
                    );
                    match details_result {
                        Ok(Some(refreshed)) => details = refreshed,
                        Ok(None) => tracing::warn!("device metadata refresh unavailable"),
                        Err(_) => tracing::warn!("device metadata refresh timed out"),
                    }
                    match battery_result {
                        Ok(Ok(battery)) => {
                            tracing::debug!(
                                level_percent = ?battery.level_percent,
                                is_charging = ?battery.is_charging,
                                cycle_count = ?battery.cycle_count,
                                "device battery diagnostics refreshed"
                            );
                            details.battery = Some(battery);
                        }
                        Ok(Err(error)) => {
                            tracing::warn!(%error, "device battery diagnostics unavailable");
                        }
                        Err(_) => {
                            tracing::warn!("device battery diagnostics timed out");
                        }
                    }
                    match developer_mode_result {
                        Ok(Ok(enabled)) => {
                            tracing::debug!(enabled, "developer mode status refreshed");
                            details.developer_mode_enabled = Some(enabled);
                        }
                        Ok(Err(error)) => {
                            tracing::warn!(%error, "developer mode status unavailable");
                        }
                        Err(_) => {
                            tracing::warn!("developer mode status timed out");
                        }
                    }
                    match developer_image_result {
                        Ok(Ok(mounted)) => {
                            tracing::debug!(mounted, "developer image status refreshed");
                            details.developer_image_mounted = Some(mounted);
                        }
                        Ok(Err(error)) => {
                            tracing::warn!(%error, "developer image status unavailable");
                        }
                        Err(_) => {
                            tracing::warn!("developer image status timed out");
                        }
                    }
                    match activation_state_result {
                        Ok(Ok(state)) => {
                            tracing::debug!(?state, "device activation state refreshed");
                            details.activation_state = Some(state);
                        }
                        Ok(Err(error)) => {
                            tracing::warn!(%error, "device activation state unavailable");
                        }
                        Err(_) => {
                            tracing::warn!("device activation state timed out");
                        }
                    }
                    let _ = reply.send(Ok(details));
                });
                None
            }
            InputCmd::RenameDevice { name, reply } => {
                let provider = self.provider.clone();
                tokio::spawn(async move {
                    let result = tokio::time::timeout(
                        Duration::from_secs(6),
                        rename_device(provider.as_ref(), &name),
                    )
                    .await
                    .map_err(|_| "device rename timed out".to_string())
                    .and_then(|result| result);
                    let _ = reply.send(result);
                });
                None
            }
            InputCmd::DeveloperMode(command) => {
                developer_mode::execute(self.provider.clone(), command);
                None
            }
            InputCmd::ListApps {
                include_system,
                include_app_clips,
                reply,
            } => {
                let first = list_device_apps(
                    self.app_service.as_mut(),
                    self.installation_proxy.as_mut(),
                    include_system,
                    include_app_clips,
                    false,
                )
                .await;
                let result = match first {
                    Ok(apps) => Ok(apps),
                    Err(first_error) => {
                        tracing::warn!(
                            error = %first_error,
                            "app listing failed; reconnecting read-only services once"
                        );
                        match self.reconnect_app_clients().await {
                            Ok(()) => list_device_apps(
                                self.app_service.as_mut(),
                                self.installation_proxy.as_mut(),
                                include_system,
                                include_app_clips,
                                true,
                            )
                            .await
                            .map_err(|retry_error| {
                                format!(
                                    "{retry_error} (initial app listing failure: {first_error})"
                                )
                            }),
                            Err(reconnect_error) => {
                                Err(format!("{first_error}; {reconnect_error}"))
                            }
                        }
                    }
                };
                let _ = reply.send(result);
                None
            }
            InputCmd::ListCompanionDevices(reply) => {
                let command = crate::companion_devices::CompanionDeviceCommand::List { reply };
                if let Err(error) = self.services.companions.try_send(command) {
                    let (reason, command) = match error {
                        tokio::sync::mpsc::error::TrySendError::Full(command) => {
                            ("companion device service is busy", command)
                        }
                        tokio::sync::mpsc::error::TrySendError::Closed(command) => {
                            ("companion device service is unavailable", command)
                        }
                    };
                    match command {
                        crate::companion_devices::CompanionDeviceCommand::List { reply } => {
                            let _ = reply.send(Err(reason.into()));
                        }
                    }
                }
                None
            }
            InputCmd::GetHomeScreenLayout(reply) => {
                let command = crate::home_screen::HomeScreenCommand::Get { reply };
                if let Err(error) = self.services.home_screen.try_send(command) {
                    let (reason, command) = match error {
                        tokio::sync::mpsc::error::TrySendError::Full(command) => {
                            ("home screen service is busy", command)
                        }
                        tokio::sync::mpsc::error::TrySendError::Closed(command) => {
                            ("home screen service is unavailable", command)
                        }
                    };
                    let crate::home_screen::HomeScreenCommand::Get { reply } = command;
                    let _ = reply.send(Err(reason.into()));
                }
                None
            }
            InputCmd::RunningProcess(command) => {
                if let Err(error) = self.services.running_processes.try_send(command) {
                    let (reason, command) = match error {
                        tokio::sync::mpsc::error::TrySendError::Full(command) => {
                            ("running process service is busy", command)
                        }
                        tokio::sync::mpsc::error::TrySendError::Closed(command) => {
                            ("running process service is unavailable", command)
                        }
                    };
                    command.reject(reason);
                }
                None
            }
            InputCmd::AppLifecycle(command) => {
                if let Err(error) = self.services.app_lifecycle.try_send(command) {
                    let (reason, command) = match error {
                        tokio::sync::mpsc::error::TrySendError::Full(command) => {
                            ("application lifecycle service is busy", command)
                        }
                        tokio::sync::mpsc::error::TrySendError::Closed(command) => {
                            ("application lifecycle service is unavailable", command)
                        }
                    };
                    command.reject(reason);
                }
                None
            }
            InputCmd::WdaAutomation(command) => {
                if let Err(error) = self.services.wda.try_send(command) {
                    let (reason, command) = match error {
                        tokio::sync::mpsc::error::TrySendError::Full(command) => {
                            ("WDA automation service is busy", command)
                        }
                        tokio::sync::mpsc::error::TrySendError::Closed(command) => {
                            ("WDA automation service is unavailable", command)
                        }
                    };
                    reject_wda_command(command, reason);
                }
                None
            }
            InputCmd::WdaRunner(command) => {
                if let Err(error) = self.services.wda_runner.try_send(command) {
                    let (reason, command) = match error {
                        tokio::sync::mpsc::error::TrySendError::Full(command) => {
                            ("WDA runner service is busy", command)
                        }
                        tokio::sync::mpsc::error::TrySendError::Closed(command) => {
                            ("WDA runner service is unavailable", command)
                        }
                    };
                    command.reject(reason);
                }
                None
            }
            InputCmd::AppConsole(command) => {
                if let Err(error) = self.services.app_console.try_send(command) {
                    let (reason, command) = match error {
                        tokio::sync::mpsc::error::TrySendError::Full(command) => {
                            ("application console service is busy", command)
                        }
                        tokio::sync::mpsc::error::TrySendError::Closed(command) => {
                            ("application console service is unavailable", command)
                        }
                    };
                    command.reject(reason);
                }
                None
            }
            InputCmd::GetAppIcon { bundle_id, reply } => {
                let command = crate::app_icons::AppIconCommand { bundle_id, reply };
                if let Err(error) = self.services.icons.try_send(command) {
                    let (reason, command) = match error {
                        tokio::sync::mpsc::error::TrySendError::Full(command) => {
                            ("app icon service is busy", command)
                        }
                        tokio::sync::mpsc::error::TrySendError::Closed(command) => {
                            ("app icon service is unavailable", command)
                        }
                    };
                    let _ = command.reply.send(Err(reason.into()));
                }
                None
            }
            InputCmd::TakeScreenshot(reply) => {
                let command = crate::screen_capture::ScreenCaptureCommand { reply };
                if let Err(error) = self.services.screen_capture.try_send(command) {
                    let (reason, command) = match error {
                        tokio::sync::mpsc::error::TrySendError::Full(command) => {
                            ("screen capture service is busy", command)
                        }
                        tokio::sync::mpsc::error::TrySendError::Closed(command) => {
                            ("screen capture service is unavailable", command)
                        }
                    };
                    let _ = command.reply.send(Err(reason.into()));
                }
                None
            }
            InputCmd::NetworkCapture(command) => {
                if let Err(error) = self.services.network_capture.try_send(command) {
                    let (reason, command) = match error {
                        tokio::sync::mpsc::error::TrySendError::Full(command) => {
                            ("packet capture service is busy", command)
                        }
                        tokio::sync::mpsc::error::TrySendError::Closed(command) => {
                            ("packet capture service is unavailable", command)
                        }
                    };
                    reject_network_capture_command(command, reason);
                }
                None
            }
            InputCmd::BluetoothCapture(command) => {
                if let Err(error) = self.services.bluetooth_capture.try_send(command) {
                    let (reason, command) = match error {
                        tokio::sync::mpsc::error::TrySendError::Full(command) => {
                            ("Bluetooth capture service is busy", command)
                        }
                        tokio::sync::mpsc::error::TrySendError::Closed(command) => {
                            ("Bluetooth capture service is unavailable", command)
                        }
                    };
                    reject_bluetooth_capture_command(command, reason);
                }
                None
            }
            InputCmd::DeviceBackup(command) => {
                if let Err(error) = self.services.device_backup.try_send(command) {
                    let (reason, command) = match error {
                        tokio::sync::mpsc::error::TrySendError::Full(command) => {
                            ("device backup service is busy", command)
                        }
                        tokio::sync::mpsc::error::TrySendError::Closed(command) => {
                            ("device backup service is unavailable", command)
                        }
                    };
                    reject_device_backup_command(command, reason);
                }
                None
            }
            InputCmd::Sysdiagnose(command) => {
                if let Err(error) = self.services.sysdiagnose.try_send(command) {
                    let (reason, command) = match error {
                        tokio::sync::mpsc::error::TrySendError::Full(command) => {
                            ("sysdiagnose service is busy", command)
                        }
                        tokio::sync::mpsc::error::TrySendError::Closed(command) => {
                            ("sysdiagnose service is unavailable", command)
                        }
                    };
                    reject_sysdiagnose_command(command, reason);
                }
                None
            }
            InputCmd::LogArchive(command) => {
                if let Err(error) = self.services.log_archive.try_send(command) {
                    let (reason, command) = match error {
                        tokio::sync::mpsc::error::TrySendError::Full(command) => {
                            ("log archive service is busy", command)
                        }
                        tokio::sync::mpsc::error::TrySendError::Closed(command) => {
                            ("log archive service is unavailable", command)
                        }
                    };
                    reject_log_archive_command(command, reason);
                }
                None
            }
            InputCmd::DeveloperImageMount(command) => {
                if let Err(error) = self.services.developer_image.try_send(command) {
                    let (reason, command) = match error {
                        tokio::sync::mpsc::error::TrySendError::Full(command) => {
                            ("developer image service is busy", command)
                        }
                        tokio::sync::mpsc::error::TrySendError::Closed(command) => {
                            ("developer image service is unavailable", command)
                        }
                    };
                    reject_developer_image_command(command, reason);
                }
                None
            }
            InputCmd::DeviceCondition(command) => {
                if let Err(error) = self.services.device_conditions.try_send(command) {
                    let (reason, command) = match error {
                        tokio::sync::mpsc::error::TrySendError::Full(command) => {
                            ("device condition service is busy", command)
                        }
                        tokio::sync::mpsc::error::TrySendError::Closed(command) => {
                            ("device condition service is unavailable", command)
                        }
                    };
                    reject_device_condition_command(command, reason);
                }
                None
            }
            InputCmd::AppDocuments(command) => {
                if let Err(error) = self.services.documents.try_send(command) {
                    let (reason, command) = match error {
                        tokio::sync::mpsc::error::TrySendError::Full(command) => {
                            ("application document service is busy", command)
                        }
                        tokio::sync::mpsc::error::TrySendError::Closed(command) => {
                            ("application document service is unavailable", command)
                        }
                    };
                    reject_app_document_command(command, reason);
                }
                None
            }
            InputCmd::DeviceFiles(command) => {
                if let Err(error) = self.services.device_files.try_send(command) {
                    let (reason, command) = match error {
                        tokio::sync::mpsc::error::TrySendError::Full(command) => {
                            ("device file service is busy", command)
                        }
                        tokio::sync::mpsc::error::TrySendError::Closed(command) => {
                            ("device file service is unavailable", command)
                        }
                    };
                    reject_device_file_command(command, reason);
                }
                None
            }
            InputCmd::LockDevice(reply) => {
                self.start_power_action(DevicePowerAction::Lock, reply);
                None
            }
            InputCmd::RestartDevice(reply) => {
                self.start_power_action(DevicePowerAction::Restart, reply);
                None
            }
            InputCmd::ShutdownDevice(reply) => {
                self.start_power_action(DevicePowerAction::Shutdown, reply);
                None
            }
            InputCmd::Provisioning(command) => {
                if let Err(error) = self.services.provisioning.try_send(command) {
                    let (reason, command) = match error {
                        tokio::sync::mpsc::error::TrySendError::Full(command) => {
                            ("provisioning profile service is busy", command)
                        }
                        tokio::sync::mpsc::error::TrySendError::Closed(command) => {
                            ("provisioning profile service is unavailable", command)
                        }
                    };
                    reject_provisioning_command(command, reason);
                }
                None
            }
            InputCmd::LaunchApp { bundle_id, reply } => {
                let result = match self.ensure_app_service().await {
                    Ok(()) => self
                        .app_service
                        .as_mut()
                        .expect("AppService was ensured")
                        .launch_application(bundle_id, &[], true, false, None, None, None)
                        .await
                        .map(|_| ())
                        .map_err(|error| format!("unable to launch app: {error:?}")),
                    Err(error) => Err(format!("app launch requires AppService: {error}")),
                };
                if result.is_err() {
                    self.app_service.take();
                }
                let _ = reply.send(result);
                None
            }
            InputCmd::StopApp { bundle_id, reply } => {
                let result = match self.ensure_app_service().await {
                    Ok(()) => {
                        stop_device_app(
                            self.app_service.as_mut().expect("AppService was ensured"),
                            &bundle_id,
                        )
                        .await
                    }
                    Err(error) => Err(format!("app stop requires AppService: {error}")),
                };
                if result.is_err() {
                    self.app_service.take();
                }
                let _ = reply.send(result);
                None
            }
            InputCmd::ListCrashReports(reply) => {
                let provider = self.provider.clone();
                tokio::spawn(async move {
                    let _ = reply.send(crate::crash_reports::list(provider).await);
                });
                None
            }
            InputCmd::ReadCrashReport {
                device_path,
                max_bytes,
                reply,
            } => {
                let provider = self.provider.clone();
                tokio::spawn(async move {
                    let result = crate::crash_reports::read(provider, device_path, max_bytes).await;
                    let _ = reply.send(result);
                });
                None
            }
            InputCmd::ExportCrashReport {
                device_path,
                destination,
                reply,
            } => {
                let provider = self.provider.clone();
                tokio::spawn(async move {
                    let result =
                        crate::crash_reports::export(provider, device_path, &destination).await;
                    let _ = reply.send(result);
                });
                None
            }
            InputCmd::DeleteCrashReport { device_path, reply } => {
                let provider = self.provider.clone();
                tokio::spawn(async move {
                    let result = crate::crash_reports::delete(provider, device_path).await;
                    let _ = reply.send(result);
                });
                None
            }
            InputCmd::PreflightApp {
                path,
                operation,
                reply,
            } => {
                let provider = self.provider.clone();
                let details = self.details.clone();
                tokio::spawn(async move {
                    let result = async {
                        let metadata = crate::ipa::inspect(&path).await?;
                        build_ipa_preflight(
                            provider.as_ref(),
                            details.as_ref(),
                            metadata,
                            operation,
                        )
                        .await
                    }
                    .await;
                    let _ = reply.send(result);
                });
                None
            }
            InputCmd::InstallApp { path, reply } => {
                let result = self.install_app(path, AppOperationKind::Install).await;
                let _ = reply.send(result);
                None
            }
            InputCmd::UpgradeApp { path, reply } => {
                let result = self.install_app(path, AppOperationKind::Upgrade).await;
                let _ = reply.send(result);
                None
            }
            InputCmd::UninstallApp { bundle_id, reply } => {
                let result = self.uninstall_app(bundle_id);
                let _ = reply.send(result);
                None
            }
            other => Some(other),
        }
    }

    fn start_power_action(
        &self,
        action: DevicePowerAction,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    ) {
        match self.power.try_start() {
            Ok(lease) => {
                spawn_device_power_action(self.provider.clone(), action, reply, lease);
            }
            Err(error) => {
                let _ = reply.send(Err(error));
            }
        }
    }
}

fn spawn_device_power_action(
    provider: Arc<dyn IdeviceProvider>,
    action: DevicePowerAction,
    reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    _lease: DevicePowerLease,
) {
    tokio::spawn(async move {
        let result = tokio::time::timeout(Duration::from_secs(8), async {
            let mut diagnostics = DiagnosticsRelayClient::connect(provider.as_ref())
                .await
                .map_err(|error| format!("cannot connect diagnostics relay: {error:?}"))?;
            match action {
                DevicePowerAction::Lock => diagnostics.sleep().await,
                DevicePowerAction::Restart => diagnostics.restart().await,
                DevicePowerAction::Shutdown => diagnostics.shutdown().await,
            }
            .map_err(|error| format!("device power command failed: {error:?}"))
        })
        .await
        .unwrap_or_else(|_| Err("device power command timed out".into()));
        match &result {
            Ok(()) => tracing::info!(?action, "device power command accepted"),
            Err(error) => tracing::warn!(?action, %error, "device power command failed"),
        }
        let _ = reply.send(result);
    });
}

async fn build_ipa_preflight(
    provider: &dyn IdeviceProvider,
    details: Option<&DeviceDetails>,
    metadata: IpaArchiveMetadata,
    operation: IpaOperation,
) -> Result<IpaPreflight, String> {
    let mut client = InstallationProxyClient::connect(provider)
        .await
        .map_err(|error| format!("installation proxy is unavailable: {error:?}"))?;
    let mut apps = client
        .get_apps(Some("User"), Some(vec![metadata.bundle_id.clone()]))
        .await
        .map_err(|error| format!("unable to verify installed app: {error:?}"))?;
    let installed_app = apps.remove(&metadata.bundle_id).map(|value| {
        let app = device_app_from_installation(metadata.bundle_id.clone(), &value)
            .ok_or_else(|| "device returned invalid installed app metadata".to_string())?;
        Ok::<_, String>(InstalledAppMatch {
            name: bounded_ipa_device_text(&app.name),
            version: app.version.map(|value| bounded_ipa_device_text(&value)),
            bundle_version: app
                .bundle_version
                .map(|value| bounded_ipa_device_text(&value)),
        })
    });
    let installed_app = match installed_app {
        Some(result) => Some(result?),
        None => None,
    };

    let positive_capabilities_supported = if metadata.required_capabilities.is_empty() {
        Some(true)
    } else {
        match client
            .check_capabilities_match(
                metadata
                    .required_capabilities
                    .iter()
                    .cloned()
                    .map(plist::Value::String)
                    .collect(),
                None,
            )
            .await
        {
            Ok(matches) => Some(matches),
            Err(error) => {
                tracing::warn!(?error, "unable to check IPA device capabilities");
                None
            }
        }
    };
    let capabilities_supported = match positive_capabilities_supported {
        Some(false) => Some(false),
        Some(true) if metadata.prohibited_capabilities.is_empty() => Some(true),
        _ => None,
    };
    let minimum_os_supported = match (&metadata.minimum_os_version, details) {
        (Some(minimum), Some(details)) => {
            crate::ipa::version_at_least(&details.product_version, minimum)
        }
        _ => None,
    };
    let device_family_supported = details.and_then(|details| {
        crate::ipa::device_family_supported(&details.product_type, &metadata.device_families)
    });
    let compatibility = IpaCompatibility {
        minimum_os_supported,
        device_family_supported,
        capabilities_supported,
    };
    let blocking_issues =
        crate::ipa::preflight_issues(operation, installed_app.is_some(), &compatibility);
    Ok(IpaPreflight {
        operation,
        file_name: metadata.file_name,
        file_size_bytes: metadata.file_size_bytes,
        bundle_id: metadata.bundle_id,
        name: metadata.name,
        version: metadata.version,
        bundle_version: metadata.bundle_version,
        minimum_os_version: metadata.minimum_os_version,
        device_families: metadata.device_families,
        required_capabilities: metadata.required_capabilities,
        prohibited_capabilities: metadata.prohibited_capabilities,
        installed_app,
        compatibility,
        operation_allowed: blocking_issues.is_empty(),
        blocking_issues,
    })
}

fn bounded_ipa_device_text(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(256)
        .collect::<String>()
        .trim()
        .to_string()
}

fn reject_blocked_ipa(preflight: &IpaPreflight) -> Result<(), String> {
    let Some(issue) = preflight.blocking_issues.first() else {
        return Ok(());
    };
    Err(match issue {
        IpaPreflightIssue::AlreadyInstalled => {
            "this app is already installed; use the explicit upgrade action".into()
        }
        IpaPreflightIssue::NotInstalled => {
            "this app is not installed; use the explicit install action".into()
        }
        IpaPreflightIssue::MinimumOsUnsupported => {
            "the device does not meet the IPA minimum OS version".into()
        }
        IpaPreflightIssue::DeviceFamilyUnsupported => {
            "the IPA does not support this device family".into()
        }
        IpaPreflightIssue::RequiredCapabilitiesUnsupported => {
            "the device does not provide all IPA required capabilities".into()
        }
    })
}

async fn uninstall_user_app(
    provider: &dyn IdeviceProvider,
    bundle_id: &str,
    operation: AppOperationSlot,
    operation_id: u64,
) -> Result<(), String> {
    let mut client = InstallationProxyClient::connect(provider)
        .await
        .map_err(|error| format!("installation proxy is unavailable: {error:?}"))?;
    let mut matches = client
        .get_apps(Some("User"), Some(vec![bundle_id.to_string()]))
        .await
        .map_err(|error| format!("unable to verify app: {error:?}"))?;
    let value = matches
        .remove(bundle_id)
        .ok_or_else(|| "app is not installed as a user application".to_string())?;
    let app = device_app_from_installation(bundle_id.to_string(), &value)
        .ok_or_else(|| "device returned invalid app metadata".to_string())?;
    if !app.is_removable || app.is_first_party {
        return Err("the selected app is not a removable third-party application".into());
    }

    operation.update(operation_id, "uninstalling", Some(0));
    client
        .uninstall_with_callback(
            bundle_id,
            None,
            |(progress, (operation, id))| async move {
                operation.update(id, "uninstalling", Some(progress.min(100) as u8));
            },
            (operation, operation_id),
        )
        .await
        .map_err(|error| format!("unable to uninstall app: {error:?}"))
}

async fn list_device_apps(
    app_service: Option<&mut AppServiceClient<Box<dyn ReadWrite>>>,
    mut installation_proxy: Option<&mut InstallationProxyClient>,
    include_system: bool,
    include_app_clips: bool,
    allow_fallback_after_app_service_error: bool,
) -> Result<Vec<DeviceApp>, String> {
    if let Some(client) = app_service {
        match client
            .list_apps(include_app_clips, true, false, false, include_system)
            .await
        {
            Ok(entries) => {
                let application_type = if include_system { "Any" } else { "User" };
                let bundle_identifiers = entries
                    .iter()
                    .map(|entry| entry.bundle_identifier.clone())
                    .collect();
                let installation_apps = if entries.is_empty() {
                    std::collections::HashMap::new()
                } else {
                    match installation_proxy.as_deref_mut() {
                        Some(client) => match client
                            .get_apps(Some(application_type), Some(bundle_identifiers))
                            .await
                        {
                            Ok(apps) => apps,
                            Err(error) => {
                                tracing::warn!(
                                    "installation proxy app metadata unavailable: {error:?}"
                                );
                                std::collections::HashMap::new()
                            }
                        },
                        None => std::collections::HashMap::new(),
                    }
                };
                let processes = match client.list_processes().await {
                    Ok(processes) => Some(processes),
                    Err(error) => {
                        tracing::warn!("CoreDevice process list unavailable: {error:?}");
                        None
                    }
                };
                return Ok(sort_device_apps(
                    entries
                        .into_iter()
                        .map(|entry| {
                            let metadata = installation_apps.get(&entry.bundle_identifier);
                            let documents_available =
                                metadata.is_some_and(installation_supports_documents);
                            let (
                                static_disk_usage_bytes,
                                dynamic_disk_usage_bytes,
                                total_disk_usage_bytes,
                            ) = metadata.map(app_disk_usage).unwrap_or((None, None, None));
                            let signing_kind = app_signing_kind(
                                metadata,
                                entry.is_first_party,
                                entry.is_developer_app,
                            );
                            let is_developer_app = entry.is_developer_app
                                || signing_kind == crate::protocol::AppSigningKind::Development;
                            let minimum_os_version = metadata.and_then(app_minimum_os_version);
                            let debuggable = metadata.and_then(app_debuggable);
                            DeviceApp {
                                is_running: processes.as_ref().map(|processes| {
                                    processes.iter().any(|process| {
                                        process.executable_url.as_ref().is_some_and(|executable| {
                                            crate::app_lifecycle::process_executable_belongs_to_app(
                                                &entry.path,
                                                &executable.relative,
                                            )
                                        })
                                    })
                                }),
                                bundle_id: entry.bundle_identifier,
                                name: entry.name,
                                version: entry.version,
                                bundle_version: entry.bundle_version,
                                is_removable: entry.is_removable,
                                is_first_party: entry.is_first_party,
                                is_developer_app,
                                is_app_clip: entry.is_app_clip,
                                signing_kind,
                                minimum_os_version,
                                debuggable,
                                documents_available,
                                static_disk_usage_bytes,
                                dynamic_disk_usage_bytes,
                                total_disk_usage_bytes,
                            }
                        })
                        .collect(),
                ));
            }
            Err(error) => {
                if !allow_fallback_after_app_service_error {
                    return Err(format!("CoreDevice AppService list failed: {error:?}"));
                }
                if let Some(scope) = extended_app_scope(include_system, include_app_clips) {
                    return Err(format!(
                        "{scope} listing requires CoreDevice AppService: {error:?}"
                    ));
                }
                tracing::warn!(
                    "CoreDevice AppService list failed; using installation proxy: {error:?}"
                );
            }
        }
    }

    if let Some(scope) = extended_app_scope(include_system, include_app_clips) {
        return Err(format!(
            "{scope} listing requires CoreDevice AppService, but it is unavailable"
        ));
    }

    let client =
        installation_proxy.ok_or_else(|| "app listing services are unavailable".to_string())?;
    let entries = client
        .get_apps(Some("User"), None)
        .await
        .map_err(|error| format!("unable to list apps: {error:?}"))?;
    Ok(sort_device_apps(
        entries
            .into_iter()
            .filter_map(|(bundle_id, value)| device_app_from_installation(bundle_id, &value))
            .collect(),
    ))
}

fn extended_app_scope(include_system: bool, include_app_clips: bool) -> Option<&'static str> {
    match (include_system, include_app_clips) {
        (true, true) => Some("system app and App Clip"),
        (true, false) => Some("system app"),
        (false, true) => Some("App Clip"),
        (false, false) => None,
    }
}

fn device_app_from_installation(bundle_id: String, value: &plist::Value) -> Option<DeviceApp> {
    let fields = value.as_dictionary()?;
    let string = |key: &str| {
        fields
            .get(key)
            .and_then(plist::Value::as_string)
            .map(ToOwned::to_owned)
    };
    let boolean = |key: &str| fields.get(key).and_then(plist::Value::as_boolean);
    let name = string("CFBundleDisplayName")
        .or_else(|| string("CFBundleName"))
        .unwrap_or_else(|| bundle_id.clone());
    let signer = string("SignerIdentity").unwrap_or_default();
    let is_first_party = boolean("IsFirstParty").unwrap_or(false);
    let is_developer_app = boolean("IsXcodeManaged").unwrap_or(false)
        || signer.contains("Apple Development")
        || signer.contains("iPhone Developer");
    let (static_disk_usage_bytes, dynamic_disk_usage_bytes, total_disk_usage_bytes) =
        app_disk_usage(value);
    Some(DeviceApp {
        bundle_id,
        name,
        version: string("CFBundleShortVersionString"),
        bundle_version: string("CFBundleVersion"),
        is_removable: boolean("IsRemovable").unwrap_or(false),
        is_first_party,
        is_developer_app,
        is_app_clip: false,
        signing_kind: app_signing_kind(Some(value), is_first_party, is_developer_app),
        minimum_os_version: app_minimum_os_version(value),
        debuggable: app_debuggable(value),
        documents_available: installation_supports_documents(value),
        static_disk_usage_bytes,
        dynamic_disk_usage_bytes,
        total_disk_usage_bytes,
        is_running: None,
    })
}

fn app_signing_kind(
    value: Option<&plist::Value>,
    is_first_party: bool,
    is_developer_app: bool,
) -> crate::protocol::AppSigningKind {
    use crate::protocol::AppSigningKind;

    if is_first_party {
        return AppSigningKind::System;
    }
    let fields = value.and_then(plist::Value::as_dictionary);
    let signer = fields
        .and_then(|fields| fields.get("SignerIdentity"))
        .and_then(plist::Value::as_string)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let xcode_managed = fields
        .and_then(|fields| fields.get("IsXcodeManaged"))
        .and_then(plist::Value::as_boolean)
        .unwrap_or(false);
    if is_developer_app
        || xcode_managed
        || signer.contains("development")
        || signer.contains("developer")
    {
        return AppSigningKind::Development;
    }
    let testflight = fields.is_some_and(|fields| {
        fields.contains_key("BetaExternalVersionIdentifier")
            || fields
                .get("IsBetaApp")
                .and_then(plist::Value::as_boolean)
                .unwrap_or(false)
    });
    if testflight {
        AppSigningKind::TestFlight
    } else if signer.contains("iphone os application signing") {
        AppSigningKind::AppStore
    } else if signer.contains("distribution") {
        AppSigningKind::Distribution
    } else {
        AppSigningKind::Unknown
    }
}

fn app_minimum_os_version(value: &plist::Value) -> Option<String> {
    let version = normalized_app_metadata_text(value, "MinimumOSVersion", 32)?;
    let segments = version.split('.').collect::<Vec<_>>();
    (segments.len() <= 4
        && segments.iter().all(|segment| {
            !segment.is_empty()
                && segment.len() <= 3
                && segment.bytes().all(|byte| byte.is_ascii_digit())
        }))
    .then_some(version)
}

fn app_debuggable(value: &plist::Value) -> Option<bool> {
    value
        .as_dictionary()?
        .get("Entitlements")?
        .as_dictionary()?
        .get("get-task-allow")?
        .as_boolean()
}

fn normalized_app_metadata_text(
    value: &plist::Value,
    key: &str,
    max_chars: usize,
) -> Option<String> {
    let raw = value.as_dictionary()?.get(key)?.as_string()?;
    let normalized = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    (!normalized.is_empty() && normalized.chars().count() <= max_chars).then_some(normalized)
}

const MAX_APP_DISK_USAGE_BYTES: u64 = 16 * 1_000_000_000_000;

fn app_disk_usage(value: &plist::Value) -> (Option<u64>, Option<u64>, Option<u64>) {
    let Some(fields) = value.as_dictionary() else {
        return (None, None, None);
    };
    let bounded = |key: &str| {
        fields
            .get(key)
            .and_then(plist::Value::as_unsigned_integer)
            .filter(|bytes| *bytes <= MAX_APP_DISK_USAGE_BYTES)
    };
    let static_bytes = bounded("StaticDiskUsage");
    let dynamic_bytes = bounded("DynamicDiskUsage");
    let total_bytes = match (static_bytes, dynamic_bytes) {
        (Some(static_bytes), Some(dynamic_bytes)) => static_bytes.checked_add(dynamic_bytes),
        (Some(bytes), None) | (None, Some(bytes)) => Some(bytes),
        (None, None) => None,
    }
    .filter(|bytes| *bytes <= MAX_APP_DISK_USAGE_BYTES);
    (static_bytes, dynamic_bytes, total_bytes)
}

fn installation_supports_documents(value: &plist::Value) -> bool {
    value.as_dictionary().is_some_and(|fields| {
        ["UIFileSharingEnabled", "UISupportsDocumentBrowser"]
            .into_iter()
            .any(|key| {
                fields
                    .get(key)
                    .and_then(plist::Value::as_boolean)
                    .unwrap_or(false)
            })
    })
}

async fn stop_device_app(
    client: &mut AppServiceClient<Box<dyn ReadWrite>>,
    bundle_id: &str,
) -> Result<bool, String> {
    let apps = client
        .list_apps(true, true, false, false, false)
        .await
        .map_err(|error| format!("unable to resolve app before stopping it: {error:?}"))?;
    let app = apps
        .into_iter()
        .find(|app| app.bundle_identifier == bundle_id)
        .ok_or_else(|| "app is not installed or is not user-manageable".to_string())?;
    let processes = client
        .list_processes()
        .await
        .map_err(|error| format!("unable to list app processes: {error:?}"))?;
    let process_ids: Vec<_> = processes
        .into_iter()
        .filter(|process| {
            process.executable_url.as_ref().is_some_and(|executable| {
                crate::app_lifecycle::process_executable_belongs_to_app(
                    &app.path,
                    &executable.relative,
                )
            })
        })
        .map(|process| process.pid)
        .collect();
    for pid in &process_ids {
        client
            .send_signal(*pid, 15)
            .await
            .map_err(|error| format!("unable to stop app: {error:?}"))?;
    }
    Ok(!process_ids.is_empty())
}

fn sort_device_apps(mut apps: Vec<DeviceApp>) -> Vec<DeviceApp> {
    apps.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.bundle_id.cmp(&right.bundle_id))
    });
    apps
}

/// How often we poll the host clipboard for host -> device changes (arboard has no
/// change notification). The device -> host direction is push-driven when available.
const CLIPBOARD_POLL: std::time::Duration = std::time::Duration::from_millis(600);
/// Max characters in the UI's clipboard-activity preview.
const CLIPBOARD_PREVIEW_LEN: usize = 48;
const CLIPBOARD_COMMAND_TIMEOUT: Duration = Duration::from_secs(8);

enum ClipboardCommand {
    SetText {
        text: String,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
}

enum ClipboardWake {
    Push(Result<PasteboardSnapshot, IdeviceError>),
    Tick,
    Command(Option<ClipboardCommand>),
}

#[derive(Clone)]
struct ClipboardBridge(Sender<ClipboardCommand>);

impl ClipboardBridge {
    async fn set_text(&self, text: String) -> Result<(), String> {
        let (reply, response) = tokio::sync::oneshot::channel();
        self.0
            .try_send(ClipboardCommand::SetText { text, reply })
            .map_err(|error| match error {
                tokio::sync::mpsc::error::TrySendError::Full(_) => {
                    "device clipboard is busy".to_string()
                }
                tokio::sync::mpsc::error::TrySendError::Closed(_) => {
                    "device clipboard is unavailable".to_string()
                }
            })?;
        tokio::time::timeout(CLIPBOARD_COMMAND_TIMEOUT, response)
            .await
            .map_err(|_| "device clipboard request timed out".to_string())?
            .map_err(|_| "device clipboard session ended".to_string())?
    }
}

/// The contents both clipboards are believed to already share, used to suppress
/// echoes and break the host⇄device feedback loop. Text and image are mutually
/// exclusive. Images are tracked by a hash of their raw RGBA bytes.
struct ClipState {
    last_text: Option<String>,
    last_image: Option<u64>,
    /// Device change counter, to ignore device snapshots that didn't change.
    last_change_count: Option<i64>,
}

/// Keep the host and device clipboards in sync (text and images), both directions.
///
/// One pasteboard connection (a second one doesn't work - the device tears down
/// the existing subscriber when a new connection issues a SET), driven by a
/// `select!`: device -> host is push-driven via `AUTONOTIFY`, host -> device is
/// polled every [`CLIPBOARD_POLL`] (which also does a fallback `PULL`).
///
/// On startup [`ClipState`] is seeded without copying anything, so connecting
/// never clobbers either clipboard. Best-effort throughout, reconnecting on socket
/// errors. Never returns (returning would tear down the session via [`run`]'s
/// `select!`); idles if the host clipboard or pasteboard service is unavailable.
async fn clipboard_task(
    pasteboard: Option<PasteboardServiceClient<Box<dyn ReadWrite>>>,
    sync_enabled: bool,
    activity: ClipboardSlot,
    mut commands: Receiver<ClipboardCommand>,
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
) {
    let Some(mut pb) = pasteboard else {
        clipboard_command_loop(None, &activity, &mut commands, adapter, handshake).await;
        return;
    };
    if !sync_enabled {
        clipboard_command_loop(Some(pb), &activity, &mut commands, adapter, handshake).await;
        return;
    }
    let mut clip = match arboard::Clipboard::new() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("no host clipboard; clipboard sync disabled: {e:?}");
            clipboard_command_loop(Some(pb), &activity, &mut commands, adapter, handshake).await;
            return;
        }
    };

    // Seed the agreed state from current host + device contents so connecting
    // doesn't push or pull pre-existing content.
    let mut state = ClipState {
        last_text: clip.get_text().ok(),
        last_image: clip.get_image().ok().map(|i| image_hash(&i.bytes)),
        last_change_count: pb
            .get(GENERAL_PASTEBOARD)
            .await
            .ok()
            .and_then(|s| s.change_count),
    };

    subscribe(&mut pb).await;

    let mut tick = tokio::time::interval(CLIPBOARD_POLL);
    let mut commands_open = true;
    loop {
        // The `recv_push` future is dropped when the tick wins - safe because the
        // XPC read path buffers partial reads. Resolve the borrow of `pb` before
        // the match body, which reuses it.
        let wake = tokio::select! {
            result = pb.recv_push() => ClipboardWake::Push(result),
            _ = tick.tick() => ClipboardWake::Tick,
            command = commands.recv(), if commands_open => ClipboardWake::Command(command),
        };

        match wake {
            // device -> host (push)
            ClipboardWake::Push(Ok(snap)) => {
                apply_device_snapshot(&snap, &mut clip, &activity, &mut state)
            }
            ClipboardWake::Push(Err(e)) => {
                tracing::warn!("clipboard PUSH failed: {e:?}");
                if let Some(c) = reconnect_pasteboard(adapter, handshake).await {
                    pb = c;
                    subscribe(&mut pb).await;
                    // Re-seed the change counter so post-reconnect state isn't
                    // mistaken for a fresh device change.
                    state.last_change_count = pb
                        .get(GENERAL_PASTEBOARD)
                        .await
                        .ok()
                        .and_then(|s| s.change_count);
                }
            }
            // poll tick
            ClipboardWake::Tick => {
                // Fallback device -> host for devices that don't push.
                match pb.get(GENERAL_PASTEBOARD).await {
                    Ok(snap) => apply_device_snapshot(&snap, &mut clip, &activity, &mut state),
                    Err(e) => {
                        tracing::warn!("clipboard PULL failed: {e:?}");
                        if let Some(c) = reconnect_pasteboard(adapter, handshake).await {
                            pb = c;
                            subscribe(&mut pb).await;
                        }
                        continue;
                    }
                }
                // Host -> device.
                if let Err(e) = push_host_clipboard(&mut pb, &mut clip, &activity, &mut state).await
                {
                    tracing::warn!("clipboard host -> device failed: {e:?}");
                    if let Some(c) = reconnect_pasteboard(adapter, handshake).await {
                        pb = c;
                        subscribe(&mut pb).await;
                    }
                }
            }
            ClipboardWake::Command(Some(command)) => {
                let prepared_text = match &command {
                    ClipboardCommand::SetText { text, .. } => text.clone(),
                };
                if execute_clipboard_command(&mut pb, &activity, command).await {
                    state.last_text = Some(prepared_text);
                    state.last_image = None;
                    state.last_change_count = pb
                        .get(GENERAL_PASTEBOARD)
                        .await
                        .ok()
                        .and_then(|snapshot| snapshot.change_count);
                } else if let Some(client) = reconnect_pasteboard(adapter, handshake).await {
                    pb = client;
                    subscribe(&mut pb).await;
                }
            }
            ClipboardWake::Command(None) => commands_open = false,
        }
    }
}

async fn clipboard_command_loop(
    mut pasteboard: Option<PasteboardServiceClient<Box<dyn ReadWrite>>>,
    activity: &ClipboardSlot,
    commands: &mut Receiver<ClipboardCommand>,
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
) {
    loop {
        let Some(command) = commands.recv().await else {
            std::future::pending::<()>().await;
            return;
        };
        if pasteboard.is_none() {
            pasteboard = reconnect_pasteboard(adapter, handshake).await;
        }
        let Some(client) = pasteboard.as_mut() else {
            reject_clipboard_command(command, "device pasteboard service is unavailable");
            continue;
        };
        if !execute_clipboard_command(client, activity, command).await {
            pasteboard = None;
        }
    }
}

async fn execute_clipboard_command(
    pasteboard: &mut PasteboardServiceClient<Box<dyn ReadWrite>>,
    activity: &ClipboardSlot,
    command: ClipboardCommand,
) -> bool {
    match command {
        ClipboardCommand::SetText { text, reply } => {
            let result = pasteboard
                .set_text(&text, GENERAL_PASTEBOARD)
                .await
                .map_err(|error| format!("unable to set device clipboard: {error:?}"));
            let succeeded = result.is_ok();
            if succeeded {
                tracing::info!(
                    bytes = text.len(),
                    "clipboard: text prepared for device paste"
                );
                activity.set(ClipboardEvent {
                    from_device: false,
                    kind: ClipboardContentKind::Text,
                    preview: clipboard_preview(&text, CLIPBOARD_PREVIEW_LEN),
                });
            }
            let _ = reply.send(result);
            succeeded
        }
    }
}

fn reject_clipboard_command(command: ClipboardCommand, reason: &str) {
    match command {
        ClipboardCommand::SetText { reply, .. } => {
            let _ = reply.send(Err(reason.into()));
        }
    }
}

/// Subscribe `pb` to device pasteboard change notifications, inlining item bytes
/// so PUSH snapshots carry text/image data directly. Best-effort.
async fn subscribe(pb: &mut PasteboardServiceClient<Box<dyn ReadWrite>>) {
    if let Err(e) = pb
        .set_change_notifications(
            true,
            GENERAL_PASTEBOARD,
            Some(DataInclusionPolicy::AllResolved),
        )
        .await
    {
        tracing::warn!("clipboard: failed to subscribe to change notifications: {e:?}");
    }
}

/// Apply a device pasteboard snapshot to the host clipboard (device -> host),
/// preferring text and falling back to an image. No-ops when the snapshot's
/// change counter hasn't advanced or its content already matches [`ClipState`].
fn apply_device_snapshot(
    snap: &PasteboardSnapshot,
    clip: &mut arboard::Clipboard,
    activity: &ClipboardSlot,
    state: &mut ClipState,
) {
    if snap.change_count == state.last_change_count {
        return; // our own SET echoing back, or a no-op notification
    }
    state.last_change_count = snap.change_count;

    if let Some(text) = snap.text() {
        if Some(&text) != state.last_text.as_ref() {
            match clip.set_text(text.clone()) {
                Ok(()) => {
                    tracing::info!("clipboard: device -> host ({} bytes text)", text.len());
                    activity.set(ClipboardEvent {
                        from_device: true,
                        kind: ClipboardContentKind::Text,
                        preview: clipboard_preview(&text, CLIPBOARD_PREVIEW_LEN),
                    });
                    state.last_text = Some(text);
                    state.last_image = None;
                }
                Err(e) => tracing::warn!("failed to set host text: {e:?}"),
            }
        }
    } else if let Some((_uti, bytes)) = snap.image() {
        match decode_image(&bytes) {
            Some(img) => {
                let (w, h) = (img.width, img.height);
                let hash = image_hash(&img.bytes);
                if Some(hash) != state.last_image {
                    match clip.set_image(img) {
                        Ok(()) => {
                            tracing::info!("clipboard: device -> host (image {w}×{h})");
                            activity.set(ClipboardEvent {
                                from_device: true,
                                kind: ClipboardContentKind::Image,
                                preview: format!("{w} x {h}"),
                            });
                            state.last_image = Some(hash);
                            state.last_text = None;
                        }
                        Err(e) => tracing::warn!("failed to set host image: {e:?}"),
                    }
                }
            }
            None => tracing::warn!("clipboard: undecodable device image, skipping"),
        }
    }
}

/// Push the host clipboard to the device (host -> device) if it changed: text
/// first, otherwise an image (re-encoded to PNG). Returns `Err` only when a
/// device SET fails, so the caller can reconnect.
async fn push_host_clipboard(
    pb: &mut PasteboardServiceClient<Box<dyn ReadWrite>>,
    clip: &mut arboard::Clipboard,
    activity: &ClipboardSlot,
    state: &mut ClipState,
) -> Result<(), IdeviceError> {
    // arboard errors on get_text when the host holds a non-text item, which we
    // treat as "no text" and fall through to the image check.
    if let Ok(text) = clip.get_text()
        && !text.is_empty()
    {
        if Some(&text) != state.last_text.as_ref() {
            pb.set_text(&text, GENERAL_PASTEBOARD).await?;
            tracing::info!("clipboard: host -> device ({} bytes text)", text.len());
            activity.set(ClipboardEvent {
                from_device: false,
                kind: ClipboardContentKind::Text,
                preview: clipboard_preview(&text, CLIPBOARD_PREVIEW_LEN),
            });
            state.last_text = Some(text);
            state.last_image = None;
            // Record the new change counter so the echoing PUSH/PULL is ignored.
            state.last_change_count = pb
                .get(GENERAL_PASTEBOARD)
                .await
                .ok()
                .and_then(|s| s.change_count);
        }
        return Ok(());
    }

    if let Ok(img) = clip.get_image() {
        let hash = image_hash(&img.bytes);
        if Some(hash) != state.last_image {
            let (w, h) = (img.width, img.height);
            match encode_png(&img) {
                Some(png) => {
                    pb.set_image(&png, UTI_PNG, GENERAL_PASTEBOARD).await?;
                    tracing::info!(
                        "clipboard: host -> device (image {w}×{h}, {} bytes png)",
                        png.len()
                    );
                    activity.set(ClipboardEvent {
                        from_device: false,
                        kind: ClipboardContentKind::Image,
                        preview: format!("{w} x {h}"),
                    });
                    state.last_image = Some(hash);
                    state.last_text = None;
                    state.last_change_count = pb
                        .get(GENERAL_PASTEBOARD)
                        .await
                        .ok()
                        .and_then(|s| s.change_count);
                }
                None => tracing::warn!("clipboard: failed to encode host image to PNG"),
            }
        }
    }
    Ok(())
}

/// Hash raw RGBA bytes for image echo suppression.
fn image_hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// Decode an encoded pasteboard image (PNG/JPEG/TIFF) into arboard's raw RGBA.
/// Returns `None` if the bytes don't decode as a supported image.
fn decode_image(bytes: &[u8]) -> Option<arboard::ImageData<'static>> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (width, height) = (img.width() as usize, img.height() as usize);
    Some(arboard::ImageData {
        width,
        height,
        bytes: std::borrow::Cow::Owned(img.into_raw()),
    })
}

/// Encode arboard's raw RGBA image into PNG bytes for the device pasteboard.
/// Returns `None` if the buffer is malformed or PNG encoding fails.
fn encode_png(img: &arboard::ImageData) -> Option<Vec<u8>> {
    let buf = image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.to_vec())?;
    let mut out = std::io::Cursor::new(Vec::new());
    buf.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// Re-establish the pasteboard service over the existing tunnel after a dropped
/// connection. Returns the new client, or `None` to let the next poll tick retry.
async fn reconnect_pasteboard(
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
) -> Option<PasteboardServiceClient<Box<dyn ReadWrite>>> {
    match tokio::time::timeout(
        Duration::from_secs(5),
        PasteboardServiceClient::connect_rsd(adapter, handshake),
    )
    .await
    {
        Ok(Ok(c)) => {
            tracing::info!("clipboard: reconnected pasteboard service");
            Some(c)
        }
        Ok(Err(e)) => {
            tracing::warn!("clipboard reconnect failed: {e:?}");
            None
        }
        Err(_) => {
            tracing::warn!("clipboard reconnect timed out");
            None
        }
    }
}

/// Dispatch one [`InputCmd`] to the appropriate HID surface.
async fn dispatch(
    touch: &mut UniversalHidClient<Box<dyn ReadWrite>>,
    indigo: &mut IndigoHidClient<Box<dyn ReadWrite>>,
    orientation: &mut Option<OrientationServiceClient<Box<dyn ReadWrite>>>,
    orientation_view: &OrientationSlot,
    cmd: InputCmd,
) -> Result<(), idevice::IdeviceError> {
    match cmd {
        InputCmd::Tap { x, y } => touch.tap(x, y).await,
        InputCmd::TouchDown { x, y } | InputCmd::TouchMove { x, y } => {
            touch
                .send_touchscreen(TOUCHSCREEN_STATE_CONTACT, x, y, None)
                .await
        }
        InputCmd::TouchUp { x, y } => {
            touch
                .send_touchscreen(TOUCHSCREEN_STATE_RELEASE, x, y, None)
                .await
        }
        InputCmd::MultiTouchFrame(contacts) => match build_multitouch_report(&contacts, None) {
            Ok(report) => {
                touch
                    .send_report(DIGITIZER_SURFACE_MAIN_TOUCHSCREEN, report)
                    .await
            }
            Err(error) => {
                tracing::warn!("dropping invalid multi-touch frame: {error}");
                Ok(())
            }
        },
        InputCmd::Text(text) => {
            for ch in text.chars() {
                if let Some((usage, shift)) = ascii_to_usage(ch) {
                    type_key(
                        indigo,
                        usage,
                        KeyMods {
                            shift,
                            ..KeyMods::default()
                        },
                    )
                    .await?;
                }
            }
            Ok(())
        }
        InputCmd::PasteText { .. } => Ok(()),
        InputCmd::KeyUsage(usage) => type_key(indigo, usage, KeyMods::default()).await,
        InputCmd::KeyCombo { usage, mods } => type_key(indigo, usage, mods).await,
        InputCmd::KeyboardDown(usage) => indigo.send_keyboard(usage, ButtonState::Down).await,
        InputCmd::KeyboardUp(usage) => indigo.send_keyboard(usage, ButtonState::Up).await,
        InputCmd::Button(name) => {
            if let Some(&(_, page, code, hold_ms)) =
                NAMED_BUTTONS.iter().find(|(n, _, _, _)| *n == name)
            {
                indigo.send_button(page, code, ButtonState::Down).await?;
                tokio::time::sleep(std::time::Duration::from_millis(hold_ms)).await;
                indigo.send_button(page, code, ButtonState::Up).await?;
            }
            Ok(())
        }
        InputCmd::ButtonDown(name) => {
            if let Some(&(_, page, code, _)) = NAMED_BUTTONS.iter().find(|(n, _, _, _)| *n == name)
            {
                indigo.send_button(page, code, ButtonState::Down).await?;
            }
            Ok(())
        }
        InputCmd::ButtonUp(name) => {
            if let Some(&(_, page, code, _)) = NAMED_BUTTONS.iter().find(|(n, _, _, _)| *n == name)
            {
                indigo.send_button(page, code, ButtonState::Up).await?;
            }
            Ok(())
        }
        InputCmd::Rotate(dir) => {
            if let Some(client) = orientation {
                let direction = match dir {
                    RotateDir::Left => RotationDirection::Left,
                    RotateDir::Right => RotationDirection::Right,
                };
                let state = client.rotate(direction).await?;
                tracing::info!(
                    "rotated {dir:?} -> {:?} (non-flat {:?})",
                    state.orientation,
                    state.non_flat_orientation,
                );
                // Use the non-flat orientation so the display stays sensible even
                // when the device is lying face up/down.
                let view = match state.non_flat_orientation {
                    DevOrientation::Portrait => Some(Orientation::Portrait),
                    DevOrientation::PortraitUpsideDown => Some(Orientation::PortraitUpsideDown),
                    DevOrientation::LandscapeLeft => Some(Orientation::LandscapeLeft),
                    DevOrientation::LandscapeRight => Some(Orientation::LandscapeRight),
                    DevOrientation::FaceUp
                    | DevOrientation::FaceDown
                    | DevOrientation::Unknown(_) => None,
                };
                if let Some(view) = view {
                    orientation_view.set(view);
                }
            } else {
                tracing::warn!("rotate requested but orientation service unavailable");
            }
            Ok(())
        }
        InputCmd::GetDeviceDetails(_)
        | InputCmd::RenameDevice { .. }
        | InputCmd::DeveloperMode(_)
        | InputCmd::ListApps { .. }
        | InputCmd::ListCompanionDevices(_)
        | InputCmd::GetHomeScreenLayout(_)
        | InputCmd::RunningProcess(_)
        | InputCmd::AppLifecycle(_)
        | InputCmd::WdaAutomation(_)
        | InputCmd::WdaRunner(_)
        | InputCmd::AppConsole(_)
        | InputCmd::GetAppIcon { .. }
        | InputCmd::TakeScreenshot(_)
        | InputCmd::NetworkCapture(_)
        | InputCmd::BluetoothCapture(_)
        | InputCmd::DeviceBackup(_)
        | InputCmd::Sysdiagnose(_)
        | InputCmd::LogArchive(_)
        | InputCmd::DeveloperImageMount(_)
        | InputCmd::DeviceCondition(_)
        | InputCmd::AppDocuments(_)
        | InputCmd::DeviceFiles(_)
        | InputCmd::LockDevice(_)
        | InputCmd::RestartDevice(_)
        | InputCmd::ShutdownDevice(_)
        | InputCmd::Provisioning(_)
        | InputCmd::LaunchApp { .. }
        | InputCmd::StopApp { .. }
        | InputCmd::ListCrashReports(_)
        | InputCmd::ReadCrashReport { .. }
        | InputCmd::ExportCrashReport { .. }
        | InputCmd::DeleteCrashReport { .. }
        | InputCmd::PreflightApp { .. }
        | InputCmd::InstallApp { .. }
        | InputCmd::UpgradeApp { .. }
        | InputCmd::UninstallApp { .. }
        | InputCmd::SetLocation { .. }
        | InputCmd::ClearLocation { .. } => Ok(()),
        InputCmd::Shutdown => Ok(()),
    }
}

/// Press a key (down then up), bracketing with any held modifier keys. Modifiers
/// are pressed in a stable order and released in reverse so iOS reads a clean
/// chord (e.g. ⌘C, ⌘Space).
async fn type_key(
    indigo: &mut IndigoHidClient<Box<dyn ReadWrite>>,
    usage: u64,
    mods: KeyMods,
) -> Result<(), idevice::IdeviceError> {
    // (usage, held) pairs in press order; release walks this in reverse.
    let modifiers = [
        (KEY_LEFT_CTRL, mods.ctrl),
        (KEY_LEFT_ALT, mods.alt),
        (KEY_LEFT_CMD, mods.cmd),
        (KEY_LEFT_SHIFT, mods.shift),
    ];
    for (m, held) in modifiers {
        if held {
            indigo.send_keyboard(m, ButtonState::Down).await?;
        }
    }
    indigo.send_keyboard(usage, ButtonState::Down).await?;
    indigo.send_keyboard(usage, ButtonState::Up).await?;
    for (m, held) in modifiers.iter().rev() {
        if *held {
            indigo.send_keyboard(*m, ButtonState::Up).await?;
        }
    }
    // A small gap so the device registers discrete keystrokes.
    tokio::time::sleep(std::time::Duration::from_millis(12)).await;
    Ok(())
}

/// Pump video RTP into ffmpeg: receive datagrams, depacketize HEVC, hand the
/// resulting Annex-B to the ffmpeg writer. This socket also carries inbound RTCP
/// under rtcp-mux; those datagrams are split off to [`RtcpShared::note_inbound`].
async fn video_task(
    udp: Arc<UdpSocketHandle>,
    hevc_queue: Arc<HevcQueue>,
    rtcp: Arc<Mutex<RtcpShared>>,
    corruption: Arc<Notify>,
    video_counters: VideoCounters,
    our_ssrc: u32,
) {
    let mut depacketizer = HevcDepacketizer::new();
    let mut assembler = AccessUnitAssembler::default();
    // Lock onto a single RTP stream (SSRC) and feed only its packets to the
    // depacketizer. A stream restart begins a new SSRC with a fresh sequence
    // number; the device doesn't reliably stop the old sender, so both streams can
    // arrive interleaved. Migrate only once the locked stream has gone quiet for
    // `SSRC_TAKEOVER_GRACE` (the old sender really stopped); ignore stray packets
    // from a competing/leaked SSRC otherwise.
    let mut locked_ssrc: Option<u32> = None;
    let mut last_locked = Instant::now();

    // Per-frame ACK is DISABLED by default - it corrupts the stream. Sending
    // AVConference's `0x00000005` APP ack (even byte-identical to Apple) makes the
    // encoder's reference diverge from our decoder under motion and never heal.
    // `DEVICEHUB_FRAME_ACK=1` re-enables it for experiments.
    let send_frame_ack = std::env::var("DEVICEHUB_FRAME_ACK").is_ok();
    // Per-access-unit completeness tracking: ACK a frame only if it arrived intact
    // (packets since the previous marker == sequence span), never vouching for a gap.
    let mut prev_marker_seq: Option<u16> = None;
    let mut au_pkts: u32 = 0;
    let mut metrics_started = Instant::now();
    let mut metrics_rtp_packets = 0_u64;
    let mut metrics_rtp_bytes = 0_u64;
    let mut metrics_access_units = 0_u64;
    let mut metrics_hevc_bytes = 0_u64;
    let mut metrics_incomplete_markers = 0_u64;
    let mut last_rtp_frame_timestamp = None;
    let mut last_source_frame_at = None;
    let mut rtp_timestamp_deltas = RunningStats::default();
    let mut source_frame_intervals_ms = RunningStats::default();

    // DIAGNOSTIC: if `DEVICEHUB_DUMP_HEVC` is set, tee the Annex-B bytes we feed
    // ffmpeg to that path for offline decoding.
    let mut dump = match std::env::var("DEVICEHUB_DUMP_HEVC") {
        Ok(path) => match tokio::fs::File::create(&path).await {
            Ok(f) => {
                tracing::info!("dumping HEVC elementary stream to {path}");
                Some(f)
            }
            Err(e) => {
                tracing::warn!("could not open HEVC dump {path}: {e}");
                None
            }
        },
        Err(_) => None,
    };

    loop {
        match udp.recv().await {
            Ok(dg) => {
                let now = Instant::now();
                // rtcp-mux: RTCP shares this port; never goes through the depacketizer.
                if is_rtcp(&dg.data) {
                    rtcp.lock()
                        .unwrap()
                        .note_inbound(&dg.data, dg.source_port, false, now);
                    continue;
                }
                let Some(pkt) = RtpPacket::parse(&dg.data) else {
                    continue;
                };
                // DIAGNOSTIC: log when a keyframe (IRAP slice) starts arriving.
                {
                    let p = pkt.payload;
                    let irap = if p.len() >= 3 && (p[0] >> 1) & 0x3f == 49 {
                        // FU: only the start fragment, with an IRAP fu-type.
                        (p[2] & 0x80) != 0 && (16..=23).contains(&(p[2] & 0x3f))
                    } else if p.len() >= 2 {
                        (16..=23).contains(&((p[0] >> 1) & 0x3f))
                    } else {
                        false
                    };
                    if irap {
                        tracing::info!("received IRAP keyframe (ssrc {:#x})", pkt.ssrc);
                    }
                }
                match locked_ssrc {
                    Some(s) if s == pkt.ssrc => last_locked = now,
                    Some(s) => {
                        // Competing stream: migrate only once the locked one has
                        // gone silent (old sender stopped).
                        if now.duration_since(last_locked) < SSRC_TAKEOVER_GRACE {
                            continue;
                        }
                        tracing::info!(
                            "RTP stream {s:#x} went quiet; migrating to {:#x}",
                            pkt.ssrc,
                        );
                        depacketizer = HevcDepacketizer::new();
                        assembler.clear();
                        prev_marker_seq = None;
                        au_pkts = 0;
                        last_rtp_frame_timestamp = None;
                        last_source_frame_at = None;
                        rtp_timestamp_deltas = RunningStats::default();
                        source_frame_intervals_ms = RunningStats::default();
                        let (dropped_access_units, dropped_bytes) = hevc_queue.force_resync();
                        tracing::info!(
                            dropped_access_units,
                            dropped_bytes,
                            "cleared HEVC queue for RTP stream migration"
                        );
                        locked_ssrc = Some(pkt.ssrc);
                        last_locked = now;
                        let mut s = rtcp.lock().unwrap();
                        s.media_ssrc = Some(pkt.ssrc);
                        s.stats = RtpStats::default();
                    }
                    None => {
                        locked_ssrc = Some(pkt.ssrc);
                        last_locked = now;
                    }
                }
                metrics_rtp_packets += 1;
                metrics_rtp_bytes += dg.data.len() as u64;
                {
                    let mut s = rtcp.lock().unwrap();
                    s.media_ssrc.get_or_insert(pkt.ssrc);
                    s.stats.on_packet(pkt.sequence_number);
                    if pkt.marker {
                        s.frames = s.frames.wrapping_add(1);
                    }
                }
                // The marker bit ends an access unit. Track packet completeness
                // even when experimental frame ACKs are disabled: a complete
                // marker lets us hand the AU to ffmpeg without waiting for the
                // following frame's AUD. An early/out-of-order marker does not.
                let belongs_to_current_au = prev_marker_seq.is_none_or(|previous| {
                    let distance = pkt.sequence_number.wrapping_sub(previous);
                    distance != 0 && distance < 0x8000
                });
                if belongs_to_current_au {
                    au_pkts = au_pkts.wrapping_add(1);
                }
                let complete_access_unit = if pkt.marker {
                    video_counters.note_source_frame();
                    if let Some(previous) = last_rtp_frame_timestamp {
                        let delta = pkt.timestamp.wrapping_sub(previous);
                        if delta > 0 && delta <= 1_000_000 {
                            rtp_timestamp_deltas.push(delta as f64);
                        }
                    }
                    last_rtp_frame_timestamp = Some(pkt.timestamp);
                    if let Some(previous) = last_source_frame_at {
                        source_frame_intervals_ms
                            .push(now.duration_since(previous).as_secs_f64() * 1000.0);
                    }
                    last_source_frame_at = Some(now);
                    let complete = match prev_marker_seq {
                        Some(prev) => {
                            let expected = pkt.sequence_number.wrapping_sub(prev) as u32;
                            au_pkts >= expected
                        }
                        None => true,
                    };
                    if send_frame_ack && complete {
                        let ack = build_frame_ack(our_ssrc, pkt.timestamp);
                        udp.send_to(dg.source_port, ack).await.ok();
                    }
                    prev_marker_seq = Some(pkt.sequence_number);
                    au_pkts = 0;
                    if !complete {
                        metrics_incomplete_markers += 1;
                    }
                    complete
                } else {
                    false
                };
                depacketizer.push(pkt.sequence_number, pkt.timestamp, pkt.payload);
                let out = depacketizer.take_output();
                if !out.is_empty() {
                    if let Some(f) = &mut dump {
                        f.write_all(&out).await.ok();
                    }
                    let mut access_units = assembler.push(&out, pkt.timestamp);
                    if complete_access_unit && let Some(access_unit) = assembler.finish() {
                        access_units.push(access_unit);
                    }
                    for access_unit in access_units {
                        metrics_access_units += 1;
                        metrics_hevc_bytes += access_unit.bytes.len() as u64;
                        match hevc_queue.push(access_unit) {
                            HevcQueuePush::Enqueued | HevcQueuePush::Dropped => {}
                            HevcQueuePush::NeedsKeyframe {
                                queued_bytes,
                                incoming_bytes,
                            } => {
                                tracing::warn!(
                                    queue_limit_bytes = HEVC_QUEUE_MAX_BYTES,
                                    queued_bytes,
                                    incoming_bytes,
                                    "HEVC queue overflow; dropping until IRAP"
                                );
                                corruption.notify_one();
                            }
                            HevcQueuePush::Recovered {
                                dropped_access_units,
                                dropped_bytes,
                            } => {
                                tracing::info!(
                                    dropped_access_units,
                                    dropped_bytes,
                                    "HEVC queue resumed at IRAP"
                                );
                            }
                        }
                    }
                }
                if metrics_started.elapsed() >= Duration::from_secs(5) {
                    let elapsed_ms = metrics_started.elapsed().as_millis() as u64;
                    let queue = hevc_queue.take_snapshot();
                    let source_fps = source_frame_intervals_ms
                        .mean()
                        .filter(|interval| *interval > 0.0)
                        .map(|interval| 1000.0 / interval);
                    tracing::debug!(
                        target: "devicehub_mask::perf",
                        elapsed_ms,
                        rtp_packets = metrics_rtp_packets,
                        rtp_bytes = metrics_rtp_bytes,
                        access_units = metrics_access_units,
                        hevc_bytes = metrics_hevc_bytes,
                        incomplete_markers = metrics_incomplete_markers,
                        ?source_fps,
                        source_frame_interval_ms = ?source_frame_intervals_ms.mean(),
                        source_frame_interval_min_ms = ?source_frame_intervals_ms.min(),
                        source_frame_interval_max_ms = ?source_frame_intervals_ms.max(),
                        source_frame_jitter_ms = ?source_frame_intervals_ms.standard_deviation(),
                        rtp_timestamp_delta_ticks = ?rtp_timestamp_deltas.mean(),
                        rtp_timestamp_delta_min_ticks = ?rtp_timestamp_deltas.min(),
                        rtp_timestamp_delta_max_ticks = ?rtp_timestamp_deltas.max(),
                        queue_access_units = queue.queued_access_units,
                        queue_bytes = queue.queued_bytes,
                        queue_peak_bytes = queue.peak_bytes,
                        waiting_for_irap = queue.waiting_for_irap,
                        queue_wait_ms = queue.wait_ms,
                        queue_wait_max_ms = queue.wait_max_ms,
                        "video input performance"
                    );
                    metrics_started = Instant::now();
                    metrics_rtp_packets = 0;
                    metrics_rtp_bytes = 0;
                    metrics_access_units = 0;
                    metrics_hevc_bytes = 0;
                    metrics_incomplete_markers = 0;
                    rtp_timestamp_deltas = RunningStats::default();
                    source_frame_intervals_ms = RunningStats::default();
                }
            }
            Err(e) => {
                tracing::warn!("video udp recv error: {e:?}");
                break;
            }
        }
    }
    hevc_queue.close();
}

/// Drain depacketized Annex-B from [`video_task`] into ffmpeg's stdin. On its own
/// task so ffmpeg backpressure never stalls the RTP receive loop's RTCP ACKs.
async fn ffmpeg_writer(mut ffmpeg_in: ChildStdin, hevc_queue: Arc<HevcQueue>) {
    while let Some(access_unit) = hevc_queue.pop().await {
        if ffmpeg_in.write_all(&access_unit.bytes).await.is_err() {
            tracing::info!("ffmpeg stdin closed; ending writer");
            break;
        }
    }
}

async fn browser_video_writer(
    hevc_queue: Arc<HevcQueue>,
    frames: crate::browser_video::BrowserVideoSlot,
    counters: VideoCounters,
    frame_beat: Arc<Notify>,
    corruption: Arc<Notify>,
) {
    let mut dimensions = None;
    let mut clock = RtpVideoClock::default();
    while let Some(access_unit) = hevc_queue.pop().await {
        if (dimensions.is_none() || access_unit.is_irap)
            && let Some(parsed) = crate::browser_video::hevc_dimensions(&access_unit.bytes)
        {
            dimensions = Some(parsed);
        }
        let Some((width, height)) = dimensions else {
            if access_unit.is_irap {
                tracing::warn!("browser video keyframe did not contain a readable HEVC SPS");
                corruption.notify_one();
            }
            continue;
        };
        counters.note_decoded_frame();
        frame_beat.notify_one();
        frames.publish(
            clock.timestamp_us(access_unit.rtp_timestamp),
            access_unit.is_irap,
            width,
            height,
            access_unit.bytes,
        );
    }
}

async fn forward_browser_keyframes(
    frames: crate::browser_video::BrowserVideoSlot,
    corruption: Arc<Notify>,
) {
    loop {
        frames.keyframe_requested().await;
        corruption.notify_one();
    }
}

/// Receive inbound RTCP on the dedicated RTCP socket (non-mux case). Records
/// Sender Reports in the shared state. Idles forever if no separate socket bound.
async fn rtcp_recv_task(udp: Option<Arc<UdpSocketHandle>>, rtcp: Arc<Mutex<RtcpShared>>) {
    let Some(udp) = udp else {
        std::future::pending::<()>().await;
        return;
    };
    loop {
        match udp.recv().await {
            Ok(dg) => {
                if is_rtcp(&dg.data) {
                    rtcp.lock().unwrap().note_inbound(
                        &dg.data,
                        dg.source_port,
                        true,
                        Instant::now(),
                    );
                }
            }
            Err(e) => {
                tracing::warn!("rtcp udp recv error: {e:?}");
                break;
            }
        }
    }
}

/// The RTCP control loop. Periodically sends a Receiver Report + SDES (liveness),
/// and on `corruption` a keyframe request (RR + SDES + PLI + FIR) for a fresh IDR.
/// Replies go wherever inbound RTCP was observed (auto-detected mux vs. separate).
async fn rtcp_send_task(
    rtp_udp: Arc<UdpSocketHandle>,
    rtcp_udp: Option<Arc<UdpSocketHandle>>,
    rtcp: Arc<Mutex<RtcpShared>>,
    our_ssrc: u32,
    cname: String,
    corruption: &Notify,
) {
    let send = |peer: RtcpPeer, pkt: Vec<u8>| {
        let rtp_udp = rtp_udp.clone();
        let rtcp_udp = rtcp_udp.clone();
        async move {
            match peer {
                RtcpPeer::Mux(port) => {
                    rtp_udp.send_to(port, pkt).await.ok();
                }
                RtcpPeer::Separate(port) => {
                    if let Some(s) = &rtcp_udp {
                        s.send_to(port, pkt).await.ok();
                    }
                }
                RtcpPeer::Unknown => {
                    // No inbound RTCP seen yet: cover both conventions (mux -> RTP
                    // sender port; separate -> +1).
                    rtp_udp.send_to(VIDEO_SENDER_PORT, pkt.clone()).await.ok();
                    if let Some(s) = &rtcp_udp {
                        s.send_to(VIDEO_SENDER_PORT + 1, pkt).await.ok();
                    }
                }
            }
        }
    };

    let mut fir_seq: u8 = 0;
    let start = Instant::now();
    // RCTL feedback is DISABLED by default - like the per-frame ACK it desyncs the
    // encoder and corrupts the picture (and isn't yet byte-correct). `DEVICEHUB_RCTL=1`
    // re-enables it. Separate intervals so neither tick resets the other.
    let send_rctl = std::env::var("DEVICEHUB_RCTL").is_ok();
    let mut rr_tick = tokio::time::interval(RTCP_REPORT_INTERVAL);
    let mut rctl_tick = tokio::time::interval(std::time::Duration::from_millis(50));
    loop {
        tokio::select! {
            _ = rctl_tick.tick() => {
                if !send_rctl {
                    continue;
                }
                let (peer, pkt) = {
                    let s = rtcp.lock().unwrap();
                    if s.media_ssrc.is_none() {
                        continue; // no stream yet
                    }
                    let clock_ms = start.elapsed().as_millis() as u16;
                    let frames = s.frames as u16;
                    let pkt = build_rctl(our_ssrc, clock_ms, frames, s.highest_seq_rel());
                    (s.peer, pkt)
                };
                send(peer, pkt).await;
            }
            _ = rr_tick.tick() => {
                let (peer, pkt) = {
                    let mut s = rtcp.lock().unwrap();
                    let blocks = s.report_blocks(Instant::now());
                    (s.peer, build_liveness(our_ssrc, &cname, &blocks))
                };
                send(peer, pkt).await;
            }
            _ = corruption.notified() => {
                let built = {
                    let mut s = rtcp.lock().unwrap();
                    match s.media_ssrc {
                        Some(media_ssrc) => {
                            let blocks = s.report_blocks(Instant::now());
                            fir_seq = fir_seq.wrapping_add(1);
                            Some((s.peer, build_keyframe_request(
                                our_ssrc, &cname, media_ssrc, &blocks, fir_seq,
                            )))
                        }
                        // No stream locked yet - nothing to ask a keyframe of.
                        None => None,
                    }
                };
                if let Some((peer, pkt)) = built {
                    tracing::info!("requesting keyframe via RTCP (PLI+FIR)");
                    send(peer, pkt).await;
                }
                // Coalesce a burst of decode errors; let the fresh IDR arrive first.
                tokio::time::sleep(KEYFRAME_DEBOUNCE).await;
            }
        }
    }
}

/// An active screen media stream and the UDP sockets the device sends RTP to.
struct ScreenMediaStream {
    client: DisplayServiceClient<Box<dyn ReadWrite>>,
    audio_udp: UdpSocketHandle,
    video_udp: UdpSocketHandle,
    /// Video RTCP socket at `video_udp`'s port + 1 (RFC 3550). `None` if that port
    /// was unavailable, in which case we rely on rtcp-mux.
    rtcp_udp: Option<UdpSocketHandle>,
}

async fn read_device_details(
    provider: &dyn IdeviceProvider,
    requested_udid: String,
) -> Option<DeviceDetails> {
    let mut lockdown = LockdownClient::connect(provider).await.ok()?;
    let values = lockdown.get_value(None, None).await.ok()?;
    let values = values.as_dictionary()?;
    let integer = |key: &str| values.get(key).and_then(plist::Value::as_unsigned_integer);
    let disk_usage = lockdown
        .get_value(None, Some("com.apple.disk_usage"))
        .await
        .ok()
        .and_then(plist::Value::into_dictionary);
    let storage = disk_usage.as_ref().and_then(device_storage_from_disk_usage);
    let mut total_disk_capacity = disk_usage
        .as_ref()
        .and_then(|values| values.get("TotalDiskCapacity"))
        .and_then(plist::Value::as_unsigned_integer)
        .or_else(|| integer("TotalDiskCapacity"));
    if total_disk_capacity.is_none() {
        total_disk_capacity = lockdown
            .get_value(Some("TotalDiskCapacity"), Some("com.apple.disk_usage"))
            .await
            .ok()
            .and_then(|value| value.as_unsigned_integer());
    }
    Some(DeviceDetails {
        udid: device_identity_token(values, "UniqueDeviceID", 128).unwrap_or(requested_udid),
        name: device_display_name(values).unwrap_or_else(|| "iOS Device".to_string()),
        product_type: device_identity_token(values, "ProductType", 32)
            .unwrap_or_else(|| "Unknown".to_string()),
        product_version: device_identity_token(values, "ProductVersion", 32)
            .unwrap_or_else(|| "Unknown".to_string()),
        build_version: device_identity_token(values, "BuildVersion", 32),
        device_class: device_identity_token(values, "DeviceClass", 32),
        cpu_architecture: device_identity_token(values, "CPUArchitecture", 32),
        model_number: device_identity_token(values, "ModelNumber", 32),
        hardware_model: device_identity_token(values, "HardwareModel", 32),
        device_color: device_identity_token(values, "DeviceColor", 32),
        enclosure_color: device_identity_token(values, "EnclosureColor", 32),
        serial_number: device_identity_token(values, "SerialNumber", 64),
        ecid: integer("UniqueChipID").map(|value| value.to_string()),
        total_disk_capacity,
        storage,
        activation_state: None,
        developer_mode_enabled: None,
        developer_image_mounted: None,
        regional_settings: device_regional_settings(values),
        battery: None,
    })
}

fn device_display_name(values: &plist::Dictionary) -> Option<String> {
    let value = values.get("DeviceName")?.as_string()?.trim();
    let characters = value.chars().count();
    (!value.is_empty()
        && value.len() <= 255
        && characters <= 64
        && !value.chars().any(char::is_control))
    .then(|| value.to_string())
}

fn device_identity_token(
    values: &plist::Dictionary,
    key: &str,
    max_characters: usize,
) -> Option<String> {
    let value = values.get(key)?.as_string()?.trim();
    (!value.is_empty()
        && value.len() <= 128
        && value.chars().count() <= max_characters
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '-' | '_' | '.' | '#' | '/' | ',')
        }))
    .then(|| value.to_string())
}

fn device_regional_settings(values: &plist::Dictionary) -> Option<DeviceRegionalSettings> {
    let token = |key: &str, max_chars: usize, allowed: fn(char) -> bool| {
        values
            .get(key)
            .and_then(plist::Value::as_string)
            .map(str::trim)
            .filter(|value| {
                !value.is_empty()
                    && value.chars().count() <= max_chars
                    && value.chars().all(allowed)
            })
            .map(ToOwned::to_owned)
    };
    let regional = DeviceRegionalSettings {
        language: token("Language", 35, |character| {
            character.is_ascii_alphanumeric() || character == '-'
        }),
        locale: token("Locale", 64, |character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
        }),
        time_zone: token("TimeZone", 64, |character| {
            character.is_ascii_alphanumeric() || matches!(character, '/' | '_' | '-' | '+' | '.')
        }),
        uses_24_hour_clock: values
            .get("Uses24HourClock")
            .and_then(plist::Value::as_boolean),
    };
    (regional.language.is_some()
        || regional.locale.is_some()
        || regional.time_zone.is_some()
        || regional.uses_24_hour_clock.is_some())
    .then_some(regional)
}

async fn rename_device(
    provider: &dyn IdeviceProvider,
    requested_name: &str,
) -> Result<String, String> {
    let name = crate::protocol::validate_device_name(requested_name).map_err(str::to_string)?;
    let mut lockdown = LockdownClient::connect(provider)
        .await
        .map_err(|error| format!("cannot connect Lockdown for device rename: {error}"))?;
    let pairing_file = provider
        .get_pairing_file()
        .await
        .map_err(|error| format!("cannot load pairing record for device rename: {error}"))?;
    lockdown
        .start_session(&pairing_file)
        .await
        .map_err(|error| format!("cannot start Lockdown session for device rename: {error}"))?;
    let rename_result: Result<(), String> = async {
        lockdown
            .set_value("DeviceName", plist::Value::String(name.clone()), None)
            .await
            .map_err(|error| format!("device rejected the new name: {error}"))?;
        let verified = lockdown
            .get_value(Some("DeviceName"), None)
            .await
            .map_err(|error| format!("cannot verify the new device name: {error}"))?
            .into_string()
            .ok_or_else(|| "device returned an invalid name after rename".to_string())?;
        if verified != name {
            return Err("device did not retain the requested name".into());
        }
        Ok(())
    }
    .await;
    match tokio::time::timeout(Duration::from_secs(1), lockdown.stop_session()).await {
        Ok(Ok(())) => tracing::debug!("device rename Lockdown session stopped"),
        Ok(Err(error)) => {
            tracing::warn!(%error, "unable to stop device rename Lockdown session")
        }
        Err(_) => tracing::warn!("stopping device rename Lockdown session timed out"),
    }
    rename_result?;
    tracing::info!(
        name_chars = name.chars().count(),
        "device name changed through Lockdown"
    );
    Ok(name)
}

async fn read_activation_state(
    provider: &dyn IdeviceProvider,
) -> Result<DeviceActivationState, String> {
    let raw = MobileActivationdClient::new(provider)
        .state()
        .await
        .map_err(|error| format!("cannot read activation state: {error:?}"))?;
    Ok(normalize_activation_state(&raw))
}

fn normalize_activation_state(value: &str) -> DeviceActivationState {
    match value.trim().to_ascii_lowercase().as_str() {
        "activated" => DeviceActivationState::Activated,
        "unactivated" => DeviceActivationState::Unactivated,
        "factoryactivated" | "factory_activated" => DeviceActivationState::FactoryActivated,
        "softactivated" | "soft_activated" => DeviceActivationState::SoftActivated,
        _ => DeviceActivationState::Unknown,
    }
}

fn device_storage_from_disk_usage(values: &plist::Dictionary) -> Option<DeviceStorage> {
    let unsigned = |key: &str| values.get(key).and_then(plist::Value::as_unsigned_integer);
    let storage = DeviceStorage {
        data_capacity_bytes: unsigned("TotalDataCapacity"),
        data_available_bytes: unsigned("TotalDataAvailable"),
        system_capacity_bytes: unsigned("TotalSystemCapacity"),
        system_available_bytes: unsigned("TotalSystemAvailable"),
    };
    if storage.data_capacity_bytes.is_none()
        && storage.data_available_bytes.is_none()
        && storage.system_capacity_bytes.is_none()
        && storage.system_available_bytes.is_none()
    {
        None
    } else {
        Some(storage)
    }
}

async fn read_developer_mode_status(provider: &dyn IdeviceProvider) -> Result<bool, String> {
    match tokio::time::timeout(
        Duration::from_millis(1_500),
        developer_mode::read_status(provider),
    )
    .await
    {
        Ok(Ok(enabled)) => return Ok(enabled),
        Ok(Err(error)) => {
            tracing::debug!(%error, "AMFI developer mode status unavailable; falling back to MobileImageMounter");
        }
        Err(_) => tracing::debug!(
            "AMFI developer mode status timed out; falling back to MobileImageMounter"
        ),
    }
    let mut mounter = ImageMounter::connect(provider)
        .await
        .map_err(|error| format!("cannot connect mobile image mounter: {error:?}"))?;
    mounter
        .query_developer_mode_status()
        .await
        .map_err(|error| format!("cannot query developer mode: {error:?}"))
}

async fn read_device_battery(provider: &dyn IdeviceProvider) -> Result<DeviceBattery, String> {
    let mut diagnostics = DiagnosticsRelayClient::connect(provider)
        .await
        .map_err(|error| format!("cannot connect diagnostics relay: {error:?}"))?;
    let values = diagnostics
        .ioregistry(None, Some("AppleSmartBattery"), None)
        .await
        .map_err(|error| format!("cannot query AppleSmartBattery: {error:?}"))?
        .ok_or_else(|| "AppleSmartBattery returned no data".to_string())?;
    Ok(device_battery_from_ioregistry(&values))
}

fn device_battery_from_ioregistry(values: &plist::Dictionary) -> DeviceBattery {
    let unsigned = |dictionary: &plist::Dictionary, key: &str, maximum: u64| {
        dictionary
            .get(key)
            .and_then(plist::Value::as_unsigned_integer)
            .filter(|value| *value <= maximum)
    };
    let signed = |dictionary: &plist::Dictionary, key: &str, absolute_maximum: i64| {
        dictionary
            .get(key)
            .and_then(plist::Value::as_signed_integer)
            .filter(|value| value.unsigned_abs() <= absolute_maximum as u64)
    };
    let boolean = |dictionary: &plist::Dictionary, key: &str| {
        dictionary.get(key).and_then(|value| {
            value
                .as_boolean()
                .or_else(|| value.as_unsigned_integer().map(|value| value != 0))
        })
    };
    let battery_data = values
        .get("BatteryData")
        .and_then(plist::Value::as_dictionary);
    let adapter = values
        .get("AdapterDetails")
        .and_then(plist::Value::as_dictionary);
    let charger_data = values
        .get("ChargerData")
        .and_then(plist::Value::as_dictionary);
    let design_capacity_mah =
        battery_data.and_then(|data| unsigned(data, "DesignCapacity", 100_000));
    let full_charge_capacity_mah =
        battery_data.and_then(|data| unsigned(data, "FullChargeCapacity", 100_000));
    let health_percent = unsigned(values, "MaximumCapacityPercent", 100)
        .or_else(|| battery_data.and_then(|data| unsigned(data, "MaximumCapacityPercent", 100)))
        .map(|value| value as f64)
        .or_else(|| {
            design_capacity_mah
                .filter(|capacity| *capacity > 0)
                .zip(full_charge_capacity_mah)
                .map(|(design, full)| (full as f64 * 100.0 / design as f64).clamp(0.0, 100.0))
        });
    let temperature_celsius = signed(values, "Temperature", 8_000)
        .or_else(|| signed(values, "BatteryTemperature", 8_000))
        .or_else(|| battery_data.and_then(|data| signed(data, "Temperature", 8_000)))
        .map(|value| value as f64 / 100.0)
        .filter(|value| (-20.0..=80.0).contains(value));

    DeviceBattery {
        level_percent: unsigned(values, "CurrentCapacity", 100)
            .or_else(|| battery_data.and_then(|data| unsigned(data, "CurrentCapacity", 100)))
            .map(|value| value as u8),
        temperature_celsius,
        is_charging: boolean(values, "IsCharging")
            .or_else(|| charger_data.and_then(|data| boolean(data, "IsCharging"))),
        external_connected: boolean(values, "ExternalConnected")
            .or_else(|| boolean(values, "AppleRawExternalConnected")),
        fully_charged: boolean(values, "FullyCharged")
            .or_else(|| battery_data.and_then(|data| boolean(data, "FullyCharged"))),
        cycle_count: unsigned(values, "CycleCount", 100_000),
        voltage_mv: unsigned(values, "Voltage", 30_000)
            .or_else(|| unsigned(values, "AppleRawBatteryVoltage", 30_000)),
        instant_amperage_ma: signed(values, "InstantAmperage", 100_000)
            .or_else(|| signed(values, "Amperage", 100_000)),
        design_capacity_mah,
        full_charge_capacity_mah,
        health_percent,
        time_remaining_minutes: unsigned(values, "TimeRemaining", 7 * 24 * 60)
            .or_else(|| unsigned(values, "AvgTimeToEmpty", 7 * 24 * 60)),
        adapter_watts: adapter.and_then(|details| unsigned(details, "Watts", 1_000)),
        adapter_name: adapter
            .and_then(|details| details.get("Name"))
            .and_then(plist::Value::as_string)
            .and_then(normalized_diagnostic_label),
    }
}

fn normalized_diagnostic_label(value: &str) -> Option<String> {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    (!value.is_empty()
        && value.chars().count() <= 64
        && value
            .chars()
            .all(|character| !character.is_control() && !matches!(character, '/' | '\\')))
    .then_some(value)
}

fn format_media_start_error(
    stream: &str,
    error: IdeviceError,
    identity: Option<&DeviceDetails>,
) -> String {
    let is_ios_27_gate = matches!(
        &error,
        IdeviceError::CoreDevice(CoreDeviceError::DeviceError(details))
            if details.contains("Integer(9021)")
                || details.contains("Remote control requires iOS 27.0 or later")
    );
    if !is_ios_27_gate {
        return format!("{stream} startMediaStream failed: {error:?}");
    }

    tracing::debug!(stream, error = ?error, "CoreDevice rejected remote-control capability");
    let detected = identity.map_or_else(
        || "this device".to_string(),
        |identity| {
            format!(
                "{} running iOS {}",
                identity.product_type, identity.product_version
            )
        },
    );
    format!(
        "Remote control is unavailable on {detected} (CoreDevice 9021). Apple requires iOS \
         27.0 or later for this device; update iOS or use a supported newer device. Switching \
         between USB and Wi-Fi cannot bypass this device-side capability check."
    )
}

/// Connect the displayservice and start the audio+video screen-sharing session.
/// Audio is started first to establish the session, then video on the same
/// `clientSessionID`.
async fn start_screen_media_stream(
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
    our_ssrc: u32,
    identity: Option<&DeviceDetails>,
    connection: ConnKind,
) -> Result<ScreenMediaStream, String> {
    let mut client = match DisplayServiceClient::connect_rsd(adapter, handshake).await {
        Ok(client) => client,
        Err(IdeviceError::ServiceNotFound) => {
            let mut related_services = handshake
                .services
                .keys()
                .filter(|name| {
                    let name = name.to_ascii_lowercase();
                    ["display", "screen", "media", "capture"]
                        .iter()
                        .any(|needle| name.contains(needle))
                })
                .cloned()
                .collect::<Vec<_>>();
            related_services.sort();
            tracing::warn!(
                connection = connection.label(),
                service_count = handshake.services.len(),
                ?related_services,
                "RSD did not advertise com.apple.coredevice.displayservice"
            );
            tracing::debug!(services = ?handshake.services.keys().collect::<Vec<_>>(), "RSD services");

            let hint = if cfg!(windows) {
                " USB supports displayservice, but this device has not published the Device Hub service set. Keep it connected and unlocked, then run `.\\scripts\\prepare-windows-device.ps1` to verify Developer Mode and mount the Personalized Developer Disk Image."
            } else {
                " The device has not published the Device Hub service set. Verify Developer Mode, the Personalized Developer Disk Image, and Device Hub pairing."
            };
            return Err(format!(
                "display service is unavailable on {} (RSD advertised {} services).{hint}",
                connection.label(),
                handshake.services.len()
            ));
        }
        Err(error) => return Err(format!("no display service: {error:?}")),
    };

    let audio_udp = adapter
        .bind_udp(0)
        .await
        .map_err(|e| format!("bind_udp(audio) failed: {e:?}"))?;
    let video_udp = adapter
        .bind_udp(0)
        .await
        .map_err(|e| format!("bind_udp(video) failed: {e:?}"))?;
    let receiver_ip = adapter.host_ip().to_string();
    let audio_receiver_port = audio_udp.local_port();
    let receiver_port = video_udp.local_port();
    let sender_ip = adapter.peer_ip().to_string();

    // Video RTCP socket at receiver_port + 1 (RFC 3550); falls back to mux-only if
    // unavailable. The send loop auto-detects where the device's RTCP actually is.
    let rtcp_udp = adapter.bind_udp(receiver_port + 1).await.ok();
    if rtcp_udp.is_none() {
        tracing::info!(
            "RTCP port {} unavailable; relying on rtcp-mux",
            receiver_port + 1
        );
    }

    let call_info = call_info();
    let session_id = uuid::Uuid::new_v4();

    // Audio stream first (establishes the screen-sharing session).
    let audio_call_id = uuid::Uuid::new_v4().to_string().to_uppercase();
    let audio_offer = build_screen_audio_offer(&audio_call_id, &call_info)
        .map_err(|e| format!("audio offer build failed: {e:?}"))?;
    let audio_params = build_start_audio_parameters(
        &receiver_ip,
        audio_receiver_port,
        &sender_ip,
        50000,
        audio_offer,
        CLIENT_SUPPORTED_FEATURES,
        session_id,
    );
    let audio_response = client
        .start_media_stream(audio_params)
        .await
        .map_err(|error| format_media_start_error("audio", error, identity))?;
    log_audio_negotiation(&audio_response);

    // Video stream on the same session.
    start_video(
        &mut client,
        &receiver_ip,
        receiver_port,
        &sender_ip,
        session_id,
        our_ssrc,
        identity,
    )
    .await?;
    match client.get_media_stream_server_status().await {
        Ok(status) => log_media_server_status(&status),
        Err(error) => tracing::warn!(?error, "unable to query negotiated media stream status"),
    }

    Ok(ScreenMediaStream {
        client,
        audio_udp,
        video_udp,
        rtcp_udp,
    })
}

fn log_audio_negotiation(response: &plist::Value) {
    let response_fields = response
        .as_dictionary()
        .map(|dictionary| dictionary.keys().cloned().collect::<Vec<_>>());
    let Some(answer) = find_negotiator_answer(response) else {
        tracing::warn!(
            ?response_fields,
            "audio negotiation response did not contain an answer"
        );
        tracing::debug!(response = ?response, "unparsed audio negotiation response");
        return;
    };
    let Ok(negotiation) = parse_answer_media_blob(answer) else {
        tracing::warn!(
            ?response_fields,
            answer_bytes = answer.len(),
            "unable to parse audio negotiation answer"
        );
        return;
    };
    tracing::info!(
        audio_features = negotiation
            .codec_features
            .as_ref()
            .map(|features| features.audio_features),
        stream_groups = negotiation.stream_groups.len(),
        "audio media negotiation accepted"
    );
    for (group_index, group) in negotiation.stream_groups.iter().enumerate() {
        for payload in &group.payloads {
            tracing::info!(
                group_index,
                stream_group = group.stream_group,
                codec_type = payload.codec_type,
                rtp_payload_type = payload.rtp_payload,
                packet_time = payload.p_time,
                rtcp_flags = payload.rtcp_flags,
                media_flags = payload.media_flags,
                profile_level_id = payload.profile_level_id,
                rtp_sample_rate = payload.rtp_sample_rate,
                cipher_suite = payload.cipher_suite,
                packed_payload_bytes = payload.packed_payload.len(),
                encoder_usage = payload.encoder_usage,
                "negotiated audio payload"
            );
        }
        for stream in &group.streams {
            tracing::info!(
                group_index,
                stream_group = group.stream_group,
                rtp_ssrc = format_args!("{:#x}", stream.rtp_ssrc),
                stream_id = stream.stream_id,
                audio_channels = stream.audio_channel_count,
                stream_index = stream.stream_index,
                required_payload_bytes = stream.required_packed_payload.len(),
                optional_payload_bytes = stream.optional_packed_payload.len(),
                "negotiated audio stream"
            );
        }
    }
}

fn log_media_server_status(status: &plist::Value) {
    let mut fields = Vec::new();
    collect_plist_fields("media_status", status, &mut fields, 0);
    tracing::info!(
        fields = fields.len(),
        "captured negotiated media stream status"
    );
    for (path, value) in fields.into_iter().take(256) {
        tracing::debug!(target: "devicehub_mask::audio", %path, %value, "media stream status field");
    }
}

fn collect_plist_fields(
    path: &str,
    value: &plist::Value,
    fields: &mut Vec<(String, String)>,
    depth: usize,
) {
    if depth > 10 || fields.len() >= 256 {
        return;
    }
    match value {
        plist::Value::Dictionary(dictionary) => {
            for (key, value) in dictionary {
                collect_plist_fields(&format!("{path}.{key}"), value, fields, depth + 1);
            }
        }
        plist::Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                collect_plist_fields(&format!("{path}[{index}]"), value, fields, depth + 1);
            }
        }
        plist::Value::Data(data) => {
            fields.push((path.to_string(), format!("data[{}]", data.len())));
            if let Ok(nested) = plist::from_bytes::<plist::Value>(data) {
                collect_plist_fields(&format!("{path}.plist"), &nested, fields, depth + 1);
            }
        }
        plist::Value::String(value) => {
            let normalized_path = path.to_ascii_lowercase();
            let sensitive = ["address", "ip", "uuid", "sessionid", "deviceid"]
                .iter()
                .any(|key| normalized_path.contains(key));
            let value = if sensitive {
                "<redacted>".to_string()
            } else {
                value.chars().take(160).collect()
            };
            fields.push((path.to_string(), value));
        }
        plist::Value::Boolean(value) => fields.push((path.to_string(), value.to_string())),
        plist::Value::Real(value) => fields.push((path.to_string(), value.to_string())),
        plist::Value::Integer(value) => fields.push((path.to_string(), format!("{value:?}"))),
        plist::Value::Date(_) => fields.push((path.to_string(), "<date>".into())),
        plist::Value::Uid(value) => fields.push((path.to_string(), format!("{value:?}"))),
        _ => fields.push((path.to_string(), format!("{value:?}"))),
    }
}

fn find_negotiator_answer(value: &plist::Value) -> Option<&[u8]> {
    match value {
        plist::Value::Dictionary(dictionary) => dictionary.iter().find_map(|(key, value)| {
            if key.to_ascii_lowercase().contains("negotiatoranswer") {
                value.as_data()
            } else {
                find_negotiator_answer(value)
            }
        }),
        plist::Value::Array(values) => values.iter().find_map(find_negotiator_answer),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AacAuHeader {
    header_bits: u16,
    access_units: u16,
    first_access_unit_bytes: u16,
}

fn parse_aac_au_header(payload: &[u8]) -> Option<AacAuHeader> {
    let header_bits = u16::from_be_bytes([*payload.first()?, *payload.get(1)?]);
    if header_bits == 0 || header_bits % 16 != 0 {
        return None;
    }
    let header_bytes = usize::from(header_bits).div_ceil(8);
    if payload.len() < 2 + header_bytes || header_bytes < 2 {
        return None;
    }
    let first = u16::from_be_bytes([payload[2], payload[3]]);
    let first_access_unit_bytes = first >> 3;
    let encoded_bytes = payload.len() - 2 - header_bytes;
    if usize::from(first_access_unit_bytes) > encoded_bytes {
        return None;
    }
    Some(AacAuHeader {
        header_bits,
        access_units: header_bits / 16,
        first_access_unit_bytes,
    })
}

async fn audio_task(udp: UdpSocketHandle, output: AudioOutput, enabled: bool) {
    if !enabled {
        tracing::info!("device audio playback disabled; draining negotiated audio stream");
        audio_receive_loop(&udp, None).await;
        return;
    }

    let mut restart_attempt = 0_u32;
    loop {
        let (mut child, stdout, stderr, rtp_address) = match decode::spawn_audio_ffmpeg().await {
            Ok(process) => process,
            Err(error) => {
                tracing::warn!(%error, "cannot start device audio decoder; draining audio stream");
                audio_receive_loop(&udp, None).await;
                return;
            }
        };
        let sender = match tokio::net::UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await {
            Ok(sender) => sender,
            Err(error) => {
                tracing::warn!(%error, "cannot bind audio RTP forwarding socket");
                audio_receive_loop(&udp, None).await;
                return;
            }
        };
        let decoder_started = Instant::now();
        let decoded_output = decode::read_audio_chunks(stdout, output.clone());
        let errors = watch_audio_errors(stderr);
        let receive = audio_receive_loop(&udp, Some((&sender, rtp_address)));
        tokio::pin!(decoded_output, errors, receive);
        let exit_reason = tokio::select! {
            _ = &mut decoded_output => "output-ended",
            _ = &mut errors => "stderr-ended",
            _ = &mut receive => {
                tracing::warn!("device audio RTP input ended");
                return;
            }
            status = child.wait() => {
                tracing::warn!(?status, "device audio decoder stopped");
                "process-ended"
            },
        };
        let elapsed = decoder_started.elapsed();
        restart_attempt = if elapsed >= AUDIO_DECODER_STABLE_RUNTIME {
            1
        } else {
            restart_attempt.saturating_add(1)
        };
        let retry_delay = audio_decoder_restart_backoff(restart_attempt - 1);
        tracing::warn!(
            exit_reason,
            elapsed_ms = elapsed.as_millis() as u64,
            restart_attempt,
            retry_ms = retry_delay.as_millis() as u64,
            "device audio decoder ended; restarting"
        );
        drop(child);
        if !drain_audio_until_retry(&udp, retry_delay).await {
            return;
        }
    }
}

fn audio_decoder_restart_backoff(attempt: u32) -> Duration {
    Duration::from_millis(250_u64.saturating_mul(1_u64 << attempt.min(4)))
}

async fn drain_audio_until_retry(udp: &UdpSocketHandle, delay: Duration) -> bool {
    let retry = tokio::time::sleep(delay);
    tokio::pin!(retry);
    loop {
        tokio::select! {
            _ = &mut retry => return true,
            packet = udp.recv() => {
                if let Err(error) = packet {
                    tracing::warn!(?error, "audio UDP receive failed while restarting decoder");
                    return false;
                }
            }
        }
    }
}

async fn watch_audio_errors(stderr: ChildStderr) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::warn!(target: "devicehub_mask::audio", message = %line, "ffmpeg audio decode error");
    }
}

async fn audio_receive_loop(
    udp: &UdpSocketHandle,
    forwarding: Option<(&tokio::net::UdpSocket, std::net::SocketAddr)>,
) {
    let mut stream: Option<(u8, u32)> = None;
    let mut last_sequence = None;
    let mut last_timestamp = None;
    let mut timestamp_deltas = RunningStats::default();
    let mut payload_sizes = RunningStats::default();
    let mut packets = 0_u64;
    let mut bytes = 0_u64;
    let mut lost_packets = 0_u64;
    let mut marker_packets = 0_u64;
    let mut rtcp_packets = 0_u64;
    let mut started = Instant::now();
    loop {
        let datagram = match udp.recv().await {
            Ok(datagram) => datagram,
            Err(error) => {
                tracing::warn!(?error, "audio UDP receive failed");
                return;
            }
        };
        if is_rtcp(&datagram.data) {
            rtcp_packets += 1;
            continue;
        }
        let Some(packet) = RtpPacket::parse(&datagram.data) else {
            continue;
        };
        if let Some((sender, target)) = forwarding {
            match add_rfc3640_au_header(&datagram.data) {
                Ok(packet) => {
                    if let Err(error) = sender.send_to(&packet, target).await {
                        tracing::warn!(%error, "failed to forward audio RTP packet to ffmpeg");
                        return;
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        error,
                        packet_bytes = datagram.data.len(),
                        "dropping invalid audio RTP packet"
                    );
                    continue;
                }
            }
        }
        if stream != Some((packet.payload_type, packet.ssrc)) {
            stream = Some((packet.payload_type, packet.ssrc));
            last_sequence = None;
            last_timestamp = None;
            tracing::info!(
                rtp_payload_type = packet.payload_type,
                rtp_ssrc = format_args!("{:#x}", packet.ssrc),
                source_port = datagram.source_port,
                extension = packet.extension,
                extension_profile = format_args!("{:#x}", packet.ext_profile),
                extension_bytes = packet.ext_data.len(),
                payload_bytes = packet.payload.len(),
                aac_au_header = ?parse_aac_au_header(packet.payload),
                "audio RTP stream detected"
            );
        }
        if let Some(previous) = last_sequence {
            let distance = packet.sequence_number.wrapping_sub(previous);
            if distance > 1 && distance < 0x8000 {
                lost_packets += u64::from(distance - 1);
            }
        }
        if let Some(previous) = last_timestamp {
            let delta = packet.timestamp.wrapping_sub(previous);
            if delta > 0 && delta < 1_000_000 {
                timestamp_deltas.push(delta as f64);
            }
        }
        last_sequence = Some(packet.sequence_number);
        last_timestamp = Some(packet.timestamp);
        packets += 1;
        bytes += datagram.data.len() as u64;
        marker_packets += u64::from(packet.marker);
        payload_sizes.push(packet.payload.len() as f64);

        if started.elapsed() >= Duration::from_secs(5) {
            tracing::debug!(
                target: "devicehub_mask::audio",
                elapsed_ms = started.elapsed().as_millis() as u64,
                packets,
                bytes,
                lost_packets,
                marker_packets,
                rtcp_packets,
                payload_bytes_mean = ?payload_sizes.mean(),
                payload_bytes_min = ?payload_sizes.min(),
                payload_bytes_max = ?payload_sizes.max(),
                timestamp_delta_ticks = ?timestamp_deltas.mean(),
                timestamp_delta_min_ticks = ?timestamp_deltas.min(),
                timestamp_delta_max_ticks = ?timestamp_deltas.max(),
                "audio RTP diagnostics"
            );
            packets = 0;
            bytes = 0;
            lost_packets = 0;
            marker_packets = 0;
            rtcp_packets = 0;
            payload_sizes = RunningStats::default();
            timestamp_deltas = RunningStats::default();
            started = Instant::now();
        }
    }
}

fn add_rfc3640_au_header(packet: &[u8]) -> Result<Vec<u8>, &'static str> {
    if packet.len() < 12 || packet[0] >> 6 != 2 {
        return Err("invalid RTP header");
    }
    let csrc_bytes = usize::from(packet[0] & 0x0f)
        .checked_mul(4)
        .ok_or("RTP header overflow")?;
    let mut payload_offset = 12_usize
        .checked_add(csrc_bytes)
        .ok_or("RTP header overflow")?;
    if packet.len() < payload_offset {
        return Err("truncated RTP CSRC list");
    }
    if packet[0] & 0x10 != 0 {
        if packet.len() < payload_offset + 4 {
            return Err("truncated RTP extension header");
        }
        let extension_words =
            u16::from_be_bytes([packet[payload_offset + 2], packet[payload_offset + 3]]);
        payload_offset = payload_offset
            .checked_add(4 + usize::from(extension_words) * 4)
            .ok_or("RTP extension overflow")?;
        if packet.len() < payload_offset {
            return Err("truncated RTP extension");
        }
    }
    let mut payload_end = packet.len();
    if packet[0] & 0x20 != 0 {
        let padding = usize::from(*packet.last().ok_or("missing RTP padding")?);
        if padding == 0 || padding > payload_end.saturating_sub(payload_offset) {
            return Err("invalid RTP padding");
        }
        payload_end -= padding;
    }
    let payload_len = payload_end.saturating_sub(payload_offset);
    if payload_len == 0 || payload_len > 0x1fff {
        return Err("AAC access unit length is outside the 13-bit RFC 3640 range");
    }
    let mut adapted = Vec::with_capacity(payload_offset + 4 + payload_len);
    adapted.extend_from_slice(&packet[..payload_offset]);
    adapted[0] &= !0x20; // output omits the source packet's RTP padding
    adapted.extend_from_slice(&[0, 16]);
    adapted.extend_from_slice(&((payload_len as u16) << 3).to_be_bytes());
    adapted.extend_from_slice(&packet[payload_offset..payload_end]);
    Ok(adapted)
}

/// The `VCCallInfoBlob` describing this (host) endpoint. The string values mirror
/// a captured Device Hub offer the device accepted.
fn call_info() -> CallInfoBlob {
    CallInfoBlob {
        call_id: 0,
        client_version: 1,
        device_type: "Mac17,7".into(),
        framework_version: "2205.3.1".into(),
        os_version: "25F71".into(),
        device_name: None,
        audio_device_uid: None,
    }
}

/// Issue the video `startmediastream` on an existing (audio-established) session.
async fn start_video(
    client: &mut DisplayServiceClient<Box<dyn ReadWrite>>,
    receiver_ip: &str,
    receiver_port: u16,
    sender_ip: &str,
    session_id: uuid::Uuid,
    our_ssrc: u32,
    identity: Option<&DeviceDetails>,
) -> Result<(), String> {
    let call_id = uuid::Uuid::new_v4().to_string().to_uppercase();
    let offer = build_screen_video_offer(&call_id, &call_info(), our_ssrc)
        .map_err(|e| format!("video offer build failed: {e:?}"))?;
    let params = build_start_video_parameters(
        receiver_ip,
        receiver_port,
        sender_ip,
        50001,
        offer,
        CLIENT_SUPPORTED_FEATURES,
        1,
        session_id,
    );
    client
        .start_media_stream(params)
        .await
        .map_err(|error| format_media_start_error("video", error, identity))?;
    Ok(())
}

/// Watch ffmpeg's stderr for HEVC decode errors; each pulses `corruption` to ask
/// [`rtcp_send_task`] for a fresh IDR. The encoder sends only one IDR, so a dropped
/// packet floods these errors and they never stop on their own.
async fn watch_decode_errors(stderr: ChildStderr, corruption: Arc<Notify>) {
    let mut reader = BufReader::new(stderr);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => break, // ffmpeg exited
            Ok(_) => {
                if line.contains("Could not find ref")
                    || line.contains("Error constructing")
                    || line.contains("error while decoding")
                {
                    corruption.notify_one();
                }
            }
            Err(_) => break,
        }
    }
}

/// Route a silently stalled stream into keyframe recovery: a fully silent stream
/// (no RTP - the App-Nap case) yields no frames and no decode errors, so nothing
/// else trips recovery. If no frame arrives within [`STALL_TIMEOUT`], pulse
/// `corruption`.
async fn stall_watchdog(frame_beat: Arc<Notify>, corruption: &Notify) {
    loop {
        if tokio::time::timeout(STALL_TIMEOUT, frame_beat.notified())
            .await
            .is_err()
        {
            tracing::debug!("no video frames for {STALL_TIMEOUT:?}; requesting keyframe");
            corruption.notify_one();
        }
    }
}

/// Build the provider chosen by the picker without silently switching transport.
async fn connect_provider(
    endpoint: SessionEndpoint,
) -> Result<(Arc<dyn IdeviceProvider>, ConnKind), String> {
    let udid = endpoint.udid().to_owned();
    let connection = endpoint.connection();
    let provider: Arc<dyn IdeviceProvider> = match endpoint {
        SessionEndpoint::Usbmuxd(endpoint) => Arc::new(
            endpoint
                .device
                .to_provider(endpoint.address, "devicehub_rs"),
        ),
        SessionEndpoint::Wifi(endpoint) => Arc::new(wifi_provider(&endpoint)),
    };
    tracing::info!(
        device_id = %crate::diagnostics::device_id_fingerprint(&udid),
        connection = connection.label(),
        "selected CoreDevice transport"
    );
    Ok((provider, connection))
}

fn connection_priority(connection: &Connection) -> u8 {
    match connection {
        Connection::Usb => 0,
        Connection::Network(_) => 1,
        Connection::Unknown(_) => 2,
    }
}

fn uses_usbmuxd_core_proxy(connection: &Connection) -> bool {
    matches!(connection, Connection::Usb)
}

fn connection_kind(connection: &Connection) -> ConnKind {
    match connection {
        Connection::Network(_) => ConnKind::Network,
        Connection::Usb => ConnKind::Usb,
        Connection::Unknown(_) => ConnKind::Other,
    }
}

fn connection_kind_priority(connection: ConnKind) -> u8 {
    match connection {
        ConnKind::Usb => 0,
        ConnKind::Network => 1,
        ConnKind::Other => 2,
    }
}

fn resolve_device_selection(requested: &str, devices: &[DeviceInfo]) -> Option<String> {
    devices
        .iter()
        .find(|device| device.id == requested)
        .or_else(|| {
            devices
                .iter()
                .filter(|device| device.udid == requested)
                .min_by_key(|device| connection_kind_priority(device.connection))
        })
        .map(|device| device.id.clone())
}

#[cfg(test)]
fn select_preferred_usbmuxd_device(
    devices: Vec<UsbmuxdDevice>,
    udid: Option<&str>,
) -> Option<UsbmuxdDevice> {
    devices
        .into_iter()
        .filter(|device| udid.is_none_or(|wanted| device.udid == wanted))
        .min_by_key(|device| {
            (
                connection_priority(&device.connection_type),
                device.device_id,
            )
        })
}

fn wifi_provider(endpoint: &WifiEndpoint) -> TcpProvider {
    TcpProvider {
        addr: endpoint.address,
        scope_id: endpoint.scope_id,
        pairing_file: endpoint.pairing_file.clone(),
        label: "devicehub_rs_wifi".into(),
    }
}

/// Map an ASCII character to its HID Keyboard/Keypad usage and whether Shift is
/// required (US layout). Ported from idevice-tools' `hid` command.
fn ascii_to_usage(c: char) -> Option<(u64, bool)> {
    Some(match c {
        'a'..='z' => (0x04 + (c as u64 - 'a' as u64), false),
        'A'..='Z' => (0x04 + (c as u64 - 'A' as u64), true),
        '1'..='9' => (0x1E + (c as u64 - '1' as u64), false),
        '0' => (0x27, false),
        '\n' => (0x28, false), // Return
        '\t' => (0x2B, false), // Tab
        ' ' => (0x2C, false),  // Space
        '!' => (0x1E, true),
        '@' => (0x1F, true),
        '#' => (0x20, true),
        '$' => (0x21, true),
        '%' => (0x22, true),
        '^' => (0x23, true),
        '&' => (0x24, true),
        '*' => (0x25, true),
        '(' => (0x26, true),
        ')' => (0x27, true),
        '-' => (0x2D, false),
        '_' => (0x2D, true),
        '=' => (0x2E, false),
        '+' => (0x2E, true),
        '[' => (0x2F, false),
        '{' => (0x2F, true),
        ']' => (0x30, false),
        '}' => (0x30, true),
        '\\' => (0x31, false),
        '|' => (0x31, true),
        ';' => (0x33, false),
        ':' => (0x33, true),
        '\'' => (0x34, false),
        '"' => (0x34, true),
        '`' => (0x35, false),
        '~' => (0x35, true),
        ',' => (0x36, false),
        '<' => (0x36, true),
        '.' => (0x37, false),
        '>' => (0x37, true),
        '/' => (0x38, false),
        '?' => (0x38, true),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use idevice::usbmuxd::UsbmuxdConnection;

    #[tokio::test]
    #[ignore = "requires a connected physical device"]
    async fn reads_developer_mode_status_from_hardware() {
        let mut usbmuxd = UsbmuxdConnection::default().await.expect("connect usbmuxd");
        let devices = usbmuxd.get_devices().await.expect("list devices");
        let endpoint = SessionEndpoint::Usbmuxd(Box::new(UsbmuxdEndpoint {
            device: select_preferred_usbmuxd_device(devices, None).expect("connected device"),
            address: UsbmuxdAddr::default(),
        }));
        let (provider, _) = connect_provider(endpoint)
            .await
            .expect("connect device provider");
        let enabled = read_developer_mode_status(provider.as_ref())
            .await
            .expect("query developer mode");
        eprintln!("developer mode enabled: {enabled}");
    }

    #[test]
    fn device_power_slot_rejects_concurrent_commands_and_releases_on_drop() {
        let slot = DevicePowerSlot::default();
        let lease = slot.try_start().unwrap();
        assert!(slot.try_start().is_err());
        drop(lease);
        assert!(slot.try_start().is_ok());
    }

    fn access_unit(size: usize, is_irap: bool) -> HevcAccessUnit {
        HevcAccessUnit {
            bytes: vec![0x5a; size],
            is_irap,
            rtp_timestamp: 0,
        }
    }

    #[test]
    fn running_stats_reports_mean_range_and_jitter() {
        let mut stats = RunningStats::default();
        stats.push(10.0);
        stats.push(20.0);
        stats.push(30.0);

        assert_eq!(stats.mean(), Some(20.0));
        assert_eq!(stats.min(), Some(10.0));
        assert_eq!(stats.max(), Some(30.0));
        assert!((stats.standard_deviation().unwrap() - 8.164_965_809).abs() < 1e-6);
    }

    #[test]
    fn assembles_access_units_across_split_aud_boundaries() {
        let first = [0, 0, 0, 1, 0x02, 0x01, 0xaa];
        let second = [0, 0, 0, 1, 0x26, 0x01, 0xbb];
        let mut assembler = AccessUnitAssembler::default();

        let mut first_chunk = first.to_vec();
        first_chunk.extend_from_slice(&HEVC_AUD[..3]);
        assert!(assembler.push(&first_chunk, 90_000).is_empty());

        let mut second_chunk = HEVC_AUD[3..].to_vec();
        second_chunk.extend_from_slice(&second);
        let completed = assembler.push(&second_chunk, 91_500);
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].bytes, first);
        assert!(!completed[0].is_irap);
        assert_eq!(completed[0].rtp_timestamp, 90_000);

        let completed = assembler.push(HEVC_AUD, 93_000);
        assert_eq!(completed.len(), 1);
        assert!(completed[0].bytes.starts_with(HEVC_AUD));
        assert!(completed[0].is_irap);
        assert_eq!(completed[0].rtp_timestamp, 91_500);
    }

    #[test]
    fn finishes_access_unit_at_complete_rtp_marker() {
        let irap = [0, 0, 0, 1, 0x26, 0x01, 0xbb];
        let mut assembler = AccessUnitAssembler::default();

        assert!(assembler.push(&irap, 123_456).is_empty());
        let completed = assembler.finish().unwrap();
        assert_eq!(completed.bytes, irap);
        assert!(completed.is_irap);
        assert_eq!(completed.rtp_timestamp, 123_456);
        assert!(assembler.finish().is_none());
    }

    #[test]
    fn browser_video_clock_preserves_source_cadence_and_wraps() {
        let mut clock = RtpVideoClock::default();
        assert_eq!(clock.timestamp_us(u32::MAX - 749), 0);
        assert_eq!(clock.timestamp_us(u32::MAX), 8_322);
        assert_eq!(clock.timestamp_us(749), 16_655);
        assert_eq!(clock.timestamp_us(1_499), 24_988);
    }

    #[test]
    fn recognizes_rfc3640_aac_access_unit_headers_without_reading_audio_data() {
        // 16 header bits, one 13-bit AU size (4 bytes) plus a 3-bit index.
        let payload = [0x00, 0x10, 0x00, 0x20, 1, 2, 3, 4];
        assert_eq!(
            parse_aac_au_header(&payload),
            Some(AacAuHeader {
                header_bits: 16,
                access_units: 1,
                first_access_unit_bytes: 4,
            })
        );
        assert_eq!(parse_aac_au_header(&[0x00, 0x10, 0x01, 0x00, 1]), None);
        assert_eq!(parse_aac_au_header(&[0x00, 0x07, 0, 0]), None);
    }

    #[test]
    fn audio_decoder_restart_backoff_is_bounded() {
        assert_eq!(audio_decoder_restart_backoff(0), Duration::from_millis(250));
        assert_eq!(audio_decoder_restart_backoff(1), Duration::from_millis(500));
        assert_eq!(audio_decoder_restart_backoff(4), Duration::from_secs(4));
        assert_eq!(audio_decoder_restart_backoff(20), Duration::from_secs(4));
    }

    #[test]
    fn adds_rfc3640_header_to_raw_aac_rtp() {
        let mut packet = vec![0x80, 101, 0, 1, 0, 0, 1, 224, 1, 2, 3, 4];
        packet.extend_from_slice(&[0xaa, 0xbb, 0xcc]);
        let adapted = add_rfc3640_au_header(&packet).unwrap();
        assert_eq!(&adapted[..12], &packet[..12]);
        assert_eq!(&adapted[12..16], &[0, 16, 0, 24]);
        assert_eq!(&adapted[16..], &[0xaa, 0xbb, 0xcc]);
    }

    #[test]
    fn preserves_rtp_extensions_and_removes_padding() {
        let mut packet = vec![0xb1, 101, 0, 1, 0, 0, 1, 224, 1, 2, 3, 4];
        packet.extend_from_slice(&[9, 8, 7, 6]); // one CSRC
        packet.extend_from_slice(&[0xbe, 0xde, 0, 1, 1, 2, 3, 4]);
        packet.extend_from_slice(&[0xaa, 0xbb, 0, 0, 3]);
        let adapted = add_rfc3640_au_header(&packet).unwrap();
        assert_eq!(adapted[0], 0x91);
        assert_eq!(
            &adapted[..24],
            &[
                0x91, 101, 0, 1, 0, 0, 1, 224, 1, 2, 3, 4, 9, 8, 7, 6, 0xbe, 0xde, 0, 1, 1, 2, 3, 4
            ]
        );
        assert_eq!(&adapted[24..], &[0, 16, 0, 16, 0xaa, 0xbb]);
    }

    #[test]
    fn rejects_oversized_or_truncated_audio_rtp() {
        let mut oversized = vec![0x80, 101, 0, 1, 0, 0, 1, 224, 1, 2, 3, 4];
        oversized.resize(12 + 0x2000, 0);
        assert!(add_rfc3640_au_header(&oversized).is_err());
        assert!(add_rfc3640_au_header(&[0x90, 101, 0, 1, 0, 0, 1, 224, 1, 2, 3, 4]).is_err());
    }

    #[tokio::test]
    async fn bounded_hevc_queue_recovers_only_at_irap() {
        let queue = HevcQueue::new(10);
        assert!(matches!(
            queue.push(access_unit(6, false)),
            HevcQueuePush::Enqueued
        ));
        assert!(matches!(
            queue.push(access_unit(6, false)),
            HevcQueuePush::NeedsKeyframe {
                queued_bytes: 6,
                incoming_bytes: 6,
            }
        ));
        assert!(matches!(
            queue.push(access_unit(2, false)),
            HevcQueuePush::Dropped
        ));
        assert!(matches!(
            queue.push(access_unit(4, true)),
            HevcQueuePush::Recovered {
                dropped_access_units: 3,
                dropped_bytes: 14,
            }
        ));

        let recovered = queue.pop().await.unwrap();
        assert!(recovered.is_irap);
        assert_eq!(recovered.bytes.len(), 4);
        queue.close();
        assert!(queue.pop().await.is_none());
    }

    #[test]
    fn maps_springboard_interface_orientations() {
        assert_eq!(
            orientation_from_interface(InterfaceOrientation::Portrait),
            Some(Orientation::Portrait)
        );
        assert_eq!(
            orientation_from_interface(InterfaceOrientation::PortraitUpsideDown),
            Some(Orientation::PortraitUpsideDown)
        );
        assert_eq!(
            orientation_from_interface(InterfaceOrientation::LandscapeLeft),
            Some(Orientation::LandscapeRight)
        );
        assert_eq!(
            orientation_from_interface(InterfaceOrientation::LandscapeRight),
            Some(Orientation::LandscapeLeft)
        );
        assert_eq!(
            orientation_from_interface(InterfaceOrientation::Unknown),
            None
        );
    }

    #[test]
    fn transport_selection_is_explicit_and_legacy_udids_prefer_usb() {
        let devices = vec![
            DeviceInfo {
                id: device_selector("phone", ConnKind::Usb),
                udid: "phone".into(),
                name: "iPhone".into(),
                connection: ConnKind::Usb,
                pairing: DevicePairingState::Paired,
            },
            DeviceInfo {
                id: device_selector("phone", ConnKind::Network),
                udid: "phone".into(),
                name: "iPhone".into(),
                connection: ConnKind::Network,
                pairing: DevicePairingState::NotApplicable,
            },
        ];

        assert_eq!(
            resolve_device_selection("phone", &devices).as_deref(),
            Some("phone::usb")
        );
        assert_eq!(
            resolve_device_selection("phone::wifi", &devices).as_deref(),
            Some("phone::wifi")
        );
    }

    #[test]
    fn pairing_errors_are_normalized_for_the_frontend() {
        assert_eq!(
            pairing_failure(IdeviceError::UserDeniedPairing).outcome,
            PairDeviceOutcome::Denied
        );
        assert_eq!(
            pairing_failure(IdeviceError::PasswordProtected).outcome,
            PairDeviceOutcome::Locked
        );
        assert_eq!(
            pairing_failure(IdeviceError::DeviceLocked).outcome,
            PairDeviceOutcome::Locked
        );
        assert_eq!(
            pairing_failure(IdeviceError::DeviceNotFound).outcome,
            PairDeviceOutcome::Failed
        );
    }

    #[test]
    fn trust_removal_preserves_partial_success() {
        assert_eq!(
            forget_device_result(None, None).outcome,
            ForgetDeviceOutcome::Forgotten
        );
        assert_eq!(
            forget_device_result(Some("device unavailable".into()), None).outcome,
            ForgetDeviceOutcome::HostRecordRemoved
        );
        assert_eq!(
            forget_device_result(None, Some("host cleanup failed".into())).outcome,
            ForgetDeviceOutcome::DeviceForgottenHostCleanupFailed
        );
        let failed = forget_device_result(
            Some("device unavailable".into()),
            Some("host cleanup failed".into()),
        );
        assert_eq!(failed.outcome, ForgetDeviceOutcome::Failed);
        assert!(failed.error.unwrap().contains("host record cleanup failed"));
    }

    #[test]
    fn network_usbmuxd_devices_never_use_the_usb_coredevice_proxy() {
        assert!(uses_usbmuxd_core_proxy(&Connection::Usb));
        assert!(!uses_usbmuxd_core_proxy(&Connection::Network(
            [192, 0, 2, 1].into()
        )));
        assert!(!uses_usbmuxd_core_proxy(&Connection::Unknown(
            "Network".into()
        )));
    }

    #[test]
    fn remote_pairing_credentials_stay_inside_application_data() {
        let pairing_dir = Path::new("app-data").join("pairings");
        assert_eq!(
            remote_pairing_path(&pairing_dir, "00008030-001905C02106402E").unwrap(),
            Path::new("app-data")
                .join("remote-pairings")
                .join("00008030-001905C02106402E.plist")
        );
        assert!(remote_pairing_path(&pairing_dir, "../outside").is_err());
        assert!(remote_pairing_path(&pairing_dir, "phone/plist").is_err());
        assert!(remote_pairing_path(&pairing_dir, "").is_err());
    }

    #[test]
    fn summarizes_coredevice_9021_without_binary_plist_dump() {
        let error = IdeviceError::CoreDevice(CoreDeviceError::DeviceError(
            r#"Dictionary({"code": Integer(9021), "NSLocalizedDescription": String("Remote control requires iOS 27.0 or later on this device.")})"#.into(),
        ));
        let identity = DeviceDetails {
            udid: "phone".into(),
            name: "Test iPhone".into(),
            product_type: "iPhone11,2".into(),
            product_version: "26.0".into(),
            build_version: None,
            device_class: None,
            cpu_architecture: None,
            model_number: None,
            hardware_model: None,
            device_color: None,
            enclosure_color: None,
            serial_number: None,
            ecid: None,
            total_disk_capacity: None,
            storage: None,
            activation_state: None,
            developer_mode_enabled: None,
            developer_image_mounted: None,
            regional_settings: None,
            battery: None,
        };

        let message = format_media_start_error("audio", error, Some(&identity));
        assert!(message.contains("CoreDevice 9021"));
        assert!(message.contains("iPhone11,2 running iOS 26.0"));
        assert!(message.contains("iOS 27.0 or later"));
        assert!(!message.contains("Dictionary"));
    }

    #[test]
    fn activation_states_are_reduced_to_a_stable_public_enum() {
        assert_eq!(
            normalize_activation_state("Activated"),
            DeviceActivationState::Activated
        );
        assert_eq!(
            normalize_activation_state(" Unactivated "),
            DeviceActivationState::Unactivated
        );
        assert_eq!(
            normalize_activation_state("FactoryActivated"),
            DeviceActivationState::FactoryActivated
        );
        assert_eq!(
            normalize_activation_state("soft_activated"),
            DeviceActivationState::SoftActivated
        );
        assert_eq!(
            normalize_activation_state("future-state\nprivate-data"),
            DeviceActivationState::Unknown
        );
    }

    #[test]
    fn normalizes_bounded_lockdown_regional_settings() {
        let values = plist::Dictionary::from_iter([
            (
                String::from("DeviceName"),
                plist::Value::String(" Boa 的 iPhone ".into()),
            ),
            (
                String::from("ProductType"),
                plist::Value::String("iPhone14,3".into()),
            ),
            (
                String::from("Language"),
                plist::Value::String(" zh-Hant ".into()),
            ),
            (String::from("Locale"), plist::Value::String("zh_TW".into())),
            (
                String::from("TimeZone"),
                plist::Value::String("Asia/Taipei".into()),
            ),
            (String::from("Uses24HourClock"), plist::Value::Boolean(true)),
        ]);
        assert_eq!(
            device_display_name(&values).as_deref(),
            Some("Boa 的 iPhone")
        );
        assert_eq!(
            device_identity_token(&values, "ProductType", 32).as_deref(),
            Some("iPhone14,3")
        );
        let regional = device_regional_settings(&values).unwrap();
        assert_eq!(regional.language.as_deref(), Some("zh-Hant"));
        assert_eq!(regional.locale.as_deref(), Some("zh_TW"));
        assert_eq!(regional.time_zone.as_deref(), Some("Asia/Taipei"));
        assert_eq!(regional.uses_24_hour_clock, Some(true));
    }

    #[test]
    fn normalizes_bounded_non_unique_device_identity() {
        let values = plist::Dictionary::from_iter([
            (
                String::from("DeviceClass"),
                plist::Value::String(" iPhone ".into()),
            ),
            (
                String::from("CPUArchitecture"),
                plist::Value::String("arm64e".into()),
            ),
            (
                String::from("ModelNumber"),
                plist::Value::String("MU663CH/A".into()),
            ),
            (
                String::from("DeviceColor"),
                plist::Value::String("#3b3b3c".into()),
            ),
            (
                String::from("EnclosureColor"),
                plist::Value::String("black-1".into()),
            ),
        ]);
        assert_eq!(
            device_identity_token(&values, "DeviceClass", 32).as_deref(),
            Some("iPhone")
        );
        assert_eq!(
            device_identity_token(&values, "CPUArchitecture", 32).as_deref(),
            Some("arm64e")
        );
        assert_eq!(
            device_identity_token(&values, "ModelNumber", 32).as_deref(),
            Some("MU663CH/A")
        );
        assert_eq!(
            device_identity_token(&values, "DeviceColor", 32).as_deref(),
            Some("#3b3b3c")
        );
        assert_eq!(
            device_identity_token(&values, "EnclosureColor", 32).as_deref(),
            Some("black-1")
        );

        let invalid = plist::Dictionary::from_iter([
            (
                String::from("DeviceName"),
                plist::Value::String("phone\nprivate".into()),
            ),
            (
                String::from("Control"),
                plist::Value::String("phone\nprivate".into()),
            ),
            (String::from("Long"), plist::Value::String("x".repeat(33))),
            (
                String::from("Unicode"),
                plist::Value::String("iPhone Pro".into()),
            ),
        ]);
        assert!(device_display_name(&invalid).is_none());
        assert!(device_identity_token(&invalid, "Control", 32).is_none());
        assert!(device_identity_token(&invalid, "Long", 32).is_none());
        assert!(device_identity_token(&invalid, "Unicode", 32).is_none());
    }

    #[test]
    fn rejects_unbounded_or_nonstandard_regional_values() {
        let values = plist::Dictionary::from_iter([
            (
                String::from("Language"),
                plist::Value::String("x".repeat(36)),
            ),
            (
                String::from("Locale"),
                plist::Value::String("en_US\nprivate".into()),
            ),
            (
                String::from("TimeZone"),
                plist::Value::String("Asia/Taipei;secret".into()),
            ),
            (
                String::from("Uses24HourClock"),
                plist::Value::String("true".into()),
            ),
        ]);
        assert!(device_regional_settings(&values).is_none());
        assert!(device_regional_settings(&plist::Dictionary::new()).is_none());
    }

    #[test]
    fn normalizes_lockdown_disk_usage_without_inventing_missing_values() {
        let values = plist::Dictionary::from_iter([
            (
                String::from("TotalDataCapacity"),
                plist::Value::Integer(120_000_000_000_u64.into()),
            ),
            (
                String::from("TotalDataAvailable"),
                plist::Value::Integer(45_000_000_000_u64.into()),
            ),
            (
                String::from("TotalSystemCapacity"),
                plist::Value::Integer(8_000_000_000_u64.into()),
            ),
        ]);

        let storage = device_storage_from_disk_usage(&values).unwrap();
        assert_eq!(storage.data_capacity_bytes, Some(120_000_000_000));
        assert_eq!(storage.data_available_bytes, Some(45_000_000_000));
        assert_eq!(storage.system_capacity_bytes, Some(8_000_000_000));
        assert_eq!(storage.system_available_bytes, None);
        assert!(device_storage_from_disk_usage(&plist::Dictionary::new()).is_none());
    }

    #[test]
    fn normalizes_battery_diagnostics_without_exposing_serials() {
        let battery_data = plist::Dictionary::from_iter([
            (
                String::from("DesignCapacity"),
                plist::Value::Integer(4325.into()),
            ),
            (
                String::from("FullChargeCapacity"),
                plist::Value::Integer(3482.into()),
            ),
        ]);
        let adapter = plist::Dictionary::from_iter([
            (
                String::from("Name"),
                plist::Value::String("20W USB-C Power Adapter".into()),
            ),
            (String::from("Watts"), plist::Value::Integer(20.into())),
            (
                String::from("SerialString"),
                plist::Value::String("must-not-leak".into()),
            ),
        ]);
        let values = plist::Dictionary::from_iter([
            (
                String::from("CurrentCapacity"),
                plist::Value::Integer(52.into()),
            ),
            (String::from("IsCharging"), plist::Value::Boolean(true)),
            (
                String::from("ExternalConnected"),
                plist::Value::Boolean(true),
            ),
            (String::from("FullyCharged"), plist::Value::Boolean(false)),
            (
                String::from("CycleCount"),
                plist::Value::Integer(1554.into()),
            ),
            (String::from("Voltage"), plist::Value::Integer(4009.into())),
            (
                String::from("Temperature"),
                plist::Value::Integer(3150.into()),
            ),
            (
                String::from("InstantAmperage"),
                plist::Value::Integer(2153.into()),
            ),
            (
                String::from("TimeRemaining"),
                plist::Value::Integer(146.into()),
            ),
            (
                String::from("BatteryData"),
                plist::Value::Dictionary(battery_data),
            ),
            (
                String::from("AdapterDetails"),
                plist::Value::Dictionary(adapter),
            ),
        ]);

        let battery = device_battery_from_ioregistry(&values);
        assert_eq!(battery.level_percent, Some(52));
        assert_eq!(battery.is_charging, Some(true));
        assert_eq!(battery.cycle_count, Some(1554));
        assert_eq!(battery.voltage_mv, Some(4009));
        assert_eq!(battery.temperature_celsius, Some(31.5));
        assert_eq!(battery.instant_amperage_ma, Some(2153));
        assert_eq!(battery.adapter_watts, Some(20));
        assert_eq!(
            battery.adapter_name.as_deref(),
            Some("20W USB-C Power Adapter")
        );
        assert!((battery.health_percent.unwrap() - 80.508_670_52).abs() < 1e-6);
        assert!(!format!("{battery:?}").contains("must-not-leak"));
    }

    #[test]
    fn bounds_untrusted_battery_diagnostics() {
        let adapter = plist::Dictionary::from_iter([
            (
                String::from("Name"),
                plist::Value::String("private/path\0adapter".into()),
            ),
            (String::from("Watts"), plist::Value::Integer(50_000.into())),
        ]);
        let values = plist::Dictionary::from_iter([
            (
                String::from("CurrentCapacity"),
                plist::Value::Integer(101.into()),
            ),
            (
                String::from("Temperature"),
                plist::Value::Integer(12_000.into()),
            ),
            (
                String::from("CycleCount"),
                plist::Value::Integer(1_000_000.into()),
            ),
            (
                String::from("Voltage"),
                plist::Value::Integer(100_000.into()),
            ),
            (
                String::from("InstantAmperage"),
                plist::Value::Integer(1_000_000.into()),
            ),
            (
                String::from("MaximumCapacityPercent"),
                plist::Value::Integer(96.into()),
            ),
            (
                String::from("AdapterDetails"),
                plist::Value::Dictionary(adapter),
            ),
        ]);

        let battery = device_battery_from_ioregistry(&values);
        assert_eq!(battery.health_percent, Some(96.0));
        assert!(battery.level_percent.is_none());
        assert!(battery.temperature_celsius.is_none());
        assert!(battery.cycle_count.is_none());
        assert!(battery.voltage_mv.is_none());
        assert!(battery.instant_amperage_ma.is_none());
        assert!(battery.adapter_watts.is_none());
        assert!(battery.adapter_name.is_none());
    }

    #[test]
    fn maps_installation_proxy_metadata_without_losing_bundle_identity() {
        let value = plist::Value::Dictionary(plist::Dictionary::from_iter([
            (
                String::from("CFBundleDisplayName"),
                plist::Value::String("Example Game".into()),
            ),
            (
                String::from("CFBundleShortVersionString"),
                plist::Value::String("2.4".into()),
            ),
            (
                String::from("CFBundleVersion"),
                plist::Value::String("42".into()),
            ),
            (String::from("IsXcodeManaged"), plist::Value::Boolean(true)),
            (
                String::from("UIFileSharingEnabled"),
                plist::Value::Boolean(true),
            ),
            (
                String::from("StaticDiskUsage"),
                plist::Value::Integer(1_500_000_u64.into()),
            ),
            (
                String::from("DynamicDiskUsage"),
                plist::Value::Integer(2_500_000_u64.into()),
            ),
        ]));

        let app = device_app_from_installation("com.example.game".into(), &value).unwrap();
        assert_eq!(app.bundle_id, "com.example.game");
        assert_eq!(app.name, "Example Game");
        assert_eq!(app.version.as_deref(), Some("2.4"));
        assert_eq!(app.bundle_version.as_deref(), Some("42"));
        assert!(app.is_developer_app);
        assert!(!app.is_app_clip);
        assert!(app.documents_available);
        assert_eq!(app.static_disk_usage_bytes, Some(1_500_000));
        assert_eq!(app.dynamic_disk_usage_bytes, Some(2_500_000));
        assert_eq!(app.total_disk_usage_bytes, Some(4_000_000));
        assert!(!app.is_removable);
        assert_eq!(app.is_running, None);
    }

    #[tokio::test]
    async fn extended_app_scopes_require_coredevice_app_service() {
        assert_eq!(
            list_device_apps(None, None, false, true, true)
                .await
                .unwrap_err(),
            "App Clip listing requires CoreDevice AppService, but it is unavailable"
        );
        assert_eq!(
            list_device_apps(None, None, true, true, true)
                .await
                .unwrap_err(),
            "system app and App Clip listing requires CoreDevice AppService, but it is unavailable"
        );
    }

    #[test]
    fn bounds_untrusted_installation_proxy_disk_usage() {
        let value = plist::Value::Dictionary(plist::Dictionary::from_iter([
            (
                String::from("StaticDiskUsage"),
                plist::Value::Integer((MAX_APP_DISK_USAGE_BYTES + 1).into()),
            ),
            (
                String::from("DynamicDiskUsage"),
                plist::Value::Integer(750_000_u64.into()),
            ),
        ]));
        assert_eq!(app_disk_usage(&value), (None, Some(750_000), Some(750_000)));
        assert_eq!(
            app_disk_usage(&plist::Value::String("invalid".into())),
            (None, None, None)
        );
    }

    #[test]
    fn normalizes_app_signing_metadata_without_exposing_signer_identity() {
        use crate::protocol::AppSigningKind;

        let metadata = |signer: &str, extra: Vec<(&str, plist::Value)>| {
            let mut fields = plist::Dictionary::new();
            fields.insert("SignerIdentity".into(), signer.into());
            fields.extend(extra.into_iter().map(|(key, value)| (key.into(), value)));
            plist::Value::Dictionary(fields)
        };
        let development = metadata(
            "Apple Development: Private Name (TEAM123)",
            vec![
                ("MinimumOSVersion", " 17.0\n".into()),
                (
                    "Entitlements",
                    plist::Value::Dictionary(plist::Dictionary::from_iter([(
                        String::from("get-task-allow"),
                        plist::Value::Boolean(true),
                    )])),
                ),
            ],
        );
        assert_eq!(
            app_signing_kind(Some(&development), false, false),
            AppSigningKind::Development
        );
        assert_eq!(
            app_minimum_os_version(&development).as_deref(),
            Some("17.0")
        );
        assert_eq!(app_debuggable(&development), Some(true));

        let testflight = metadata(
            "Apple iPhone OS Application Signing",
            vec![("BetaExternalVersionIdentifier", 123_u64.into())],
        );
        assert_eq!(
            app_signing_kind(Some(&testflight), false, false),
            AppSigningKind::TestFlight
        );
        assert_eq!(
            app_signing_kind(
                Some(&metadata("Apple iPhone OS Application Signing", vec![])),
                false,
                false,
            ),
            AppSigningKind::AppStore
        );
        assert_eq!(
            app_signing_kind(
                Some(&metadata("iPhone Distribution: Private Company", vec![])),
                false,
                false,
            ),
            AppSigningKind::Distribution
        );
        assert_eq!(
            app_signing_kind(Some(&testflight), true, false),
            AppSigningKind::System
        );
        assert_eq!(
            app_signing_kind(None, false, false),
            AppSigningKind::Unknown
        );
    }

    #[test]
    fn rejects_unbounded_app_metadata_text() {
        let value = plist::Value::Dictionary(plist::Dictionary::from_iter([(
            String::from("MinimumOSVersion"),
            plist::Value::String("x".repeat(33)),
        )]));
        assert_eq!(app_minimum_os_version(&value), None);
        let invalid = plist::Value::Dictionary(plist::Dictionary::from_iter([(
            String::from("MinimumOSVersion"),
            plist::Value::String("17.0 beta".into()),
        )]));
        assert_eq!(app_minimum_os_version(&invalid), None);
    }
}
