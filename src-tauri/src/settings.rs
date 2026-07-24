use std::path::PathBuf;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

use crate::protocol::FrameFormat;

const DEFAULT_AUDIO_VOLUME: f32 = 0.8;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoDecoderBackend {
    Native,
    #[default]
    Browser,
}

#[derive(Debug, Deserialize, Serialize)]
struct PersistedSettings {
    #[serde(default)]
    video_pixel_format: FrameFormat,
    #[serde(default)]
    video_decoder_backend: VideoDecoderBackend,
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
            video_pixel_format: FrameFormat::default(),
            video_decoder_backend: VideoDecoderBackend::default(),
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
pub struct VideoSettingsStatus {
    pub video_pixel_format: FrameFormat,
    pub video_decoder_backend: VideoDecoderBackend,
    pub browser_decoder_fallback: Option<String>,
    pub environment_override: bool,
    pub audio_enabled: bool,
    pub audio_muted: bool,
    pub audio_volume: f32,
    pub clipboard_sync_enabled: bool,
}

pub struct AppSettings {
    path: PathBuf,
    persisted: RwLock<PersistedSettings>,
    environment_override: Option<FrameFormat>,
    browser_decoder_fallback: RwLock<Option<String>>,
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
            browser_decoder_fallback: RwLock::new(None),
        };
        let status = settings.status();
        tracing::info!(
            video_pixel_format = ?status.video_pixel_format,
            video_decoder_backend = ?status.video_decoder_backend,
            environment_override = status.environment_override,
            audio_enabled = status.audio_enabled,
            audio_muted = status.audio_muted,
            audio_volume = status.audio_volume,
            clipboard_sync_enabled = status.clipboard_sync_enabled,
            "application settings loaded"
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

    pub fn video_decoder_backend(&self) -> VideoDecoderBackend {
        let requested = self
            .persisted
            .read()
            .expect("application settings lock poisoned")
            .video_decoder_backend;
        if requested == VideoDecoderBackend::Browser
            && self
                .browser_decoder_fallback
                .read()
                .expect("application settings lock poisoned")
                .is_some()
        {
            VideoDecoderBackend::Native
        } else {
            requested
        }
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
            video_decoder_backend: persisted.video_decoder_backend,
            browser_decoder_fallback: self
                .browser_decoder_fallback
                .read()
                .expect("application settings lock poisoned")
                .clone(),
            environment_override: self.environment_override.is_some(),
            audio_enabled: persisted.audio_enabled,
            audio_muted: persisted.audio_muted,
            audio_volume: persisted.audio_volume,
            clipboard_sync_enabled: persisted.clipboard_sync_enabled,
        }
    }

    pub fn audio_enabled(&self) -> bool {
        self.persisted
            .read()
            .expect("application settings lock poisoned")
            .audio_enabled
    }

