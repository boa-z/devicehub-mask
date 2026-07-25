// The async device session: connect over the tunnel, bring up the screen media
// stream (which both sources the video AND holds open the HID auth gate), then
// run the video pipeline and dispatch input commands to the device's HID surfaces.

mod apps;
mod clipboard;
mod discovery;
mod manager;
mod media;
mod orientation;
mod rtcp;
mod services;
mod transport;
mod trust;

pub(crate) use apps::{APP_CONTROL_REQUEST_TIMEOUT, APP_LIST_REQUEST_TIMEOUT};
pub(crate) use manager::manage;

use apps::{AppClientSet, AppManagement, AppServiceTransport};

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::ChildStderr;
use tokio::sync::Notify;
use tokio::sync::mpsc::UnboundedReceiver;

use idevice::{
    IdeviceError, IdeviceService, ReadWrite, RsdService,
    core_device::{
        CallInfoBlob, CoreDeviceError, DisplayServiceClient, HevcDepacketizer,
        Orientation as DevOrientation, OrientationServiceClient, PasteboardServiceClient,
        RotationDirection, RtpPacket, build_frame_ack, build_screen_audio_offer,
        build_screen_video_offer, build_start_audio_parameters, build_start_video_parameters,
        hid::{
            ButtonState, DIGITIZER_SURFACE_MAIN_TOUCHSCREEN, IndigoHidClient,
            TOUCHSCREEN_STATE_CONTACT, TOUCHSCREEN_STATE_RELEASE,
        },
        is_rtcp, parse_answer_media_blob,
    },
    diagnostics_relay::DiagnosticsRelayClient,
    lockdown::LockdownClient,
    mobile_image_mounter::ImageMounter,
    mobileactivationd::MobileActivationdClient,
    provider::IdeviceProvider,
    rsd::RsdHandshake,
    tcp::handle::{AdapterHandle, UdpSocketHandle},
};

use crate::audio_output::AudioOutput;
use crate::decode;
use crate::developer_mode;
use crate::hid::{UniversalHidClient, build_multitouch_report};
use crate::location::LocationCommand;
use crate::protocol::{
    AppOperationSlot, ClipboardSlot, ConnKind, DeviceActivationState, DeviceBattery, DeviceDetails,
    DeviceRegionalSettings, DeviceStorage, InputCmd, KeyMods, Orientation, OrientationSlot,
    RotateDir, VideoCounterSnapshot, VideoCounters,
};
use clipboard::ClipboardBridge;
use manager::{SessionVideo, SessionViews};
use media::{
    AccessUnitAssembler, HEVC_QUEUE_MAX_BYTES, HevcQueue, HevcQueuePush, RtpVideoClock,
    RunningStats, audio_decoder_restart_backoff,
};
use orientation::OrientationWatcher;
use rtcp::{RtcpShared, receive_task as rtcp_receive_task, send_task as rtcp_send_task};
use services::{DeviceManagementServices, LocationBridge, SessionServices};
use transport::{SessionEndpoint, connect_core_tunnel, connect_provider};

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

/// Sample transport/source/decode progress without treating a legitimately static
/// screen as a decoder failure. A fully silent transport is retried only after
/// several consecutive samples so normal RTCP sender-report spacing is covered.
const VIDEO_WATCHDOG_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3);
const VIDEO_TRANSPORT_SILENT_WINDOWS: u8 = 3;
/// How long the locked stream must go silent before we migrate to a different
/// SSRC: long enough to ignore stray packets from a competing/leaked sender,
/// short enough to pick up a real stream restart promptly.
const SSRC_TAKEOVER_GRACE: std::time::Duration = std::time::Duration::from_millis(250);
const AUDIO_DECODER_STABLE_RUNTIME: Duration = Duration::from_secs(10);
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

