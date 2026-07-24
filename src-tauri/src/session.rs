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
    IdeviceError, IdeviceService, ReadWrite, RsdService,
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
    rsd::RsdHandshake,
    springboardservices::{InterfaceOrientation, SpringBoardServicesClient},
    tcp::handle::{AdapterHandle, UdpSocketHandle},
    usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdDevice},
    utils::installation::install_package_with_callback,
};
use tokio::process::ChildStdin;

use crate::audio_output::AudioOutput;
use crate::decode;
use crate::developer_mode;
use crate::hid::{UniversalHidClient, build_multitouch_report};
use crate::protocol::{
    ActiveSlot, AppOperationKind, AppOperationSlot, ClipboardContentKind, ClipboardEvent,
    ClipboardSlot, ConnKind, ControlCmd, DeviceActivationState, DeviceApp, DeviceBattery,
    DeviceDetails, DeviceInfo, DeviceListSlot, DeviceStorage, ErrorSlot, FrameFormat, FrameSlot,
    InputCmd, InputSink, KeyMods, LocationStatus, LocationStatusSlot, Orientation, OrientationSlot,
    RotateDir, StatusSlot, VideoCounters, clipboard_preview, device_selector,
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
}

#[derive(Debug)]
struct QueuedHevcAccessUnit {
    access_unit: HevcAccessUnit,
    enqueued_at: Instant,
}

#[derive(Debug, Default)]
struct AccessUnitAssembler {
    pending: Vec<u8>,
}

