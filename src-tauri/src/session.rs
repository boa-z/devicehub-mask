// The async device session: connect over the tunnel, bring up the screen media
// stream (which both sources the video AND holds open the HID auth gate), then
// run the video pipeline and dispatch input commands to the device's HID surfaces.

mod clipboard;
mod diagnostics;
mod discovery;
mod manager;
mod services;
mod trust;

pub(crate) use manager::manage;

use std::path::PathBuf;

use tokio::sync::mpsc::UnboundedReceiver;

use crate::protocol::{ClipboardSlot, InputCmd};
#[cfg(test)]
use devicehub_runtime::read_device_developer_mode_status;
use devicehub_runtime::{
    DeviceAudioPipeline, DeviceManagementBootstrap, DiagnosticDumpSinkFactory, MediaSessionConfig,
    MediaSessionRuntime, OrientationWatcher, SessionEndpoint, SystemUsbmuxdConfig, VideoRtpOptions,
    connect_core_tunnel, connect_device_clipboard, connect_device_input, connect_provider,
    run_device_command_loop, run_management_command_loop, start_screen_media_stream,
};
use manager::{SessionVideo, SessionViews};

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
    let management = DeviceManagementBootstrap::prepare(
        provider.clone(),
        requested_udid.clone(),
        views.app_operation.clone(),
    )
    .await;
    let (mut adapter, mut handshake) = connect_core_tunnel(
        &endpoint,
        &*provider,
        &pairing_dir,
        &views.status,
        &system_usbmuxd,
    )
    .await?;
    let mut management = management.bind_transport(adapter.clone(), handshake.clone());

    let mut session_services = services::start(
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
        management.details(),
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
                management.into_router(device_management_services),
                &mut input_rx,
            )
            .await;
            session_services.shutdown().await;
            views.status.set("stopping...");
            return Ok(());
        }
    };

    views.status.set("connecting HID...");
    let diagnostic_sinks = diagnostics::TokioDiagnosticDumpSinks;
    let hid_dump_sink = diagnostic_sinks
        .open(video.diagnostics.hid_dump.clone(), 1, "HID diagnostic dump")
        .await;
    let device_input = connect_device_input(
        &mut adapter,
        &mut handshake,
        views.orientation.clone(),
        hid_dump_sink,
    )
    .await?;

    let host_clipboard = video
        .clipboard_sync_enabled
        .then(|| Box::new(clipboard::connect_host) as devicehub_runtime::HostClipboardFactory);
    let (clipboard_bridge, clipboard_session) = connect_device_clipboard(
        &mut adapter,
        &mut handshake,
        video.clipboard_sync_enabled,
        host_clipboard,
    )
    .await;

    // The media stream always exposes a native portrait framebuffer, including
    // when a landscape-only game has rotated its content inside that frame.
    // SpringBoard provides the current interface orientation without changing it.
    let orientation_watcher =
        OrientationWatcher::connect(&mut adapter, &mut handshake, &views.orientation).await;

    management
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
    let browser_lifecycle = video.browser_frames.clone();

    let device_management_services = session_services.take_management();
    let hevc_dump_sink = diagnostic_sinks
        .open(
            video.diagnostics.hevc_dump.clone(),
            8,
            "HEVC diagnostic dump",
        )
        .await;
    let media_runtime = MediaSessionRuntime::new(
        media.video_udp,
        media.rtcp_udp,
        video.counters.clone(),
        video.browser_frames,
        MediaSessionConfig {
            our_ssrc,
            cname,
            video: VideoRtpOptions {
                send_frame_ack: video.diagnostics.send_frame_ack,
                annexb_sink: hevc_dump_sink,
            },
            rtcp: video.diagnostics.rtcp,
        },
    );
    media_runtime
        .run(
            video.audio.run(media.audio_udp),
            clipboard_session.run(clipboard, &mut adapter, &mut handshake),
            orientation_task,
            run_device_command_loop(
                device_input,
                management.into_router(device_management_services),
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
