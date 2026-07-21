// The async device session: connect over the tunnel, bring up the screen media
// stream (which both sources the video AND holds open the HID auth gate), then
// run the video pipeline and dispatch input commands to the device's HID surfaces.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::ChildStderr;
use tokio::sync::Notify;
use tokio::sync::mpsc::UnboundedReceiver;

use idevice::{
    IdeviceError, IdeviceService, ReadWrite, RsdService,
    core_device::{
        CallInfoBlob, CoreDeviceError, DataInclusionPolicy, DisplayServiceClient,
        GENERAL_PASTEBOARD, HevcDepacketizer, Orientation as DevOrientation,
        OrientationServiceClient, PasteboardServiceClient, PasteboardSnapshot, ReportBlock,
        RotationDirection, RtpPacket, SenderReport, UTI_PNG, build_frame_ack,
        build_keyframe_request, build_liveness, build_rctl, build_screen_audio_offer,
        build_screen_video_offer, build_start_audio_parameters, build_start_video_parameters,
        hid::{
            ButtonState, DIGITIZER_SURFACE_MAIN_TOUCHSCREEN, IndigoHidClient,
            TOUCHSCREEN_STATE_CONTACT, TOUCHSCREEN_STATE_RELEASE,
        },
        is_rtcp,
    },
    core_device_proxy::CoreDeviceProxy,
    lockdown::LockdownClient,
    provider::IdeviceProvider,
    rsd::RsdHandshake,
    springboardservices::{InterfaceOrientation, SpringBoardServicesClient},
    tcp::handle::{AdapterHandle, UdpSocketHandle},
    usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdConnection, UsbmuxdDevice},
};
use tokio::process::ChildStdin;

use crate::decode;
use crate::hid::{UniversalHidClient, build_multitouch_report};
use crate::protocol::{
    ActiveSlot, ClipboardEvent, ClipboardSlot, ConnKind, ControlCmd, DeviceInfo, DeviceListSlot,
    ErrorSlot, FrameSlot, InputCmd, InputSink, KeyMods, Orientation, OrientationSlot, RotateDir,
    StatusSlot, clipboard_preview,
};

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
/// RTCP Receiver Report interval. AVConference uses RTCP for liveness; if reports
/// stop, the device's sender eventually stops too and the screen freezes.
const RTCP_REPORT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
/// The local UDP port we tell the device to send video RTP/RTCP *from*. Used as
/// the default RTCP destination until we observe where the device's RTCP originates.
const VIDEO_SENDER_PORT: u16 = 50001;
/// Keep the display rotation in sync when an app switches between portrait and
/// landscape. The screen stream itself is always native portrait.
const ORIENTATION_POLL_INTERVAL: Duration = Duration::from_millis(500);

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

