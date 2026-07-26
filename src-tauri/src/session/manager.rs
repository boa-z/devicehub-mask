//! Desktop composition for the runtime-owned session manager.

use std::path::PathBuf;

use tokio::sync::mpsc::UnboundedReceiver;

use super::{clipboard, diagnostics, services};
use crate::device_runtime::{AudioPublisher, ControlCmd};
use devicehub_runtime::{CoreRuntimeState, CoreTunnelConfig, SessionManager};

/// Bind desktop filesystem, process, and clipboard capabilities to the shared
/// runtime manager. Selection, trust, reconnect, and teardown policy stay in
/// `devicehub-runtime`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn manage(
    initial_udid: Option<String>,
    pairing_dir: PathBuf,
    transport: super::DeviceTransportConfig,
    preferences: crate::device_runtime::RuntimePreferences,
    audio: AudioPublisher,
    audio_decoder: crate::decode::AudioDecoderConfig,
    session_diagnostics: crate::device_runtime::RuntimeSessionDiagnostics<PathBuf>,
    state: CoreRuntimeState<PathBuf>,
    control_rx: UnboundedReceiver<ControlCmd>,
) {
    let sidecar = crate::netmuxd::NetmuxdSupervisor::new(pairing_dir.clone(), transport.netmuxd);
    let pairing_store = match crate::wifi_devices::HostPairingStore::new(pairing_dir) {
        Ok(store) => Some(store),
        Err(error) => {
            tracing::warn!(%error, "Wi-Fi pairing storage unavailable; continuing with usbmuxd");
            None
        }
    };
    let tunnel = CoreTunnelConfig::from_host(
        pairing_store
            .clone()
            .unwrap_or(crate::wifi_devices::HostPairingStore::unavailable()),
        transport.system_usbmuxd,
    );
    SessionManager::new(
        sidecar,
        pairing_store,
        tunnel,
        crate::decode::FfmpegAudioPipelineFactory::new(audio, audio_decoder),
        diagnostics::TokioDiagnosticDumpSinks,
        clipboard::ArboardClipboardProvider,
        services::adapters(),
    )
    .run(
        initial_udid,
        preferences,
        session_diagnostics,
        state,
        control_rx,
    )
    .await;
}
