export const deviceViewScales = ["fit", "0.25", "0.5", "0.75", "1", "1.25", "1.5", "2"] as const;

export type DeviceViewScale = (typeof deviceViewScales)[number];

export type DeviceViewPreferences = {
  scale: DeviceViewScale;
  controlOverlayVisible: boolean;
  rotationControlsLocked: boolean;
  fullscreenToolbarAutoHide: boolean;
};

export const defaultDeviceViewPreferences: DeviceViewPreferences = {
  scale: "fit",
  controlOverlayVisible: true,
  rotationControlsLocked: false,
  fullscreenToolbarAutoHide: true,
};

const storageKey = "devicehub-mask.device-view";
const scaleSet = new Set<string>(deviceViewScales);

export function parseDeviceViewPreferences(value: string | null): DeviceViewPreferences {
  if (value === null) return { ...defaultDeviceViewPreferences };
  try {
    const parsed: unknown = JSON.parse(value);
    if (parsed === null || typeof parsed !== "object") throw new Error("invalid preference");
    const candidate = parsed as Record<string, unknown>;
    return {
      scale: typeof candidate.scale === "string" && scaleSet.has(candidate.scale)
        ? candidate.scale as DeviceViewScale
        : defaultDeviceViewPreferences.scale,
      controlOverlayVisible: typeof candidate.controlOverlayVisible === "boolean"
        ? candidate.controlOverlayVisible
        : defaultDeviceViewPreferences.controlOverlayVisible,
      rotationControlsLocked: typeof candidate.rotationControlsLocked === "boolean"
        ? candidate.rotationControlsLocked
        : defaultDeviceViewPreferences.rotationControlsLocked,
      fullscreenToolbarAutoHide: typeof candidate.fullscreenToolbarAutoHide === "boolean"
        ? candidate.fullscreenToolbarAutoHide
        : defaultDeviceViewPreferences.fullscreenToolbarAutoHide,
    };
  } catch {
    return { ...defaultDeviceViewPreferences };
  }
}

export function readDeviceViewPreferences(): DeviceViewPreferences {
  try {
    return parseDeviceViewPreferences(localStorage.getItem(storageKey));
  } catch {
    return parseDeviceViewPreferences(null);
  }
}

export function saveDeviceViewPreferences(preferences: DeviceViewPreferences) {
  try {
    localStorage.setItem(storageKey, JSON.stringify(preferences));
  } catch {
    // Preferences remain active for this session when storage is unavailable.
  }
}

export function deviceViewScaleFactor(scale: DeviceViewScale): number | null {
  if (scale === "fit") return null;
  const factor = Number(scale);
  return Number.isFinite(factor) && factor > 0 ? factor : null;
}
