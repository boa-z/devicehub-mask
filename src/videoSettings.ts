import { invoke } from "@tauri-apps/api/core";

export type VideoPixelFormat = "rgb24" | "yuv420p";
export type VideoDecoderBackend = "native" | "browser";

export type VideoSettingsStatus = {
  video_pixel_format: VideoPixelFormat;
  video_decoder_backend: VideoDecoderBackend;
  browser_decoder_fallback: string | null;
  environment_override: boolean;
  audio_enabled: boolean;
  audio_muted: boolean;
  audio_volume: number;
  clipboard_sync_enabled: boolean;
};

export type AudioOutputStatus = {
  state: "idle" | "running" | "unavailable";
  muted: boolean;
  volume: number;
  dropped_chunks: number;
};

export function readVideoSettings() {
  return invoke<VideoSettingsStatus>("video_settings_status");
}

export function setAudioEnabled(enabled: boolean) {
  return invoke<VideoSettingsStatus>("set_audio_enabled", { enabled });
}

export function setAudioPlayback(muted: boolean, volume: number) {
  return invoke<VideoSettingsStatus>("set_audio_playback", { muted, volume });
}

export function readAudioOutputStatus() {
  return invoke<AudioOutputStatus>("audio_output_status");
}

export function setClipboardSyncEnabled(enabled: boolean) {
  return invoke<VideoSettingsStatus>("set_clipboard_sync_enabled", { enabled });
}

export function setVideoPixelFormat(videoPixelFormat: VideoPixelFormat) {
  return invoke<VideoSettingsStatus>("set_video_pixel_format", {
    videoPixelFormat,
  });
}

export function setVideoDecoderBackend(videoDecoderBackend: VideoDecoderBackend) {
  return invoke<VideoSettingsStatus>("set_video_decoder_backend", {
    videoDecoderBackend,
  });
}
