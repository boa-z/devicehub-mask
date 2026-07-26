//! Desktop handle for the runtime-owned Apple device lifecycle.
//!
//! `devicehub-runtime` creates the dedicated thread, state graph, control plane,
//! and session manager. This module retains that owner and exposes its shared
//! `RuntimeClient` to the desktop HTTP and MCP composition root.

use std::path::PathBuf;

pub(crate) use devicehub_runtime::{AudioPublisher, PcmAudioConsumer, RuntimePreferences};

/// Desktop host-path bindings for runtime-owned commands and command slots.
pub(crate) type InputCmd = devicehub_runtime::DeviceSessionCommand<PathBuf>;
pub(crate) type InputSink = devicehub_runtime::SessionCommandSlot<PathBuf>;
pub(crate) type ControlCmd = devicehub_runtime::SessionControlCommand;

/// Host-resolved diagnostics applied to each device session.
///
/// Environment variables are parsed once by the desktop composition root. The
/// device thread only receives immutable values, which keeps session lifecycle
/// code independent from the host process environment.
pub(crate) use devicehub_runtime::SessionDiagnostics as RuntimeSessionDiagnostics;

pub(crate) struct RuntimeConfig {
    pub(crate) initial_udid: Option<String>,
    pub(crate) pairing_dir: PathBuf,
    pub(crate) transport: crate::session::DeviceTransportConfig,
    pub(crate) preferences: RuntimePreferences,
    pub(crate) audio: AudioPublisher,
    pub(crate) audio_decoder: crate::decode::AudioDecoderConfig,
    pub(crate) session_diagnostics: RuntimeSessionDiagnostics<PathBuf>,
}

pub(crate) struct DeviceRuntime {
    client: devicehub_runtime::RuntimeClient<PathBuf>,
    owner: devicehub_runtime::CoreRuntime,
}

impl DeviceRuntime {
    pub(crate) fn start(config: RuntimeConfig) -> Result<Self, String> {
        let RuntimeConfig {
            initial_udid,
            pairing_dir,
            transport,
            preferences,
            audio,
            audio_decoder,
            session_diagnostics,
        } = config;
        let started = crate::session::start_manager(
            initial_udid,
            pairing_dir,
            transport,
            preferences,
            audio,
            audio_decoder,
            session_diagnostics,
        )?;
        let (owner, client) = started.into_parts();
        Ok(Self { client, owner })
    }

    pub(crate) fn client(&self) -> devicehub_runtime::RuntimeClient<PathBuf> {
        self.client.clone()
    }

    pub(crate) fn stop(&self) {
        self.owner.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_client_shares_the_owner_control_plane() {
        let (client, mut control_rx) =
            devicehub_runtime::RuntimeClientFixture::<PathBuf>::default().build();

        client.control.send(ControlCmd::Refresh).unwrap();

        assert!(matches!(
            control_rx.blocking_recv(),
            Some(ControlCmd::Refresh)
        ));
    }
}
