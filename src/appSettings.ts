import { invoke } from "@tauri-apps/api/core";

export type AppSettingsStatus = {
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

export function readAppSettings() {
  return invoke<AppSettingsStatus>("app_settings_status");
}

export function setAudioEnabled(enabled: boolean) {
  return invoke<AppSettingsStatus>("set_audio_enabled", { enabled });
}

export function setAudioPlayback(muted: boolean, volume: number) {
  return invoke<AppSettingsStatus>("set_audio_playback", { muted, volume });
}

export function readAudioOutputStatus() {
  return invoke<AudioOutputStatus>("audio_output_status");
}

export function setClipboardSyncEnabled(enabled: boolean) {
  return invoke<AppSettingsStatus>("set_clipboard_sync_enabled", { enabled });
}