impl AccessUnitAssembler {
    fn push(&mut self, bytes: &[u8]) -> Vec<HevcAccessUnit> {
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
                });
            }
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
        })
    }

    fn clear(&mut self) {
        self.pending.clear();
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
/// Per-device budget for resolving `DeviceName` over lockdown; on timeout we fall
/// back to the UDID so a flaky/locked device doesn't stall the picker.
const NAME_TIMEOUT: Duration = Duration::from_secs(2);

/// What the manager should do once the current session is no longer running.
enum Next {
    /// Connect to this UDID.
    Switch(String),
    /// Go idle (no device); wait for the user to pick one.
    Idle,
    /// The UI is gone - exit the manager entirely.
    Quit,
}

#[derive(Debug, Clone, Copy)]
enum DevicePowerAction {
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
    location: LocationStatusSlot,
    performance: performance::PerformanceSlot,
    performance_demand: performance::PerformanceDemand,
    device_logs: crate::device_logs::DeviceLogSlot,
    device_log_demand: crate::device_logs::DeviceLogDemand,
    services: supervisor::ServiceRegistry,
    device_events: crate::device_events::DeviceEventSlot,
    network_capture: crate::network_capture::NetworkCaptureSlot,
    device_conditions: crate::device_conditions::DeviceConditionSlot,
}

#[derive(Clone)]
struct SessionVideo {
    frame_format: FrameFormat,
    counters: VideoCounters,
    frames: FrameSlot,
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
    audio: AudioOutput,
    status: StatusSlot,
    clipboard: ClipboardSlot,
    device_events: crate::device_events::DeviceEventSlot,
    network_capture: crate::network_capture::NetworkCaptureSlot,
    device_conditions: crate::device_conditions::DeviceConditionSlot,
    orientation_view: OrientationSlot,
    device_list: DeviceListSlot,
    active: ActiveSlot,
    error: ErrorSlot,
    app_operation: AppOperationSlot,
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
    let mut wifi = match WifiDiscovery::start(pairing_dir) {
        Ok(discovery) => Some(discovery),
        Err(error) => {
            tracing::warn!(%error, "Wi-Fi discovery unavailable; continuing with usbmuxd");
            None
        }
    };
    // Auto-pick the first device only when no UDID was given, and only until we've
    // connected once: after a session ends we drop to idle rather than hot-loop.
    let mut auto_pick = initial_udid.is_none();
    let mut target = initial_udid;

    loop {
        let (devices, endpoints) = enumerate_devices(&mut names, &mut netmuxd, wifi.as_mut()).await;
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
            && let Some(first) = device_list.get().first()
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
                    Some(ControlCmd::Quit) | None => return,
                },
                _ = tokio::time::sleep(IDLE_RESCAN) => {}
            }
            continue;
        };

        let Some(endpoint) = endpoints.get(&selection_id).cloned() else {
            let message = format!("requested device transport ({selection_id}) not found");
            tracing::warn!(%message);
            active.set(None);
            error.set(Some(message));
            target = None;
            continue;
        };
        let udid = endpoint.udid().to_owned();

        // Per-session input channel, published so the UI's input reaches it.
        let (in_tx, in_rx) = tokio::sync::mpsc::unbounded_channel();
        input_sink.set(Some(in_tx.clone()));
        active.set_selected(udid.clone(), selection_id.clone());
        error.set(None);

        let session = run(
            endpoint,
            SessionVideo {
                frame_format: settings.video_pixel_format(),
                counters: video_counters.clone(),
                frames: frames.clone(),
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
                location: location.clone(),
                performance: performance.clone(),
                performance_demand: performance_demand.clone(),
                device_logs: device_logs.clone(),
                device_log_demand: device_log_demand.clone(),
                services: services.clone(),
                device_events: device_events.clone(),
                network_capture: network_capture.clone(),
                device_conditions: device_conditions.clone(),
            },
            in_rx,
        );
        tokio::pin!(session);

        // Run until the session ends on its own or the UI redirects us.
        let outcome = loop {
            tokio::select! {
                res = &mut session => {
                    if let Err(e) = res {
                        tracing::error!("session ended: {e}");
                        error.set(Some(e));
                    }
                    break Next::Idle;
                }
                cmd = control_rx.recv() => match cmd {
                    Some(ControlCmd::Connect(u)) if u != selection_id && u != udid => break Next::Switch(u),
                    Some(ControlCmd::Connect(_)) => {} // already on this device
                    Some(ControlCmd::Reconnect(u)) => break Next::Switch(u),
                    Some(ControlCmd::Refresh) => {
                        names.clear();
                        let (devices, _) = enumerate_devices(&mut names, &mut netmuxd, wifi.as_mut()).await;
                        device_list.set(devices);
                    }
                    Some(ControlCmd::Quit) | None => break Next::Quit,
                },
            }
        };

        // For user-initiated transitions the session is still live: stop it and
        // wait for teardown so two sessions never fight over the same media stream.
        if !matches!(outcome, Next::Idle) {
            let _ = in_tx.send(InputCmd::Shutdown);
            let _ = tokio::time::timeout(SWITCH_GRACE, &mut session).await;
        }
        input_sink.set(None);
        active.set(None);
        location.set(LocationStatus::default());

        match outcome {
            Next::Switch(u) => target = Some(u),
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
    mut wifi: Option<&mut WifiDiscovery>,
) -> (Vec<DeviceInfo>, HashMap<String, SessionEndpoint>) {
    let netmuxd_addr = netmuxd.ensure_ready().await;
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

    if let (Some(discovery), Some(usbmuxd)) = (wifi.as_deref_mut(), usbmuxd.as_mut()) {
        for device in &devs {
            if !discovery.pairing_needs_refresh(&device.udid) {
                continue;
            }
            match usbmuxd.get_pair_record(&device.udid).await {
                Ok(pairing_file) => {
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
                Err(error) => tracing::debug!(
                    device_id = %crate::diagnostics::device_id_fingerprint(&device.udid),
                    ?error,
                    "pairing record unavailable for Wi-Fi discovery"
                ),
            }
        }
    }

    let mut selected = devs;
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

    // netmuxd already performs authenticated Bonjour discovery. Run our scanner
    // only as the fallback so one process owns discovery at a time.
    if !using_netmuxd && let Some(discovery) = wifi {
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
            });
            endpoints.insert(id, SessionEndpoint::Wifi(Box::new(endpoint)));
        }
    }
    out.sort_by(|a, b| {
        a.udid.cmp(&b.udid).then_with(|| {
            connection_kind_priority(a.connection).cmp(&connection_kind_priority(b.connection))
        })
    });
    (out, endpoints)
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

