use std::path::PathBuf;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

use crate::protocol::FrameFormat;

#[derive(Debug, Default, Deserialize, Serialize)]
struct PersistedSettings {
    #[serde(default)]
    video_pixel_format: FrameFormat,
    #[serde(default)]
    audio_enabled: bool,
}

#[derive(Debug, Serialize)]
pub struct VideoSettingsStatus {
    pub video_pixel_format: FrameFormat,
    pub environment_override: bool,
    pub audio_enabled: bool,
}

pub struct AppSettings {
    path: PathBuf,
    persisted: RwLock<PersistedSettings>,
    environment_override: Option<FrameFormat>,
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
        let environment_override = configured_environment_format(
            std::env::var_os("DEVICEHUB_VIDEO_PIXEL_FORMAT").as_deref(),
        );
        let settings = Self {
            path,
            persisted: RwLock::new(persisted),
            environment_override,
        };
        let status = settings.status();
        tracing::info!(
            video_pixel_format = ?status.video_pixel_format,
            environment_override = status.environment_override,
            "video settings loaded"
        );
        settings
    }

    pub fn video_pixel_format(&self) -> FrameFormat {
        self.environment_override.unwrap_or_else(|| {
            self.persisted
                .read()
                .expect("application settings lock poisoned")
                .video_pixel_format
        })
    }

    pub fn status(&self) -> VideoSettingsStatus {
        let persisted = self
            .persisted
            .read()
            .expect("application settings lock poisoned");
        VideoSettingsStatus {
            video_pixel_format: self
                .environment_override
                .unwrap_or(persisted.video_pixel_format),
            environment_override: self.environment_override.is_some(),
            audio_enabled: persisted.audio_enabled,
        }
    }

    pub fn audio_enabled(&self) -> bool {
        self.persisted
            .read()
            .expect("application settings lock poisoned")
            .audio_enabled
    }

    pub fn set_video_pixel_format(
        &self,
        video_pixel_format: FrameFormat,
    ) -> Result<VideoSettingsStatus, String> {
        if self.environment_override.is_some() {
            return Err("video pixel format is controlled by DEVICEHUB_VIDEO_PIXEL_FORMAT".into());
        }
        let mut persisted = self
            .persisted
            .write()
            .map_err(|_| "application settings lock poisoned".to_owned())?;
        let next = PersistedSettings {
            video_pixel_format,
            audio_enabled: persisted.audio_enabled,
        };
        self.save_locked(&mut persisted, next)?;
        drop(persisted);
        tracing::info!(
            ?video_pixel_format,
            "video pixel format changed; applies to next session"
        );
        Ok(self.status())
    }

    pub fn set_audio_enabled(&self, audio_enabled: bool) -> Result<VideoSettingsStatus, String> {
        let mut persisted = self
            .persisted
            .write()
            .map_err(|_| "application settings lock poisoned".to_owned())?;
        let next = PersistedSettings {
            video_pixel_format: persisted.video_pixel_format,
            audio_enabled,
        };
        self.save_locked(&mut persisted, next)?;
        drop(persisted);
        tracing::info!(
            audio_enabled,
            "device audio setting changed; applies to next session"
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

fn configured_environment_format(value: Option<&std::ffi::OsStr>) -> Option<FrameFormat> {
    match value.and_then(|value| value.to_str()) {
        None | Some("") => None,
        Some("rgb24") => Some(FrameFormat::Rgb24),
        Some("yuv420p") => Some(FrameFormat::Yuv420p),
        Some(value) => {
            tracing::warn!(value, "ignoring invalid DEVICEHUB_VIDEO_PIXEL_FORMAT");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn accepts_only_known_environment_formats() {
        assert_eq!(configured_environment_format(None), None);
        assert_eq!(
            configured_environment_format(Some(OsStr::new("rgb24"))),
            Some(FrameFormat::Rgb24)
        );
        assert_eq!(
            configured_environment_format(Some(OsStr::new("yuv420p"))),
            Some(FrameFormat::Yuv420p)
        );
        assert_eq!(
            configured_environment_format(Some(OsStr::new("invalid"))),
            None
        );
    }

    #[test]
    fn saves_selected_format() {
        let directory = std::env::temp_dir().join(format!(
            "devicehub-mask-settings-test-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let path = directory.join("settings.json");
        let settings = AppSettings {
            path: path.clone(),
            persisted: RwLock::new(PersistedSettings::default()),
            environment_override: None,
        };

        let status = settings
            .set_video_pixel_format(FrameFormat::Yuv420p)
            .unwrap();
        assert_eq!(status.video_pixel_format, FrameFormat::Yuv420p);
        let saved: PersistedSettings =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved.video_pixel_format, FrameFormat::Yuv420p);
        assert!(!saved.audio_enabled);

        let status = settings.set_audio_enabled(true).unwrap();
        assert!(status.audio_enabled);
        let saved: PersistedSettings =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved.video_pixel_format, FrameFormat::Yuv420p);
        assert!(saved.audio_enabled);

        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn old_settings_default_audio_to_disabled() {
        let saved: PersistedSettings =
            serde_json::from_str(r#"{"video_pixel_format":"rgb24"}"#).unwrap();
        assert!(!saved.audio_enabled);
    }
}
