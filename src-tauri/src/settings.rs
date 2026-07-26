use std::path::PathBuf;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

const DEFAULT_AUDIO_VOLUME: f32 = 0.8;

#[derive(Debug, Clone, Deserialize, Serialize)]
struct PersistedSettings {
    #[serde(default)]
    audio_enabled: bool,
    #[serde(default)]
    audio_muted: bool,
    #[serde(default = "default_audio_volume")]
    audio_volume: f32,
    #[serde(default)]
    clipboard_sync_enabled: bool,
}

impl Default for PersistedSettings {
    fn default() -> Self {
        Self {
            audio_enabled: false,
            audio_muted: false,
            audio_volume: DEFAULT_AUDIO_VOLUME,
            clipboard_sync_enabled: false,
        }
    }
}

fn default_audio_volume() -> f32 {
    DEFAULT_AUDIO_VOLUME
}

#[derive(Debug, Serialize)]
pub struct SettingsStatus {
    pub audio_enabled: bool,
    pub audio_muted: bool,
    pub audio_volume: f32,
    pub clipboard_sync_enabled: bool,
}

pub struct AppSettings {
    path: PathBuf,
    persisted: RwLock<PersistedSettings>,
    runtime: crate::device_runtime::RuntimePreferences,
}

impl AppSettings {
    pub fn load(path: PathBuf) -> Self {
        let persisted = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(settings) => settings,
                Err(error) => {
                    tracing::warn!(path = %path.display(), %error, "ignoring invalid application settings");
                    PersistedSettings::default()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                PersistedSettings::default()
            }
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "cannot read application settings");
                PersistedSettings::default()
            }
        };
        let runtime = crate::device_runtime::RuntimePreferences::new(
            persisted.audio_enabled,
            persisted.clipboard_sync_enabled,
        );
        let settings = Self {
            path,
            persisted: RwLock::new(persisted),
            runtime,
        };
        let status = settings.status();
        tracing::info!(
            video_decoder_backend = "webcodecs",
            audio_enabled = status.audio_enabled,
            audio_muted = status.audio_muted,
            audio_volume = status.audio_volume,
            clipboard_sync_enabled = status.clipboard_sync_enabled,
            "application settings loaded"
        );
        settings
    }

    pub fn status(&self) -> SettingsStatus {
        let persisted = self
            .persisted
            .read()
            .expect("application settings lock poisoned");
        SettingsStatus {
            audio_enabled: persisted.audio_enabled,
            audio_muted: persisted.audio_muted,
            audio_volume: persisted.audio_volume,
            clipboard_sync_enabled: persisted.clipboard_sync_enabled,
        }
    }

    pub(crate) fn runtime_preferences(&self) -> crate::device_runtime::RuntimePreferences {
        self.runtime.clone()
    }

    pub fn set_audio_enabled(&self, audio_enabled: bool) -> Result<SettingsStatus, String> {
        let mut persisted = self
            .persisted
            .write()
            .map_err(|_| "application settings lock poisoned".to_owned())?;
        let next = PersistedSettings {
            audio_enabled,
            ..persisted.clone()
        };
        self.save_locked(&mut persisted, next)?;
        drop(persisted);
        self.runtime.set_audio_enabled(audio_enabled);
        tracing::info!(
            audio_enabled,
            "device audio setting changed; applies to next session"
        );
        Ok(self.status())
    }

    pub fn set_audio_playback(
        &self,
        audio_muted: bool,
        audio_volume: f32,
    ) -> Result<SettingsStatus, String> {
        if !audio_volume.is_finite() || !(0.0..=1.0).contains(&audio_volume) {
            return Err("audio volume must be a finite value between 0 and 1".into());
        }
        let mut persisted = self
            .persisted
            .write()
            .map_err(|_| "application settings lock poisoned".to_owned())?;
        let next = PersistedSettings {
            audio_muted,
            audio_volume,
            ..persisted.clone()
        };
        self.save_locked(&mut persisted, next)?;
        drop(persisted);
        tracing::info!(
            audio_muted,
            audio_volume,
            "device audio playback setting changed"
        );
        Ok(self.status())
    }

    pub fn set_clipboard_sync_enabled(
        &self,
        clipboard_sync_enabled: bool,
    ) -> Result<SettingsStatus, String> {
        let mut persisted = self
            .persisted
            .write()
            .map_err(|_| "application settings lock poisoned".to_owned())?;
        let next = PersistedSettings {
            clipboard_sync_enabled,
            ..persisted.clone()
        };
        self.save_locked(&mut persisted, next)?;
        drop(persisted);
        self.runtime
            .set_clipboard_sync_enabled(clipboard_sync_enabled);
        tracing::info!(
            clipboard_sync_enabled,
            "clipboard sync setting changed; applies to next session"
        );
        Ok(self.status())
    }

    fn save_locked(
        &self,
        persisted: &mut PersistedSettings,
        next: PersistedSettings,
    ) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(&next)
            .map_err(|error| format!("cannot serialize application settings: {error}"))?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "cannot create settings directory {}: {error}",
                    parent.display()
                )
            })?;
        }
        std::fs::write(&self.path, json)
            .map_err(|error| format!("cannot write {}: {error}", self.path.display()))?;
        *persisted = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_audio_and_clipboard_preferences() {
        let directory = std::env::temp_dir().join(format!(
            "devicehub-mask-settings-test-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let path = directory.join("settings.json");
        let settings = AppSettings {
            path: path.clone(),
            persisted: RwLock::new(PersistedSettings::default()),
            runtime: crate::device_runtime::RuntimePreferences::new(false, false),
        };

        let status = settings.set_audio_enabled(true).unwrap();
        assert!(status.audio_enabled);
        assert!(settings.runtime_preferences().audio_enabled());
        let saved: PersistedSettings =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(saved.audio_enabled);

        let status = settings.set_audio_playback(true, 0.35).unwrap();
        assert!(status.audio_muted);
        assert_eq!(status.audio_volume, 0.35);
        assert!(settings.set_audio_playback(false, f32::NAN).is_err());
        let saved: PersistedSettings =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(saved.audio_muted);
        assert_eq!(saved.audio_volume, 0.35);

        let status = settings.set_clipboard_sync_enabled(true).unwrap();
        assert!(status.clipboard_sync_enabled);
        assert!(settings.runtime_preferences().clipboard_sync_enabled());
        let saved: PersistedSettings =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(saved.audio_enabled);
        assert!(saved.clipboard_sync_enabled);

        let status = settings.set_audio_enabled(false).unwrap();
        assert!(!status.audio_enabled);
        assert!(!settings.runtime_preferences().audio_enabled());
        assert!(status.clipboard_sync_enabled);

        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn legacy_video_fields_are_ignored() {
        let saved: PersistedSettings = serde_json::from_str(
            r#"{"video_pixel_format":"yuv420p","video_decoder_backend":"native"}"#,
        )
        .unwrap();
        assert!(!saved.audio_enabled);
        assert!(!saved.audio_muted);
        assert_eq!(saved.audio_volume, DEFAULT_AUDIO_VOLUME);
        assert!(!saved.clipboard_sync_enabled);
    }

    #[test]
    fn failed_persistence_does_not_change_runtime_preferences() {
        let directory = std::env::temp_dir().join(format!(
            "devicehub-mask-settings-failure-test-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let blocking_file = directory.join("not-a-directory");
        std::fs::write(&blocking_file, b"block settings parent").unwrap();
        let settings = AppSettings {
            path: blocking_file.join("settings.json"),
            persisted: RwLock::new(PersistedSettings::default()),
            runtime: crate::device_runtime::RuntimePreferences::new(false, false),
        };

        assert!(settings.set_audio_enabled(true).is_err());
        assert!(settings.set_clipboard_sync_enabled(true).is_err());
        let runtime = settings.runtime_preferences();
        assert!(!runtime.audio_enabled());
        assert!(!runtime.clipboard_sync_enabled());

        std::fs::remove_file(blocking_file).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
