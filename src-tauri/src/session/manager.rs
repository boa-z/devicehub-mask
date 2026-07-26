//! Desktop composition for the runtime-owned session manager.

use std::path::PathBuf;

use super::{clipboard, diagnostics, services};
use crate::device_runtime::AudioPublisher;
use devicehub_runtime::RuntimeHostAdapters;

/// Start the runtime-owned manager without exposing its concrete desktop
/// adapter types to the rest of the Tauri host.
#[allow(clippy::too_many_arguments)]
pub(crate) fn start(
    initial_udid: Option<String>,
    pairing_dir: PathBuf,
    transport: super::DeviceTransportConfig,
    preferences: crate::device_runtime::RuntimePreferences,
    audio: AudioPublisher,
    audio_decoder: crate::decode::AudioDecoderConfig,
    session_diagnostics: crate::device_runtime::RuntimeSessionDiagnostics<PathBuf>,
) -> Result<devicehub_runtime::StartedRuntime<PathBuf>, String> {
    devicehub_runtime::start_runtime(
        move || {
            let sidecar =
                crate::netmuxd::NetmuxdSupervisor::new(pairing_dir.clone(), transport.netmuxd);
            let pairing_store = match crate::wifi_devices::HostPairingStore::new(pairing_dir) {
                Ok(store) => Some(store),
                Err(error) => {
                    tracing::warn!(%error, "Wi-Fi pairing storage unavailable; continuing with usbmuxd");
                    None
                }
            };
            RuntimeHostAdapters {
                sidecar,
                pairing_store,
                system_usbmuxd: transport.system_usbmuxd,
                audio: crate::decode::FfmpegAudioPipelineFactory::new(audio, audio_decoder),
                diagnostic_sinks: diagnostics::TokioDiagnosticDumpSinks,
                clipboard: clipboard::ArboardClipboardProvider,
                services: services::adapters(),
            }
        },
        initial_udid,
        preferences,
        session_diagnostics,
    )
}
