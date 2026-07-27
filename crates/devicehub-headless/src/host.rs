use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use devicehub_runtime::RuntimePreferences;
use devicehub_server::http::{
    HostBuildInfo, HostCapabilities, HostControl, HostDiagnosticsStatus, HostSettingsPatch,
    HostSettingsStatus,
};
use serde::{Deserialize, Serialize};

const DEFAULT_AUDIO_VOLUME: f32 = 0.8;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PersistedSettings {
    #[serde(default)]
    audio_enabled: bool,
    #[serde(default)]
    audio_muted: bool,
    #[serde(default = "default_audio_volume")]
    audio_volume: f32,
}

impl Default for PersistedSettings {
    fn default() -> Self {
        Self {
            audio_enabled: false,
            audio_muted: false,
            audio_volume: DEFAULT_AUDIO_VOLUME,
        }
    }
}

fn default_audio_volume() -> f32 {
    DEFAULT_AUDIO_VOLUME
}

struct HeadlessHostInner {
    settings_path: PathBuf,
    settings: RwLock<PersistedSettings>,
    runtime_preferences: RuntimePreferences,
    log_filter: String,
    run_id: String,
}

#[derive(Clone)]
pub struct HeadlessHostControl(Arc<HeadlessHostInner>);

impl HeadlessHostControl {
    pub fn load(settings_path: PathBuf) -> Self {
        let settings = match std::fs::read(&settings_path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                tracing::warn!(path = %settings_path.display(), %error, "ignoring invalid headless settings");
                PersistedSettings::default()
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                PersistedSettings::default()
            }
            Err(error) => {
                tracing::warn!(path = %settings_path.display(), %error, "cannot read headless settings");
                PersistedSettings::default()
            }
        };
        let log_filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into());
        let audio_enabled = settings.audio_enabled;
        Self(Arc::new(HeadlessHostInner {
            settings_path,
            settings: RwLock::new(settings),
            runtime_preferences: RuntimePreferences::new(audio_enabled, false),
            log_filter,
            run_id: uuid::Uuid::new_v4().simple().to_string(),
        }))
    }

    pub fn runtime_preferences(&self) -> RuntimePreferences {
        self.0.runtime_preferences.clone()
    }

    fn save(&self, settings: &PersistedSettings) -> Result<(), String> {
        if let Some(parent) = self.0.settings_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "cannot create settings directory {}: {error}",
                    parent.display()
                )
            })?;
        }
        let bytes = serde_json::to_vec_pretty(settings)
            .map_err(|error| format!("cannot serialize headless settings: {error}"))?;
        std::fs::write(&self.0.settings_path, bytes)
            .map_err(|error| format!("cannot write {}: {error}", self.0.settings_path.display()))
    }
}

impl HostControl for HeadlessHostControl {
    fn settings(&self) -> HostSettingsStatus {
        let settings = self
            .0
            .settings
            .read()
            .expect("headless settings lock poisoned");
        HostSettingsStatus {
            audio_enabled: settings.audio_enabled,
            audio_muted: settings.audio_muted,
            audio_volume: settings.audio_volume,
            clipboard_sync_enabled: false,
        }
    }

    fn update_settings(&self, patch: HostSettingsPatch) -> Result<HostSettingsStatus, String> {
        if patch.clipboard_sync_enabled == Some(true) {
            return Err("host clipboard synchronization is unavailable in headless mode".into());
        }
        let mut settings = self
            .0
            .settings
            .write()
            .map_err(|_| "headless settings lock poisoned".to_owned())?;
        let next = PersistedSettings {
            audio_enabled: patch.audio_enabled.unwrap_or(settings.audio_enabled),
            audio_muted: patch.audio_muted.unwrap_or(settings.audio_muted),
            audio_volume: patch.audio_volume.unwrap_or(settings.audio_volume),
        };
        self.save(&next)?;
        self.0
            .runtime_preferences
            .set_audio_enabled(next.audio_enabled);
        *settings = next;
        drop(settings);
        Ok(self.settings())
    }

    fn diagnostics(&self) -> HostDiagnosticsStatus {
        HostDiagnosticsStatus {
            debug_enabled: self.0.log_filter.contains("debug")
                || self.0.log_filter.contains("trace"),
            custom_filter: true,
            filter: self.0.log_filter.clone(),
            log_directory: String::new(),
            file_logging: false,
            run_id: self.0.run_id.clone(),
            dropped_log_lines: 0,
        }
    }
}

pub fn capabilities() -> HostCapabilities {
    HostCapabilities {
        system_fullscreen: true,
        device_audio: true,
        ..HostCapabilities::default()
    }
}

pub fn build_info() -> HostBuildInfo {
    HostBuildInfo {
        version: env!("CARGO_PKG_VERSION").into(),
        build: env!("DEVICEHUB_BUILD_NUMBER").into(),
        commit: env!("DEVICEHUB_COMMIT").into(),
        update_channel: env!("DEVICEHUB_UPDATE_CHANNEL").into(),
        host: "headless".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_update_runtime_audio_preference() {
        let path = std::env::temp_dir().join(format!(
            "devicehub-headless-settings-{}.json",
            uuid::Uuid::new_v4().simple()
        ));
        let control = HeadlessHostControl::load(path.clone());
        control
            .update_settings(HostSettingsPatch {
                audio_enabled: Some(true),
                ..HostSettingsPatch::default()
            })
            .unwrap();
        assert!(control.runtime_preferences().audio_enabled());
        let _ = std::fs::remove_file(path);
    }
}

#[derive(Clone)]
pub struct BrowserPcmConsumer(pub devicehub_server::websocket::BrowserAudioSlot);

impl devicehub_runtime::PcmAudioConsumer for BrowserPcmConsumer {
    fn publish(&self, pcm: bytes::Bytes) {
        self.0.publish(pcm);
    }
}