/// Supervise the device session: enumerate attached devices for the picker,
/// connect to one, and tear down / reconnect when the selection changes.
#[allow(clippy::too_many_arguments)]
pub async fn manage(
    initial_udid: Option<String>,
    repaint: impl Fn() + Send + Clone + 'static,
    frames: FrameSlot,
    status: StatusSlot,
    clipboard: ClipboardSlot,
    orientation_view: OrientationSlot,
    device_list: DeviceListSlot,
    active: ActiveSlot,
    error: ErrorSlot,
    input_sink: InputSink,
    mut control_rx: UnboundedReceiver<ControlCmd>,
) {
    // Cache of UDID -> DeviceName so a refresh doesn't re-query lockdown.
    let mut names: HashMap<String, String> = HashMap::new();

    // Auto-pick the first device only when no UDID was given, and only until we've
    // connected once: after a session ends we drop to idle rather than hot-loop.
    let mut auto_pick = initial_udid.is_none();
    let mut target = initial_udid;

    loop {
        device_list.set(enumerate_devices(&mut names).await);

        if target.is_none()
            && auto_pick
            && let Some(first) = device_list.get().first()
        {
            target = Some(first.udid.clone());
            auto_pick = false;
        }

        let Some(udid) = target.clone() else {
            active.set(None);
            status.set("no device - pick one from the menu");
            tokio::select! {
                cmd = control_rx.recv() => match cmd {
                    Some(ControlCmd::Connect(u)) => target = Some(u),
                    Some(ControlCmd::Refresh) => {}
                    Some(ControlCmd::Quit) | None => return,
                },
                _ = tokio::time::sleep(IDLE_RESCAN) => {}
            }
            continue;
        };

        // Per-session input channel, published so the UI's input reaches it.
        let (in_tx, in_rx) = tokio::sync::mpsc::unbounded_channel();
        input_sink.set(Some(in_tx.clone()));
        active.set(Some(udid.clone()));
        error.set(None);

        let session = run(
            Some(udid.clone()),
            repaint.clone(),
            frames.clone(),
            status.clone(),
            clipboard.clone(),
            orientation_view.clone(),
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
                    Some(ControlCmd::Connect(u)) if u != udid => break Next::Switch(u),
                    Some(ControlCmd::Connect(_)) => {} // already on this device
                    Some(ControlCmd::Refresh) => {
                        device_list.set(enumerate_devices(&mut names).await);
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
async fn enumerate_devices(names: &mut HashMap<String, String>) -> Vec<DeviceInfo> {
    let mut usbmuxd = match UsbmuxdConnection::default().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("unable to connect to usbmuxd: {e:?}");
            return Vec::new();
        }
    };
    let addr = match UsbmuxdAddr::from_env_var() {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!("bad usbmuxd addr: {e:?}");
            return Vec::new();
        }
    };
    let devs = match usbmuxd.get_devices().await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("unable to list devices: {e:?}");
            return Vec::new();
        }
    };

    let mut preferred = HashMap::<String, UsbmuxdDevice>::new();
    for dev in devs {
        match preferred.entry(dev.udid.clone()) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(dev);
            }
            std::collections::hash_map::Entry::Occupied(mut entry)
                if connection_priority(&dev.connection_type)
                    < connection_priority(&entry.get().connection_type) =>
            {
                entry.insert(dev);
            }
            _ => {}
        }
    }

    let mut selected: Vec<_> = preferred.into_values().collect();
    selected.sort_by(|a, b| a.udid.cmp(&b.udid));

    let mut out = Vec::with_capacity(selected.len());
    for dev in selected {
        let connection = match dev.connection_type {
            Connection::Usb => ConnKind::Usb,
            Connection::Network(_) => ConnKind::Network,
            Connection::Unknown(_) => ConnKind::Other,
        };
        let name = match names.get(&dev.udid) {
            Some(n) => n.clone(),
            None => {
                let n = fetch_device_name(&dev, &addr)
                    .await
                    .unwrap_or_else(|| dev.udid.clone());
                names.insert(dev.udid.clone(), n.clone());
                n
            }
        };
        out.push(DeviceInfo {
            udid: dev.udid,
            name,
            connection,
        });
    }
    out
}

