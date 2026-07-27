//! Complete lifecycle for one selected, connected device session.

use devicehub_core::{AppOperationSlot, ErrorSlot, OrientationSlot, StatusSlot, VideoCounters};
use tokio::sync::mpsc::UnboundedReceiver;

use super::{
    DeviceManagementBootstrap, DiagnosticDumpSinkFactory, OrientationWatcher,
    RuntimeHostServiceViews, RuntimeServiceViews, RuntimeSessionHostAdapters,
    RuntimeSessionServices, SessionDiagnostics, connect_device_input, run_device_command_loop,
    run_management_command_loop,
};
use crate::clipboard::{HostClipboardFactory, connect_device_clipboard};
use crate::media::{
    MediaSessionConfig, MediaSessionRuntime, VideoRtpOptions, start_screen_media_stream,
};
use crate::transport::CoreTunnelConfig;
use crate::{
    BrowserVideoSlot, CaptureFileIo, ClipboardSlot, DeveloperImageAssetLoader, DeviceAudioPipeline,
    DeviceBackupDestination, DeviceSessionCommand, HostFileIo, ProvisioningProfileLoader,
    SessionEndpoint, connect_provider,
};

/// Session state published to host-facing adapters without giving the host
/// ownership of protocol clients or supervised tasks.
#[derive(Clone)]
pub(crate) struct ConnectedSessionViews {
    pub(crate) status: StatusSlot,
    pub(crate) orientation: OrientationSlot,
    pub(crate) error: ErrorSlot,
    pub(crate) app_operation: AppOperationSlot,
    pub(crate) clipboard: ClipboardSlot,
    pub(crate) video_counters: VideoCounters,
    pub(crate) browser_frames: BrowserVideoSlot,
    pub(crate) runtime_services: RuntimeServiceViews,
    pub(crate) host_services: RuntimeHostServiceViews,
}

/// Immutable media policy applied to one connected session.
pub(crate) struct ConnectedSessionMedia<DiagnosticSource> {
    pub(crate) clipboard_sync_enabled: bool,
    pub(crate) diagnostics: SessionDiagnostics<DiagnosticSource>,
}

/// Host capabilities used by a connected session. Protocol and lifecycle
/// ownership remain in the runtime; only platform implementations cross here.
pub(crate) struct ConnectedSessionHost<
    Audio,
    DiagnosticSinks,
    Files,
    CaptureFiles,
    Backup,
    DeveloperImages,
    Profiles,
> {
    pub(crate) audio: Audio,
    pub(crate) diagnostic_sinks: DiagnosticSinks,
    pub(crate) clipboard: Option<HostClipboardFactory>,
    pub(crate) services:
        RuntimeSessionHostAdapters<Files, CaptureFiles, Backup, DeveloperImages, Profiles>,
}

/// Run one selected device session until setup fails or the command channel
/// closes. Management services remain available when screen control is absent.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_connected_session<
    Audio,
    DiagnosticSinks,
    Files,
    CaptureFiles,
    Backup,
    DeveloperImages,
    Profiles,
