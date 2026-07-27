import { invoke } from "@tauri-apps/api/core";
import { browserHostJson, runningInDesktopHost } from "./hostApi";

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
  return runningInDesktopHost()
    ? invoke<AppSettingsStatus>("app_settings_status")
    : browserHostJson<AppSettingsStatus>("/api/host/settings");
}

export function setAudioEnabled(enabled: boolean) {
  return runningInDesktopHost()
    ? invoke<AppSettingsStatus>("set_audio_enabled", { enabled })
    : updateBrowserSettings({ audio_enabled: enabled });
}

export function setAudioPlayback(muted: boolean, volume: number) {
  return runningInDesktopHost()
    ? invoke<AppSettingsStatus>("set_audio_playback", { muted, volume })
    : updateBrowserSettings({ audio_muted: muted, audio_volume: volume });
}

export function readAudioOutputStatus() {
  if (runningInDesktopHost()) return invoke<AudioOutputStatus>("audio_output_status");
  return Promise.resolve<AudioOutputStatus>({
    state: "unavailable",
    muted: false,
    volume: 0,
    dropped_chunks: 0,
  });
}

export function setClipboardSyncEnabled(enabled: boolean) {
  return runningInDesktopHost()
    ? invoke<AppSettingsStatus>("set_clipboard_sync_enabled", { enabled })
    : updateBrowserSettings({ clipboard_sync_enabled: enabled });
}

function updateBrowserSettings(patch: Record<string, boolean | number>) {
  return browserHostJson<AppSettingsStatus>("/api/host/settings", {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(patch),
  });
}