/// Run the whole session to completion. Returns an error string suitable for the
/// status bar if setup fails; otherwise runs until a [`InputCmd::Shutdown`] (or
/// the UI dropping the input channel).
async fn run(
    endpoint: SessionEndpoint,
    video: SessionVideo,
    repaint: impl Fn() + Send + 'static,
    clipboard: ClipboardSlot,
    views: SessionViews,
    mut input_rx: UnboundedReceiver<InputCmd>,
) -> Result<(), String> {
    views.status.set("connecting to device...");
    let requested_udid = endpoint.udid().to_owned();
    let (provider, connection) = connect_provider(endpoint).await?;
    let device_details = read_device_details(&*provider, requested_udid).await;
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
    let proxy = CoreDeviceProxy::connect(&*provider)
        .await
        .map_err(|e| format!("no core device proxy: {e:?}"))?;
    let rsd_port = proxy.tunnel_info().server_rsd_port;
    let adapter = proxy
        .create_software_tunnel()
        .map_err(|e| format!("no software tunnel: {e:?}"))?;
    let mut adapter = adapter.to_async_handle();
    let stream = adapter
        .connect(rsd_port)
        .await
        .map_err(|e| format!("RSD connect failed: {e:?}"))?;
    let mut handshake = RsdHandshake::new(stream)
        .await
        .map_err(|e| format!("RSD handshake failed: {e:?}"))?;

    views.performance.reset();
    views.device_logs.reset();
    let mut supervisor = supervisor::ServiceSupervisor::new(views.services.clone());
    supervisor.spawn(crate::heartbeat::supervise(
        provider.clone(),
        supervisor.reporter("device.heartbeat"),
        supervisor.shutdown_receiver(),
    ));
    supervisor.spawn(crate::device_logs::supervise(
        provider.clone(),
        views.device_logs.clone(),
        supervisor.reporter("device.logs"),
        views.device_log_demand.subscribe(),
        supervisor.shutdown_receiver(),
    ));
    supervisor.spawn(crate::device_events::supervise(
        provider.clone(),
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

    views.location.set(LocationStatus::default());
    let (location_sender, location_receiver) = tokio::sync::mpsc::channel(8);
    supervisor.spawn(location::supervise(
        adapter.clone(),
        handshake.clone(),
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
    let (app_documents_sender, app_documents_receiver) = tokio::sync::mpsc::channel(8);
    supervisor.spawn(crate::app_documents::serve(
        adapter.clone(),
        handshake.clone(),
        app_documents_receiver,
        supervisor.shutdown_receiver(),
    ));
    let (screen_capture_sender, screen_capture_receiver) = tokio::sync::mpsc::channel(1);
    supervisor.spawn(crate::screen_capture::serve(
        adapter.clone(),
        handshake.clone(),
        screen_capture_receiver,
        supervisor.shutdown_receiver(),
    ));
    let (network_capture_sender, network_capture_receiver) = tokio::sync::mpsc::channel(4);
    supervisor.spawn(crate::network_capture::serve(
        adapter.clone(),
        handshake.clone(),
        network_capture_receiver,
        views.network_capture.clone(),
        supervisor.reporter("network.capture"),
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
        provider.clone(),
        provisioning_receiver,
        supervisor.reporter("device.provisioning"),
        supervisor.shutdown_receiver(),
    ));
    let device_management_services = DeviceManagementServices {
        icons: app_icon_sender,
        documents: app_documents_sender,
        screen_capture: screen_capture_sender,
        network_capture: network_capture_sender,
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

    // video UDP -> depacketize -> ffmpeg stdin ; ffmpeg stdout -> frames.
    let frame_format = video.frame_format;
    let (mut child, ffmpeg_in, ffmpeg_out, ffmpeg_err) =
        decode::spawn_ffmpeg(frame_format).map_err(|e| format!("failed to spawn ffmpeg: {e}"))?;

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
        _ = ffmpeg_writer(ffmpeg_in, hevc_queue) => {
            tracing::warn!("ffmpeg writer ended");
        }
        _ = decode::read_frames(
            ffmpeg_out,
            frame_format,
            video.frames,
            video.counters,
            frame_beat.clone(),
            repaint,
        ) => {
            tracing::warn!("decode task ended early");
        }
        _ = watch_decode_errors(ffmpeg_err, corruption.clone()) => {
            tracing::warn!("ffmpeg stderr watcher ended");
        }
        _ = stall_watchdog(frame_beat, &corruption) => {}
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
    views.status.set("stopping...");
    display.stop_media_stream().await.ok();
    child.start_kill().ok();
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
    documents: tokio::sync::mpsc::Sender<crate::app_documents::AppDocumentCommand>,
    screen_capture: tokio::sync::mpsc::Sender<crate::screen_capture::ScreenCaptureCommand>,
    network_capture: tokio::sync::mpsc::Sender<crate::network_capture::NetworkCaptureCommand>,
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

struct DeviceManagement {
    provider: Arc<dyn IdeviceProvider>,
    power: DevicePowerSlot,
    app_operation: AppOperationSlot,
    operation_task: Option<ActiveAppOperation>,
    details: Option<DeviceDetails>,
    app_service: Option<AppServiceClient<Box<dyn ReadWrite>>>,
    installation_proxy: Option<InstallationProxyClient>,
    services: DeviceManagementServices,
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
            services,
        }
    }

    fn fallback(
        provider: Arc<dyn IdeviceProvider>,
        app_operation: AppOperationSlot,
        details: Option<DeviceDetails>,
        installation_proxy: Option<InstallationProxyClient>,
        services: DeviceManagementServices,
    ) -> Self {
        Self::new(
            provider,
            app_operation,
            details,
            None,
            installation_proxy,
            services,
        )
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

    async fn install_app(&mut self, path: PathBuf) -> Result<(), String> {
        self.clear_finished_operation();
        let (path, label) = validate_ipa_path(&path).await?;
        let id = self.app_operation.start(AppOperationKind::Install, label)?;
        self.app_operation.update(id, "uploading", None);

        let provider = self.provider.clone();
        let operation = self.app_operation.clone();
        let task_operation = operation.clone();
        let handle = tokio::spawn(async move {
            let result = install_package_with_callback(
                provider.as_ref(),
                path,
                None,
                |(progress, (operation, operation_id))| async move {
                    operation.update(operation_id, "installing", Some(progress.min(100) as u8));
                },
                (task_operation, id),
            )
            .await;
            match result {
                Ok(()) => operation.succeed(id),
                Err(error) => operation.fail(id, format!("unable to install IPA: {error:?}")),
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
            InputCmd::ListApps(reply) => {
                let result =
                    list_device_apps(self.app_service.as_mut(), self.installation_proxy.as_mut())
                        .await;
                let _ = reply.send(result);
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
                let result = match self.app_service.as_mut() {
                    Some(client) => client
                        .launch_application(bundle_id, &[], true, false, None, None, None)
                        .await
                        .map(|_| ())
                        .map_err(|error| format!("unable to launch app: {error:?}")),
                    None => Err("app launch requires the CoreDevice AppService".to_string()),
                };
                let _ = reply.send(result);
                None
            }
            InputCmd::StopApp { bundle_id, reply } => {
                let result = match self.app_service.as_mut() {
                    Some(client) => stop_device_app(client, &bundle_id).await,
                    None => Err("app stop requires the CoreDevice AppService".to_string()),
                };
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
            InputCmd::InstallApp { path, reply } => {
                let result = self.install_app(path).await;
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

async fn validate_ipa_path(path: &Path) -> Result<(PathBuf, String), String> {
    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ipa"))
    {
        return Err("selected file must have an .ipa extension".into());
    }
    let canonical = tokio::fs::canonicalize(path)
        .await
        .map_err(|error| format!("unable to access IPA: {error}"))?;
    let metadata = tokio::fs::metadata(&canonical)
        .await
        .map_err(|error| format!("unable to inspect IPA: {error}"))?;
    if !metadata.is_file() {
        return Err("selected IPA path is not a regular file".into());
    }
    let label = canonical
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "selected IPA has no valid file name".to_string())?
        .to_string();
    Ok((canonical, label))
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
) -> Result<Vec<DeviceApp>, String> {
    if let Some(client) = app_service {
        match client.list_apps(false, true, false, false, false).await {
            Ok(entries) => {
                let document_apps = match installation_proxy.as_deref_mut() {
                    Some(client) => match client.get_apps(Some("User"), None).await {
                        Ok(apps) => apps
                            .iter()
                            .filter(|(_, value)| installation_supports_documents(value))
                            .map(|(bundle_id, _)| bundle_id.clone())
                            .collect::<std::collections::HashSet<_>>(),
                        Err(error) => {
                            tracing::warn!(
                                "app document capability metadata unavailable: {error:?}"
                            );
                            std::collections::HashSet::new()
                        }
                    },
                    None => std::collections::HashSet::new(),
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
                            let documents_available =
                                document_apps.contains(&entry.bundle_identifier);
                            DeviceApp {
                                is_running: processes.as_ref().map(|processes| {
                                    processes.iter().any(|process| {
                                        process.executable_url.as_ref().is_some_and(|executable| {
                                            process_executable_belongs_to_app(
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
                                is_developer_app: entry.is_developer_app,
                                documents_available,
                            }
                        })
                        .collect(),
                ));
            }
            Err(error) => tracing::warn!(
                "CoreDevice AppService list failed; using installation proxy: {error:?}"
            ),
        }
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
    Some(DeviceApp {
        bundle_id,
        name,
        version: string("CFBundleShortVersionString"),
        bundle_version: string("CFBundleVersion"),
        is_removable: boolean("IsRemovable").unwrap_or(false),
        is_first_party: boolean("IsFirstParty")
            .unwrap_or_else(|| signer.contains("Apple iPhone OS Application Signing")),
        is_developer_app: boolean("IsXcodeManaged").unwrap_or(false)
            || signer.contains("Apple Development"),
        documents_available: installation_supports_documents(value),
        is_running: None,
    })
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

fn normalized_app_path(path: &str) -> &str {
    path.strip_prefix("file://localhost")
        .or_else(|| path.strip_prefix("file://"))
        .unwrap_or(path)
        .trim_end_matches('/')
}

fn process_executable_belongs_to_app(app_path: &str, executable_path: &str) -> bool {
    let app_path = normalized_app_path(app_path);
    let executable_path = normalized_app_path(executable_path);
    executable_path
        .rsplit_once('/')
        .is_some_and(|(parent, executable)| parent == app_path && !executable.is_empty())
}

async fn stop_device_app(
    client: &mut AppServiceClient<Box<dyn ReadWrite>>,
    bundle_id: &str,
) -> Result<bool, String> {
    let apps = client
        .list_apps(false, true, false, false, false)
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
                process_executable_belongs_to_app(&app.path, &executable.relative)
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
        | InputCmd::ListApps(_)
        | InputCmd::GetAppIcon { .. }
        | InputCmd::TakeScreenshot(_)
        | InputCmd::NetworkCapture(_)
        | InputCmd::DeviceCondition(_)
        | InputCmd::AppDocuments(_)
        | InputCmd::RestartDevice(_)
        | InputCmd::ShutdownDevice(_)
        | InputCmd::Provisioning(_)
        | InputCmd::LaunchApp { .. }
        | InputCmd::StopApp { .. }
        | InputCmd::ListCrashReports(_)
        | InputCmd::ExportCrashReport { .. }
        | InputCmd::InstallApp { .. }
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
                    let mut access_units = assembler.push(&out);
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
    let string = |key: &str| {
        values
            .get(key)
            .and_then(plist::Value::as_string)
            .map(ToOwned::to_owned)
    };
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
        udid: string("UniqueDeviceID").unwrap_or(requested_udid),
        name: string("DeviceName").unwrap_or_else(|| "iOS Device".to_string()),
        product_type: string("ProductType").unwrap_or_else(|| "Unknown".to_string()),
        product_version: string("ProductVersion").unwrap_or_else(|| "Unknown".to_string()),
        build_version: string("BuildVersion"),
        hardware_model: string("HardwareModel"),
        serial_number: string("SerialNumber"),
        ecid: integer("UniqueChipID").map(|value| value.to_string()),
        total_disk_capacity,
        storage,
        activation_state: None,
        developer_mode_enabled: None,
        battery: None,
    })
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
    let unsigned = |dictionary: &plist::Dictionary, key: &str| {
        dictionary
            .get(key)
            .and_then(plist::Value::as_unsigned_integer)
    };
    let signed = |dictionary: &plist::Dictionary, key: &str| {
        dictionary
            .get(key)
            .and_then(plist::Value::as_signed_integer)
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
    let design_capacity_mah = battery_data.and_then(|data| unsigned(data, "DesignCapacity"));
    let full_charge_capacity_mah =
        battery_data.and_then(|data| unsigned(data, "FullChargeCapacity"));
    let health_percent = design_capacity_mah
        .filter(|capacity| *capacity > 0)
        .zip(full_charge_capacity_mah)
        .map(|(design, full)| (full as f64 * 100.0 / design as f64).clamp(0.0, 100.0));

    DeviceBattery {
        level_percent: unsigned(values, "CurrentCapacity")
            .or_else(|| battery_data.and_then(|data| unsigned(data, "CurrentCapacity")))
            .filter(|value| *value <= 100)
            .map(|value| value as u8),
        is_charging: boolean(values, "IsCharging")
            .or_else(|| charger_data.and_then(|data| boolean(data, "IsCharging"))),
        external_connected: boolean(values, "ExternalConnected")
            .or_else(|| boolean(values, "AppleRawExternalConnected")),
        fully_charged: boolean(values, "FullyCharged")
            .or_else(|| battery_data.and_then(|data| boolean(data, "FullyCharged"))),
        cycle_count: unsigned(values, "CycleCount"),
        voltage_mv: unsigned(values, "Voltage")
            .or_else(|| unsigned(values, "AppleRawBatteryVoltage")),
        instant_amperage_ma: signed(values, "InstantAmperage")
            .or_else(|| signed(values, "Amperage")),
        design_capacity_mah,
        full_charge_capacity_mah,
        health_percent,
        time_remaining_minutes: unsigned(values, "TimeRemaining")
            .or_else(|| unsigned(values, "AvgTimeToEmpty"))
            .filter(|minutes| *minutes <= 7 * 24 * 60),
        adapter_watts: adapter.and_then(|details| unsigned(details, "Watts")),
        adapter_name: adapter
            .and_then(|details| details.get("Name"))
            .and_then(plist::Value::as_string)
            .map(ToOwned::to_owned),
    }
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
        assert!(assembler.push(&first_chunk).is_empty());

        let mut second_chunk = HEVC_AUD[3..].to_vec();
        second_chunk.extend_from_slice(&second);
        let completed = assembler.push(&second_chunk);
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].bytes, first);
        assert!(!completed[0].is_irap);

        let completed = assembler.push(HEVC_AUD);
        assert_eq!(completed.len(), 1);
        assert!(completed[0].bytes.starts_with(HEVC_AUD));
        assert!(completed[0].is_irap);
    }

    #[test]
    fn finishes_access_unit_at_complete_rtp_marker() {
        let irap = [0, 0, 0, 1, 0x26, 0x01, 0xbb];
        let mut assembler = AccessUnitAssembler::default();

        assert!(assembler.push(&irap).is_empty());
        let completed = assembler.finish().unwrap();
        assert_eq!(completed.bytes, irap);
        assert!(completed.is_irap);
        assert!(assembler.finish().is_none());
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
            },
            DeviceInfo {
                id: device_selector("phone", ConnKind::Network),
                udid: "phone".into(),
                name: "iPhone".into(),
                connection: ConnKind::Network,
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
            hardware_model: None,
            serial_number: None,
            ecid: None,
            total_disk_capacity: None,
            storage: None,
            activation_state: None,
            developer_mode_enabled: None,
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
        ]));

        let app = device_app_from_installation("com.example.game".into(), &value).unwrap();
        assert_eq!(app.bundle_id, "com.example.game");
        assert_eq!(app.name, "Example Game");
        assert_eq!(app.version.as_deref(), Some("2.4"));
        assert_eq!(app.bundle_version.as_deref(), Some("42"));
        assert!(app.is_developer_app);
        assert!(app.documents_available);
        assert!(!app.is_removable);
        assert_eq!(app.is_running, None);
    }

    #[test]
    fn matches_only_an_apps_main_executable() {
        let app = "/private/var/containers/Bundle/Application/UUID/Example.app/";
        assert!(process_executable_belongs_to_app(
            app,
            "file:///private/var/containers/Bundle/Application/UUID/Example.app/Example"
        ));
        assert!(process_executable_belongs_to_app(
            "file://localhost/private/var/containers/Bundle/Application/UUID/Example.app",
            "/private/var/containers/Bundle/Application/UUID/Example.app/Example"
        ));
        assert!(!process_executable_belongs_to_app(
            app,
            "/private/var/containers/Bundle/Application/UUID/Example.app/PlugIns/Widget.appex/Widget"
        ));
        assert!(!process_executable_belongs_to_app(
            app,
            "/private/var/containers/Bundle/Application/OTHER/Example.app/Example"
        ));
    }

    #[tokio::test]
    async fn validates_and_canonicalizes_ipa_files() {
        let directory =
            std::env::temp_dir().join(format!("devicehub-mask-ipa-test-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&directory).await.unwrap();
        let ipa = directory.join("Example.IPA");
        tokio::fs::write(&ipa, b"placeholder").await.unwrap();

        let (validated, label) = validate_ipa_path(&ipa).await.unwrap();
        assert_eq!(validated, tokio::fs::canonicalize(&ipa).await.unwrap());
        assert_eq!(label, "Example.IPA");
        assert!(
            validate_ipa_path(&directory.join("Example.zip"))
                .await
                .is_err()
        );

        let fake_ipa_directory = directory.join("folder.ipa");
        tokio::fs::create_dir(&fake_ipa_directory).await.unwrap();
        assert!(validate_ipa_path(&fake_ipa_directory).await.is_err());
        let _ = tokio::fs::remove_dir_all(directory).await;
    }
}
