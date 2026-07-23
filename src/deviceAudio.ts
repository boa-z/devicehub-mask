export type DeviceAudioPreferences = {
  muted: boolean;
  volume: number;
};

export const defaultDeviceAudioPreferences: DeviceAudioPreferences = {
  muted: false,
  volume: 0.8,
};

const legacyStorageKey = "devicehub-mask.device-audio";

export function parseLegacyDeviceAudioPreferences(value: string | null): DeviceAudioPreferences | null {
  if (value === null) return null;
  try {
    const parsed = JSON.parse(value) as Record<string, unknown>;
    return {
      muted: typeof parsed.muted === "boolean" ? parsed.muted : defaultDeviceAudioPreferences.muted,
      volume: typeof parsed.volume === "number" && Number.isFinite(parsed.volume)
        ? Math.min(1, Math.max(0, parsed.volume))
        : defaultDeviceAudioPreferences.volume,
    };
  } catch {
    return null;
  }
}

export function readLegacyDeviceAudioPreferences(): DeviceAudioPreferences | null {
  try {
    return parseLegacyDeviceAudioPreferences(localStorage.getItem(legacyStorageKey));
  } catch {
    return null;
  }
}

export function clearLegacyDeviceAudioPreferences() {
  try {
    localStorage.removeItem(legacyStorageKey);
  } catch {
    // A failed cleanup only causes migration to be retried on the next launch.
  }
}

export type DeviceAudioControlAction = "unavailable" | "enable" | "unmute" | "mute";

export function deviceAudioControlAction(
  enabled: boolean | null,
  muted: boolean,
): DeviceAudioControlAction {
  if (enabled === null) return "unavailable";
  if (!enabled) return "enable";
  return muted ? "unmute" : "mute";
}
