import { invoke } from "@tauri-apps/api/core";
import { browserHostJson, runningInDesktopHost } from "./hostApi";

export type AppSettingsStatus = {
  audio_enabled: boolean;
  audio_muted: boolean;
  audio_volume: number;
  clipboard_sync_enabled: boolean;
  startup_device_priority: string[];
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
  return readAppSettings().then<AudioOutputStatus>((settings) => ({
    state: settings.audio_enabled ? "running" : "idle",
    muted: settings.audio_muted,
    volume: settings.audio_volume,
    dropped_chunks: 0,
  }));
}

export function setClipboardSyncEnabled(enabled: boolean) {
  return runningInDesktopHost()
    ? invoke<AppSettingsStatus>("set_clipboard_sync_enabled", { enabled })
    : updateBrowserSettings({ clipboard_sync_enabled: enabled });
}

export function setStartupDevicePriority(priority: string[]) {
  return runningInDesktopHost()
    ? invoke<AppSettingsStatus>("set_startup_device_priority", { priority })
    : updateBrowserSettings({ startup_device_priority: priority });
}

function updateBrowserSettings(patch: Record<string, boolean | number | string[]>) {
  return browserHostJson<AppSettingsStatus>("/api/host/settings", {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(patch),
  });
}