>(
    endpoint: SessionEndpoint,
    transport: CoreTunnelConfig,
    media: ConnectedSessionMedia<DiagnosticSinks::Source>,
    host: ConnectedSessionHost<
        Audio,
        DiagnosticSinks,
        Files,
        CaptureFiles,
        Backup,
        DeveloperImages,
        Profiles,
    >,
    views: ConnectedSessionViews,
    commands: &mut UnboundedReceiver<DeviceSessionCommand<Files::Path>>,
) -> Result<(), String>
where
    Audio: DeviceAudioPipeline,
    DiagnosticSinks: DiagnosticDumpSinkFactory,
    Files: HostFileIo,
    CaptureFiles: CaptureFileIo<Destination = Files::Path>,
    Backup: DeviceBackupDestination<Destination = Files::Path>,
    DeveloperImages: DeveloperImageAssetLoader<Source = Files::Path>,
    Profiles: ProvisioningProfileLoader<Source = Files::Path>,
{
    views.status.set("connecting to device...");
    let requested_udid = endpoint.udid().to_owned();
    let (provider, connection) = connect_provider(endpoint.clone()).await?;
    let management = DeviceManagementBootstrap::prepare(
        provider.clone(),
        requested_udid.clone(),
        views.app_operation.clone(),
    )
    .await;
    let (mut adapter, mut handshake) = transport
        .connect(&endpoint, provider.as_ref(), &views.status)
        .await?;
    let mut management = management.bind_transport(adapter.clone(), handshake.clone());

    let mut session_services = RuntimeSessionServices::start(
        provider.clone(),
        connection,
        adapter.clone(),
        handshake.clone(),
        views.runtime_services,
    )
    .attach_host_services(
        provider,
        connection,
        adapter.clone(),
        handshake.clone(),
        requested_udid,
        views.host_services,
        host.services,
    );

    // This receiver SSRC must be present in the offer or device RTCP feedback
    // is ignored even though the RTP stream itself remains healthy.
    let our_ssrc = uuid::Uuid::new_v4().as_u128() as u32;
    views.status.set("starting screen media stream...");
    let negotiated = match start_screen_media_stream(
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
            let services = session_services.take_management();
            run_management_command_loop(management.into_router(services), commands).await;
            session_services.shutdown().await;
            views.status.set("stopping...");
            return Ok(());
        }
    };

    views.status.set("connecting HID...");
    let hid_dump_sink = host
        .diagnostic_sinks
        .open(media.diagnostics.hid_dump, 1, "HID diagnostic dump")
        .await;
    let device_input = connect_device_input(
        &mut adapter,
        &mut handshake,
        views.orientation.clone(),
        hid_dump_sink,
    )
    .await?;

    let (clipboard_bridge, clipboard_session) = connect_device_clipboard(
        &mut adapter,
        &mut handshake,
        media.clipboard_sync_enabled,
        host.clipboard,
    )
    .await;
    let orientation_watcher =
        OrientationWatcher::connect(&mut adapter, &mut handshake, &views.orientation).await;
    management
        .connect_app_service(&mut adapter, &mut handshake)
        .await;

    tracing::info!(
        decoder_backend = "webcodecs",
        "selected video decoder backend"
    );
    views.status.set("connected");

    let cname = format!("devicehub@{}", adapter.host_ip());
    let mut display = negotiated.client;
    let orientation_view = views.orientation;
    let orientation_task = async move {
        match orientation_watcher {
            Some(watcher) => watcher.run(orientation_view).await,
            None => std::future::pending::<()>().await,
        }
    };
    let browser_lifecycle = views.browser_frames.clone();
    let services = session_services.take_management();
    let hevc_dump_sink = host
        .diagnostic_sinks
        .open(media.diagnostics.hevc_dump, 8, "HEVC diagnostic dump")
        .await;
    let media_runtime = MediaSessionRuntime::new(
        negotiated.video_udp,
        negotiated.rtcp_udp,
        views.video_counters,
        views.browser_frames,
        MediaSessionConfig {
            our_ssrc,
            cname,
            video: VideoRtpOptions {
                send_frame_ack: media.diagnostics.send_frame_ack,
                annexb_sink: hevc_dump_sink,
            },
            rtcp: media.diagnostics.rtcp,
        },
    );
    media_runtime
        .run(
            host.audio
                .run(crate::DeviceAudioSource::new(negotiated.audio_udp)),
            clipboard_session.run(views.clipboard, &mut adapter, &mut handshake),
            orientation_task,
            run_device_command_loop(
                device_input,
                management.into_router(services),
                commands,
                &clipboard_bridge,
            ),
        )
        .await;

    session_services.shutdown().await;
    browser_lifecycle.reset_dimensions();
    views.status.set("stopping...");
    display.stop_media_stream().await.ok();
    Ok(())
}