/// Resolve a device's `DeviceName` over lockdown, with a timeout. Returns `None`
/// (caller falls back to the UDID) if the device can't be reached or named.
async fn fetch_device_name(dev: &UsbmuxdDevice, addr: &UsbmuxdAddr) -> Option<String> {
    let provider = dev.to_provider(addr.clone(), "devicehub_rs");
    let lookup = async {
        let mut lockdown = LockdownClient::connect(&provider).await.ok()?;
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
pub async fn run(
    udid: Option<String>,
    repaint: impl Fn() + Send + 'static,
    frames: FrameSlot,
    status: StatusSlot,
    clipboard: ClipboardSlot,
    orientation_view: OrientationSlot,
    mut input_rx: UnboundedReceiver<InputCmd>,
) -> Result<(), String> {
    status.set("connecting to device...");
    let (provider, connection) = connect_provider(udid).await?;
    let device_identity = read_device_identity(&*provider).await;
    if let Some(identity) = &device_identity {
        tracing::info!(
            product_type = %identity.product_type,
            product_version = %identity.product_version,
            "connected device identity"
        );
    }

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

    // Our RTCP SSRC. MUST be declared in the video offer (field 5.1) so the device
    // associates our RTCP feedback with the stream; otherwise it's ignored.
    let our_ssrc = uuid::Uuid::new_v4().as_u128() as u32;

    status.set("starting screen media stream...");
    let media = start_screen_media_stream(
        &mut adapter,
        &mut handshake,
        our_ssrc,
        device_identity.as_ref(),
        connection,
    )
    .await?;

    // HID surfaces only authenticate once the media stream is up; give backboardd
    // a moment to re-match them before connecting.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    status.set("connecting HID...");
    let mut touch = UniversalHidClient::connect_rsd(&mut adapter, &mut handshake)
        .await
        .map_err(|e| format!("no universalhidservice: {e:?}"))?;
    touch.dump_services_from_env().await;
    let mut indigo = IndigoHidClient::connect_rsd(&mut adapter, &mut handshake)
        .await
        .map_err(|e| format!("no hid.indigo: {e:?}"))?;

    // Clipboard sync is best-effort: run without it if the service is unavailable.
    let pasteboard = match PasteboardServiceClient::connect_rsd(&mut adapter, &mut handshake).await
    {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::warn!("no pasteboardservice; clipboard sync disabled: {e:?}");
            None
        }
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
            if let Err(error) = refresh_interface_orientation(&mut client, &orientation_view).await
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

    // video UDP -> depacketize -> ffmpeg stdin ; ffmpeg stdout -> frames.
    let (mut child, ffmpeg_in, ffmpeg_out, ffmpeg_err) =
        decode::spawn_ffmpeg().map_err(|e| format!("failed to spawn ffmpeg: {e}"))?;

    status.set("connected");

    // A stable CNAME for our RTCP SDES (identifies this receiver endpoint).
    let cname = format!("devicehub@{}", adapter.host_ip());

    // Hold the audio socket bound for the stream's lifetime (dropping it unbinds
    // the port). Keep the display client to stop the stream on teardown.
    let _audio_udp = media.audio_udp;
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
    // The hevc channel decouples ffmpeg writing from the RTP receive loop, which
    // sends per-frame RTCP ACKs the encoder depends on and must not stall on
    // ffmpeg's stdin backpressure (which spikes under heavy motion).
    let (hevc_tx, hevc_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let orientation_watch_view = orientation_view.clone();
    let orientation_task = async move {
        match springboard {
            Some(client) => watch_interface_orientation(client, orientation_watch_view).await,
            None => std::future::pending::<()>().await,
        }
    };

    tokio::select! {
        _ = video_task(video_udp.clone(), hevc_tx, rtcp.clone(), our_ssrc) => {
            tracing::warn!("video task ended early");
        }
        _ = ffmpeg_writer(ffmpeg_in, hevc_rx) => {
            tracing::warn!("ffmpeg writer ended");
        }
        _ = decode::read_frames(ffmpeg_out, frames, frame_beat.clone(), repaint) => {
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
        _ = clipboard_task(pasteboard, clipboard, &mut adapter, &mut handshake) => {}
        _ = orientation_task => {}
        _ = input_loop(&mut touch, &mut indigo, &mut orientation, &orientation_view, &mut input_rx) => {}
    }

    status.set("stopping...");
    display.stop_media_stream().await.ok();
    child.start_kill().ok();
    // `proxy`, `adapter`, `handshake` drop here, tearing down the tunnel.
    Ok(())
}

/// Dispatch input until the UI shuts us down or the channel closes.
async fn input_loop(
    touch: &mut UniversalHidClient<Box<dyn ReadWrite>>,
    indigo: &mut IndigoHidClient<Box<dyn ReadWrite>>,
    orientation: &mut Option<OrientationServiceClient<Box<dyn ReadWrite>>>,
    orientation_view: &OrientationSlot,
    input_rx: &mut UnboundedReceiver<InputCmd>,
) {
    while let Some(cmd) = input_rx.recv().await {
        if matches!(cmd, InputCmd::Shutdown) {
            break;
        }
        if let Err(e) = dispatch(touch, indigo, orientation, orientation_view, cmd).await {
            tracing::warn!("input dispatch failed: {e:?}");
        }
    }
}

/// How often we poll the host clipboard for host -> device changes (arboard has no
/// change notification). The device -> host direction is push-driven when available.
const CLIPBOARD_POLL: std::time::Duration = std::time::Duration::from_millis(600);
/// Max characters in the UI's clipboard-activity preview.
const CLIPBOARD_PREVIEW_LEN: usize = 48;

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
    activity: ClipboardSlot,
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
) {
    let Some(mut pb) = pasteboard else {
        std::future::pending::<()>().await;
        return;
    };
    let mut clip = match arboard::Clipboard::new() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("no host clipboard; clipboard sync disabled: {e:?}");
            std::future::pending::<()>().await;
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
    loop {
        // The `recv_push` future is dropped when the tick wins - safe because the
        // XPC read path buffers partial reads. Resolve the borrow of `pb` before
        // the match body, which reuses it.
        let push = tokio::select! {
            r = pb.recv_push() => Some(r),
            _ = tick.tick() => None,
        };

        match push {
            // device -> host (push)
            Some(Ok(snap)) => apply_device_snapshot(&snap, &mut clip, &activity, &mut state),
            Some(Err(e)) => {
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
            None => {
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
                                preview: format!("🖼 image {w}×{h}"),
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
                        preview: format!("🖼 image {w}×{h}"),
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
    match PasteboardServiceClient::connect_rsd(adapter, handshake).await {
        Ok(c) => {
            tracing::info!("clipboard: reconnected pasteboard service");
            Some(c)
        }
        Err(e) => {
            tracing::warn!("clipboard reconnect failed: {e:?}");
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
    hevc_tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    rtcp: Arc<Mutex<RtcpShared>>,
    our_ssrc: u32,
) {
    let mut depacketizer = HevcDepacketizer::new();
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
                {
                    let mut s = rtcp.lock().unwrap();
                    s.media_ssrc.get_or_insert(pkt.ssrc);
                    s.stats.on_packet(pkt.sequence_number);
                    if pkt.marker {
                        s.frames = s.frames.wrapping_add(1);
                    }
                }
                // Per-frame ACK (disabled by default - see `send_frame_ack`). The
                // marker bit ends an access unit; we ACK only intact frames.
                if send_frame_ack {
                    au_pkts = au_pkts.wrapping_add(1);
                    if pkt.marker {
                        let complete = match prev_marker_seq {
                            Some(prev) => {
                                let expected = pkt.sequence_number.wrapping_sub(prev) as u32;
                                au_pkts >= expected
                            }
                            None => true,
                        };
                        if complete {
                            let ack = build_frame_ack(our_ssrc, pkt.timestamp);
                            udp.send_to(dg.source_port, ack).await.ok();
                        }
                        prev_marker_seq = Some(pkt.sequence_number);
                        au_pkts = 0;
                    }
                }
                depacketizer.push(pkt.sequence_number, pkt.timestamp, pkt.payload);
                let out = depacketizer.take_output();
                if !out.is_empty() {
                    if let Some(f) = &mut dump {
                        f.write_all(&out).await.ok();
                    }
                    // Non-blocking hand-off so this loop never stalls on ffmpeg
                    // backpressure and keeps ACKs timely.
                    if hevc_tx.send(out).is_err() {
                        tracing::info!("ffmpeg writer gone; ending video task");
                        break;
                    }
                }
            }
            Err(e) => {
                tracing::warn!("video udp recv error: {e:?}");
                break;
            }
        }
    }
}

/// Drain depacketized Annex-B from [`video_task`] into ffmpeg's stdin. On its own
/// task so ffmpeg backpressure never stalls the RTP receive loop's RTCP ACKs.
async fn ffmpeg_writer(
    mut ffmpeg_in: ChildStdin,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
) {
    while let Some(buf) = rx.recv().await {
        if ffmpeg_in.write_all(&buf).await.is_err() {
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

#[derive(Debug)]
struct DeviceIdentity {
    product_type: String,
    product_version: String,
}

async fn read_device_identity(provider: &dyn IdeviceProvider) -> Option<DeviceIdentity> {
    let mut lockdown = LockdownClient::connect(provider).await.ok()?;
    let product_type = lockdown
        .get_value(Some("ProductType"), None)
        .await
        .ok()?
        .as_string()?
        .to_string();
    let product_version = lockdown
        .get_value(Some("ProductVersion"), None)
        .await
        .ok()?
        .as_string()?
        .to_string();
    Some(DeviceIdentity {
        product_type,
        product_version,
    })
}

fn format_media_start_error(
    stream: &str,
    error: IdeviceError,
    identity: Option<&DeviceIdentity>,
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
    identity: Option<&DeviceIdentity>,
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
    client
        .start_media_stream(audio_params)
        .await
        .map_err(|error| format_media_start_error("audio", error, identity))?;

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

    Ok(ScreenMediaStream {
        client,
        audio_udp,
        video_udp,
        rtcp_udp,
    })
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
    identity: Option<&DeviceIdentity>,
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

/// Connect to the first (or named) device over usbmuxd and build a provider.
async fn connect_provider(
    udid: Option<String>,
) -> Result<(Box<dyn IdeviceProvider>, ConnKind), String> {
    let mut usbmuxd = UsbmuxdConnection::default()
        .await
        .map_err(|e| format!("unable to connect to usbmuxd: {e:?}"))?;

    let addr = UsbmuxdAddr::from_env_var().map_err(|e| format!("bad usbmuxd addr: {e:?}"))?;

    let devs = usbmuxd
        .get_devices()
        .await
        .map_err(|e| format!("unable to list devices: {e:?}"))?;
    let dev = select_preferred_device(devs, udid.as_deref()).ok_or_else(|| match udid {
        Some(udid) => format!("device {udid} not found"),
        None => "no devices connected".to_string(),
    })?;

    let connection = match &dev.connection_type {
        Connection::Usb => ConnKind::Usb,
        Connection::Network(_) => ConnKind::Network,
        Connection::Unknown(_) => ConnKind::Other,
    };
    tracing::info!(
        udid = %dev.udid,
        connection = connection_label(&dev.connection_type),
        "selected CoreDevice transport"
    );
    Ok((Box::new(dev.to_provider(addr, "devicehub_rs")), connection))
}

// Prefer the cable path when usbmuxd reports the same UDID over both transports.
// CoreDeviceProxy supports displayservice over USB directly, and the cable keeps
// discovery independent of Bonjour and local-network multicast configuration.
fn connection_priority(connection: &Connection) -> u8 {
    match connection {
        Connection::Usb => 0,
        Connection::Network(_) => 1,
        Connection::Unknown(_) => 2,
    }
}

fn connection_label(connection: &Connection) -> &'static str {
    match connection {
        Connection::Network(_) => "Wi-Fi",
        Connection::Usb => "USB",
        Connection::Unknown(_) => "Other",
    }
}

fn select_preferred_device(
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
    use std::net::{IpAddr, Ipv4Addr};

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
    fn prefers_usb_transport_for_duplicate_udid() {
        let usb = UsbmuxdDevice {
            connection_type: Connection::Usb,
            udid: "phone".into(),
            device_id: 1,
        };
        let network = UsbmuxdDevice {
            connection_type: Connection::Network(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            udid: "phone".into(),
            device_id: 2,
        };

        let selected = select_preferred_device(vec![usb, network], Some("phone")).unwrap();
        assert!(matches!(selected.connection_type, Connection::Usb));
    }

    #[test]
    fn falls_back_to_usb_and_filters_by_udid() {
        let other = UsbmuxdDevice {
            connection_type: Connection::Network(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            udid: "other".into(),
            device_id: 1,
        };
        let usb = UsbmuxdDevice {
            connection_type: Connection::Usb,
            udid: "phone".into(),
            device_id: 2,
        };

        let selected = select_preferred_device(vec![other, usb], Some("phone")).unwrap();
        assert!(matches!(selected.connection_type, Connection::Usb));
    }

    #[test]
    fn summarizes_coredevice_9021_without_binary_plist_dump() {
        let error = IdeviceError::CoreDevice(CoreDeviceError::DeviceError(
            r#"Dictionary({"code": Integer(9021), "NSLocalizedDescription": String("Remote control requires iOS 27.0 or later on this device.")})"#.into(),
        ));
        let identity = DeviceIdentity {
            product_type: "iPhone11,2".into(),
            product_version: "26.0".into(),
        };

        let message = format_media_start_error("audio", error, Some(&identity));
        assert!(message.contains("CoreDevice 9021"));
        assert!(message.contains("iPhone11,2 running iOS 26.0"));
        assert!(message.contains("iOS 27.0 or later"));
        assert!(!message.contains("Dictionary"));
    }
}