    pub fn clipboard_sync_enabled(&self) -> bool {
        self.persisted
            .read()
            .expect("application settings lock poisoned")
            .clipboard_sync_enabled
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
            video_decoder_backend: persisted.video_decoder_backend,
            audio_enabled: persisted.audio_enabled,
            audio_muted: persisted.audio_muted,
            audio_volume: persisted.audio_volume,
            clipboard_sync_enabled: persisted.clipboard_sync_enabled,
        };
        self.save_locked(&mut persisted, next)?;
        drop(persisted);
        tracing::info!(
            ?video_pixel_format,
            "video pixel format changed; applies to next session"
        );
        Ok(self.status())
    }

    pub fn set_video_decoder_backend(
        &self,
        video_decoder_backend: VideoDecoderBackend,
    ) -> Result<VideoSettingsStatus, String> {
        let mut persisted = self
            .persisted
            .write()
            .map_err(|_| "application settings lock poisoned".to_owned())?;
        let next = PersistedSettings {
            video_pixel_format: persisted.video_pixel_format,
            video_decoder_backend,
            audio_enabled: persisted.audio_enabled,
            audio_muted: persisted.audio_muted,
            audio_volume: persisted.audio_volume,
            clipboard_sync_enabled: persisted.clipboard_sync_enabled,
        };
        self.save_locked(&mut persisted, next)?;
        *self
            .browser_decoder_fallback
            .write()
            .map_err(|_| "application settings lock poisoned".to_owned())? = None;
        tracing::info!(
            ?video_decoder_backend,
            "video decoder backend changed; applies to next session"
        );
        Ok(self.status())
    }

    pub fn report_browser_decoder_failure(&self, error: String) -> bool {
        if self
            .persisted
            .read()
            .expect("application settings lock poisoned")
            .video_decoder_backend
            != VideoDecoderBackend::Browser
        {
            return false;
        }
        let mut fallback = self
            .browser_decoder_fallback
            .write()
            .expect("application settings lock poisoned");
        if fallback.is_some() {
            return false;
        }
        tracing::warn!(%error, "browser HEVC decoder unavailable; falling back to native decoder");
        *fallback = Some(error);
        true
    }

    pub fn set_audio_enabled(&self, audio_enabled: bool) -> Result<VideoSettingsStatus, String> {
        let mut persisted = self
            .persisted
            .write()
            .map_err(|_| "application settings lock poisoned".to_owned())?;
        let next = PersistedSettings {
            video_pixel_format: persisted.video_pixel_format,
            video_decoder_backend: persisted.video_decoder_backend,
            audio_enabled,
            audio_muted: persisted.audio_muted,
            audio_volume: persisted.audio_volume,
            clipboard_sync_enabled: persisted.clipboard_sync_enabled,
        };
        self.save_locked(&mut persisted, next)?;
        drop(persisted);
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
    ) -> Result<VideoSettingsStatus, String> {
        if !audio_volume.is_finite() || !(0.0..=1.0).contains(&audio_volume) {
            return Err("audio volume must be a finite value between 0 and 1".into());
        }
        let mut persisted = self
            .persisted
            .write()
            .map_err(|_| "application settings lock poisoned".to_owned())?;
        let next = PersistedSettings {
            video_pixel_format: persisted.video_pixel_format,
            video_decoder_backend: persisted.video_decoder_backend,
            audio_enabled: persisted.audio_enabled,
            audio_muted,
            audio_volume,
            clipboard_sync_enabled: persisted.clipboard_sync_enabled,
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
    ) -> Result<VideoSettingsStatus, String> {
        let mut persisted = self
            .persisted
            .write()
            .map_err(|_| "application settings lock poisoned".to_owned())?;
        let next = PersistedSettings {
            video_pixel_format: persisted.video_pixel_format,
            video_decoder_backend: persisted.video_decoder_backend,
            audio_enabled: persisted.audio_enabled,
            audio_muted: persisted.audio_muted,
            audio_volume: persisted.audio_volume,
            clipboard_sync_enabled,
        };
        self.save_locked(&mut persisted, next)?;
        drop(persisted);
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
            browser_decoder_fallback: RwLock::new(None),
        };

        let status = settings
            .set_video_pixel_format(FrameFormat::Yuv420p)
            .unwrap();
        assert_eq!(status.video_pixel_format, FrameFormat::Yuv420p);
        let saved: PersistedSettings =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved.video_pixel_format, FrameFormat::Yuv420p);
        assert!(!saved.audio_enabled);
        assert!(!saved.clipboard_sync_enabled);

        let status = settings.set_audio_enabled(true).unwrap();
        assert!(status.audio_enabled);
        let saved: PersistedSettings =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved.video_pixel_format, FrameFormat::Yuv420p);
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
        let saved: PersistedSettings =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved.video_pixel_format, FrameFormat::Yuv420p);
        assert!(saved.audio_enabled);
        assert!(saved.clipboard_sync_enabled);

        let status = settings.set_audio_enabled(false).unwrap();
        assert!(!status.audio_enabled);
        assert!(status.clipboard_sync_enabled);

        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn old_settings_default_optional_streams_to_disabled() {
        let saved: PersistedSettings =
            serde_json::from_str(r#"{"video_pixel_format":"rgb24"}"#).unwrap();
        assert_eq!(saved.video_decoder_backend, VideoDecoderBackend::Browser);
        assert!(!saved.audio_enabled);
        assert!(!saved.audio_muted);
        assert_eq!(saved.audio_volume, DEFAULT_AUDIO_VOLUME);
        assert!(!saved.clipboard_sync_enabled);
    }
}
