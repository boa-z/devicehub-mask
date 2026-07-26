// The async device session: connect over the tunnel, bring up the screen media
// stream (which both sources the video AND holds open the HID auth gate), then
// run the video pipeline and dispatch input commands to the device's HID surfaces.

mod clipboard;
mod discovery;
mod manager;
mod services;
mod trust;

pub(crate) use manager::manage;

use std::path::PathBuf;
use std::sync::Arc;

use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc::{self, UnboundedReceiver};

use idevice::{
    RsdService,
    core_device::{OrientationServiceClient, PasteboardServiceClient, hid::IndigoHidClient},
    provider::IdeviceProvider,
};

use crate::decode;
use crate::protocol::{AppOperationSlot, ClipboardSlot, DeviceDetails, InputCmd};
use clipboard::ClipboardBridge;
#[cfg(test)]
use devicehub_runtime::read_device_developer_mode_status;
use devicehub_runtime::{
    AppClientSet, AppServiceTransport, DeviceInputDispatcher, MediaSessionConfig,
    MediaSessionRuntime, OrientationWatcher, RtcpOptions, SessionEndpoint, SystemUsbmuxdConfig,
    UniversalHidClient, VideoRtpOptions, connect_core_tunnel, connect_provider,
    read_device_details, run_device_command_loop, run_management_command_loop,
    start_screen_media_stream,
};
use manager::{SessionVideo, SessionViews};
use services::{DeviceManagementServices, SessionServices};

#[derive(Clone, Debug)]
pub(crate) struct DeviceTransportConfig {
    pub(crate) netmuxd: crate::netmuxd::NetmuxdConfig,
    pub(crate) system_usbmuxd: SystemUsbmuxdConfig,
}

impl DeviceTransportConfig {
    pub(crate) fn from_host(
        netmuxd: crate::netmuxd::NetmuxdConfig,
        system_usbmuxd: Option<String>,
    ) -> Self {
        Self {
            netmuxd,
            system_usbmuxd: SystemUsbmuxdConfig::from_host(system_usbmuxd),
        }
    }
}

/// Run the whole session to completion. Returns an error string suitable for the
/// status bar if setup fails; otherwise runs until a [`InputCmd::Shutdown`] (or
/// the UI dropping the input channel).
async fn run(
    endpoint: SessionEndpoint,
    pairing_dir: PathBuf,
    system_usbmuxd: SystemUsbmuxdConfig,
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
    let (mut adapter, mut handshake) = connect_core_tunnel(
        &endpoint,
        &*provider,
        &pairing_dir,
        &views.status,
        &system_usbmuxd,
    )
    .await?;

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
            run_management_command_loop(
                device_router(
                    provider,
                    views.app_operation.clone(),
                    device_details,
                    app_clients,
                    AppServiceTransport::new(adapter.clone(), handshake.clone()),
                    device_management_services,
                ),
                &mut input_rx,
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
    crate::hid::dump_services_from_env(&mut touch).await;
    let indigo = IndigoHidClient::connect_rsd(&mut adapter, &mut handshake)
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
    let orientation =
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

    let orientation_watch_view = views.orientation.clone();
    let orientation_task = async move {
        match orientation_watcher {
            Some(watcher) => watcher.run(orientation_watch_view).await,
            None => std::future::pending::<()>().await,
        }
    };
    let (clipboard_bridge, clipboard_commands) = ClipboardBridge::channel();
    let browser_lifecycle = video.browser_frames.clone();

    let management_app_adapter = adapter.clone();
    let management_app_handshake = handshake.clone();
    let device_management_services = session_services.take_management();
    let send_frame_ack = std::env::var("DEVICEHUB_FRAME_ACK").is_ok();
    let rtcp_options = RtcpOptions {
        send_rctl: std::env::var("DEVICEHUB_RCTL").is_ok(),
    };
    let hevc_dump_sink = open_hevc_dump_sink().await;
    let media_runtime = MediaSessionRuntime::new(
        media.video_udp,
        media.rtcp_udp,
        video.counters.clone(),
        video.browser_frames,
        MediaSessionConfig {
            our_ssrc,
            cname,
            video: VideoRtpOptions {
                send_frame_ack,
                annexb_sink: hevc_dump_sink,
            },
            rtcp: rtcp_options,
        },
    );
    media_runtime
        .run(
            decode::run_audio_pipeline(
                media.audio_udp,
                video.audio,
                video.audio_decoder,
                video.audio_enabled,
            ),
            clipboard::run(
                pasteboard,
                video.clipboard_sync_enabled,
                clipboard,
                clipboard_commands,
                &mut adapter,
                &mut handshake,
            ),
            orientation_task,
            run_device_command_loop(
                DeviceInputDispatcher::new(touch, indigo, orientation, views.orientation.clone()),
                device_router(
                    provider,
                    views.app_operation.clone(),
                    device_details,
                    app_clients,
                    AppServiceTransport::new(management_app_adapter, management_app_handshake),
                    device_management_services,
                ),
                &mut input_rx,
                &clipboard_bridge,
            ),
        )
        .await;

    session_services.shutdown().await;
    browser_lifecycle.reset_dimensions();
    views.status.set("stopping...");
    display.stop_media_stream().await.ok();
    // `proxy`, `adapter`, `handshake` drop here, tearing down the tunnel.
    Ok(())
}

fn device_router(
    provider: Arc<dyn IdeviceProvider>,
    app_operation: AppOperationSlot,
    details: Option<DeviceDetails>,
    app_clients: AppClientSet,
    app_service_transport: AppServiceTransport,
    services: DeviceManagementServices,
) -> devicehub_runtime::DeviceSessionRouter<PathBuf> {
    devicehub_runtime::DeviceSessionRouter::new(
        provider,
        app_operation,
        details,
        app_clients,
        app_service_transport,
        services,
    )
}

/// Opens the optional host-side HEVC dump without coupling the runtime media
/// pipeline to environment variables or filesystem APIs.
async fn open_hevc_dump_sink() -> Option<mpsc::Sender<Vec<u8>>> {
    let path = std::env::var("DEVICEHUB_DUMP_HEVC").ok()?;
    let mut file = match tokio::fs::File::create(&path).await {
        Ok(file) => file,
        Err(error) => {
            tracing::warn!(%path, %error, "could not open HEVC diagnostic dump");
            return None;
        }
    };
    tracing::info!(%path, "dumping HEVC elementary stream");
    let (sender, mut receiver) = mpsc::channel::<Vec<u8>>(8);
    tokio::spawn(async move {
        while let Some(bytes) = receiver.recv().await {
            if let Err(error) = file.write_all(&bytes).await {
                tracing::warn!(%path, %error, "HEVC diagnostic dump stopped");
                break;
            }
        }
    });
    Some(sender)
}

#[cfg(test)]
mod tests {
    use super::*;
    use devicehub_runtime::{UsbmuxdEndpoint, select_preferred_usbmuxd_device};
    use idevice::usbmuxd::{UsbmuxdAddr, UsbmuxdConnection};

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
        let enabled = read_device_developer_mode_status(provider.as_ref())
            .await
            .expect("query developer mode");
        eprintln!("developer mode enabled: {enabled}");
    }
}