/// Run the whole session to completion. Returns an error string suitable for the
/// status bar if setup fails; otherwise runs until a [`InputCmd::Shutdown`] (or
/// the UI dropping the input channel).
async fn run(
    endpoint: SessionEndpoint,
    pairing_dir: PathBuf,
    video: SessionVideo,
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

    let mut app_clients = AppClientSet::connect_installation_proxy(&*provider).await;
    let (mut adapter, mut handshake) =
        connect_core_tunnel(&endpoint, &*provider, &pairing_dir, &views.status).await?;

    let mut session_services = SessionServices::start(
        provider.clone(),
        connection,
        adapter.clone(),
        handshake.clone(),
        requested_udid,
        &views,
    );

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
            let device_management_services = session_services.take_management();
            management_input_loop(
                DeviceManagement::fallback(
                    provider,
                    views.app_operation.clone(),
                    device_details,
                    app_clients,
                    AppServiceTransport::new(adapter.clone(), handshake.clone()),
                    device_management_services,
                ),
                &mut input_rx,
                session_services.location(),
            )
            .await;
            session_services.shutdown().await;
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
    let orientation_watcher =
        OrientationWatcher::connect(&mut adapter, &mut handshake, &views.orientation).await;

    app_clients
        .connect_app_service(&mut adapter, &mut handshake)
        .await;

    video.browser_frames.reset_dimensions();
    tracing::info!(
        decoder_backend = "webcodecs",
        "selected video decoder backend"
    );

    views.status.set("connected");

    // A stable CNAME for our RTCP SDES (identifies this receiver endpoint).
    let cname = format!("devicehub@{}", adapter.host_ip());

    // Keep the display client to stop the stream on teardown.
    let mut display = media.client;

    // Shared between the RTP receive loop and the RTCP send loop (rtcp-mux feedback
    // goes back out the RTP socket).
    let video_udp = Arc::new(media.video_udp);
    let rtcp_udp = media.rtcp_udp.map(Arc::new);

    // Pulsed when the HEVC publisher loses synchronization or the stream stalls;
    // the RTCP loop reacts with PLI + FIR on the same stream.
    let corruption = Arc::new(Notify::new());

    let rtcp = Arc::new(Mutex::new(RtcpShared::default()));

    // `udp.recv()` holds a non-Send MutexGuard across an await, so these loops
    // can't be spawned; we run them concurrently on this task via `select!`. The
    // input loop is the only one that returns normally (Shutdown / channel close);
    // when it does, the other session-owned futures are dropped as one unit.
    //
    // Complete access units wait in a byte-bounded queue so WebSocket/WebCodecs
    // backpressure cannot stall RTP/RTCP or grow memory without limit.
    let hevc_queue = Arc::new(HevcQueue::new(HEVC_QUEUE_MAX_BYTES));
    let orientation_watch_view = views.orientation.clone();
    let orientation_task = async move {
        match orientation_watcher {
            Some(watcher) => watcher.run(orientation_watch_view).await,
            None => std::future::pending::<()>().await,
        }
    };
    let (clipboard_bridge, clipboard_commands) = ClipboardBridge::channel();
    let decode_corruption = corruption.clone();
    let decode_queue = hevc_queue.clone();
    let decode_counters = video.counters.clone();
    let browser_keyframes = video.browser_frames.clone();
    let browser_lifecycle = video.browser_frames.clone();
    let decode_pipeline = async move {
        browser_video_writer(
            decode_queue,
            video.browser_frames,
            decode_counters,
            decode_corruption,
        )
        .await;
        Ok::<(), String>(())
    };

    let management_app_adapter = adapter.clone();
    let management_app_handshake = handshake.clone();
    let device_management_services = session_services.take_management();
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
        _ = stall_watchdog(video.counters.clone(), &corruption) => {}
        _ = forward_browser_keyframes(browser_keyframes, corruption.clone()) => {}
        _ = rtcp_receive_task(rtcp_udp.clone(), rtcp.clone(), video.counters.clone()) => {}
        _ = rtcp_send_task(
            video_udp, rtcp_udp, rtcp, our_ssrc, cname, &corruption,
        ) => {}
        _ = clipboard::run(
            pasteboard,
            video.clipboard_sync_enabled,
            clipboard,
            clipboard_commands,
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
                app_clients,
                AppServiceTransport::new(management_app_adapter, management_app_handshake),
                device_management_services,
            ),
            &mut input_rx,
            InputBridges {
                orientation: &views.orientation,
                location: session_services.location(),
                clipboard: &clipboard_bridge,
            },
        ) => {}
    }

    session_services.shutdown().await;
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
        WdaAutomationCommand::Unlock { reply, .. } => {
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
        WdaAutomationCommand::BackgroundApp { reply, .. } => {
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
    details: Option<DeviceDetails>,
    apps: AppManagement,
    services: DeviceManagementServices,
}

impl DeviceManagement {
    fn new(
        provider: Arc<dyn IdeviceProvider>,
        app_operation: AppOperationSlot,
        details: Option<DeviceDetails>,
        app_clients: AppClientSet,
        app_service_transport: AppServiceTransport,
        services: DeviceManagementServices,
    ) -> Self {
        let apps = AppManagement::new(
            provider.clone(),
            app_operation,
            app_clients,
            app_service_transport,
        );
        Self {
            provider,
            power: DevicePowerSlot::default(),
            details,
            apps,
            services,
        }
    }

    fn fallback(
        provider: Arc<dyn IdeviceProvider>,
        app_operation: AppOperationSlot,
        details: Option<DeviceDetails>,
        app_clients: AppClientSet,
        app_service_transport: AppServiceTransport,
        services: DeviceManagementServices,
    ) -> Self {
        Self::new(
            provider,
            app_operation,
            details,
            app_clients,
            app_service_transport,
            services,
        )
    }

    async fn handle(&mut self, command: InputCmd) -> Option<InputCmd> {
        let command = self.apps.handle(command).await?;
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
                    command.reject(reason);
                }
                None
            }
            InputCmd::GetWallpaper { kind, reply } => {
                let command = crate::home_screen::HomeScreenCommand::Wallpaper { kind, reply };
                if let Err(error) = self.services.home_screen.try_send(command) {
                    let (reason, command) = match error {
                        tokio::sync::mpsc::error::TrySendError::Full(command) => {
                            ("home screen service is busy", command)
                        }
                        tokio::sync::mpsc::error::TrySendError::Closed(command) => {
                            ("home screen service is unavailable", command)
                        }
                    };
                    command.reject(reason);
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
        | InputCmd::GetWallpaper { .. }
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

/// Receive video RTP, depacketize HEVC, and queue complete Annex-B access units
/// for the WebSocket/WebCodecs publisher. This socket also carries inbound RTCP
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

    // DIAGNOSTIC: if `DEVICEHUB_DUMP_HEVC` is set, tee the published Annex-B
    // bytes to that path for offline inspection.
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
                video_counters.note_transport_activity();
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
                        rtcp.lock().unwrap().reset_media_source(pkt.ssrc);
                    }
                    None => {
                        locked_ssrc = Some(pkt.ssrc);
                        last_locked = now;
                    }
                }
                metrics_rtp_packets += 1;
                metrics_rtp_bytes += dg.data.len() as u64;
                rtcp.lock()
                    .unwrap()
                    .note_rtp_packet(pkt.ssrc, pkt.sequence_number, pkt.marker);
                // The marker bit ends an access unit. Track packet completeness
                // even when experimental frame ACKs are disabled: a complete
                // marker lets us publish the AU without waiting for the
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

async fn browser_video_writer(
    hevc_queue: Arc<HevcQueue>,
    frames: crate::browser_video::BrowserVideoSlot,
    counters: VideoCounters,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VideoWatchdogObservation {
    Decoded,
    SourceWithoutDecode,
    TransportOnly,
    Silent,
}

fn video_watchdog_observation(
    previous: VideoCounterSnapshot,
    current: VideoCounterSnapshot,
) -> VideoWatchdogObservation {
    if current.decoded_frames != previous.decoded_frames {
        VideoWatchdogObservation::Decoded
    } else if current.source_frames != previous.source_frames {
        VideoWatchdogObservation::SourceWithoutDecode
    } else if current.transport_events != previous.transport_events {
        VideoWatchdogObservation::TransportOnly
    } else {
        VideoWatchdogObservation::Silent
    }
}

/// Recover only from evidence of a decoder stall or a genuinely silent transport.
/// RTCP-only activity is healthy for a static screen and must not trigger PLI.
async fn stall_watchdog(counters: VideoCounters, corruption: &Notify) {
    let mut previous = counters.snapshot();
    let mut silent_windows = 0_u8;
    loop {
        tokio::time::sleep(VIDEO_WATCHDOG_INTERVAL).await;
        let current = counters.snapshot();
        match video_watchdog_observation(previous, current) {
            VideoWatchdogObservation::Decoded | VideoWatchdogObservation::TransportOnly => {
                silent_windows = 0;
            }
            VideoWatchdogObservation::SourceWithoutDecode => {
                silent_windows = 0;
                tracing::warn!(
                    interval_ms = VIDEO_WATCHDOG_INTERVAL.as_millis() as u64,
                    "video source advanced without decoded output; requesting keyframe"
                );
                corruption.notify_one();
            }
            VideoWatchdogObservation::Silent => {
                silent_windows = silent_windows.saturating_add(1);
                if silent_windows >= VIDEO_TRANSPORT_SILENT_WINDOWS {
                    tracing::warn!(
                        silent_ms =
                            VIDEO_WATCHDOG_INTERVAL.as_millis() as u64 * u64::from(silent_windows),
                        "video RTP/RTCP transport is silent; requesting keyframe"
                    );
                    corruption.notify_one();
                    silent_windows = 0;
                }
            }
        }
        previous = current;
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
    use super::transport::UsbmuxdEndpoint;
    use super::*;
    use idevice::usbmuxd::{UsbmuxdAddr, UsbmuxdConnection};

    fn video_snapshot(
        transport_events: u64,
        source_frames: u64,
        decoded_frames: u64,
    ) -> VideoCounterSnapshot {
        VideoCounterSnapshot {
            transport_events,
            source_frames,
            decoded_frames,
        }
    }

    #[test]
    fn video_watchdog_distinguishes_static_transport_from_decoder_stalls() {
        let previous = video_snapshot(10, 5, 5);
        assert_eq!(
            video_watchdog_observation(previous, video_snapshot(11, 5, 5)),
            VideoWatchdogObservation::TransportOnly
        );
        assert_eq!(
            video_watchdog_observation(previous, video_snapshot(12, 6, 5)),
            VideoWatchdogObservation::SourceWithoutDecode
        );
        assert_eq!(
            video_watchdog_observation(previous, video_snapshot(12, 6, 6)),
            VideoWatchdogObservation::Decoded
        );
        assert_eq!(
            video_watchdog_observation(previous, previous),
            VideoWatchdogObservation::Silent
        );
    }

    #[tokio::test]
    #[ignore = "requires a connected physical device"]
    async fn reads_developer_mode_status_from_hardware() {
        let mut usbmuxd = UsbmuxdConnection::default().await.expect("connect usbmuxd");
        let devices = usbmuxd.get_devices().await.expect("list devices");
        let endpoint = SessionEndpoint::Usbmuxd(Box::new(UsbmuxdEndpoint {
            device: transport::select_preferred_usbmuxd_device(devices, None)
                .expect("connected device"),
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
}
