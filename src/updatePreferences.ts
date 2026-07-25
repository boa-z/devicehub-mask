import type { UpdateChannel } from "./buildInfo";

export const automaticUpdateStorageKey = "devicehub-mask.updates.automatic";
export const updateChannelStorageKey = "devicehub-mask.updates.channel";

export function parseAutomaticUpdatePreference(value: string | null) {
  return value !== "false";
}

export function readAutomaticUpdatePreference() {
  try {
    return parseAutomaticUpdatePreference(localStorage.getItem(automaticUpdateStorageKey));
  } catch {
    return true;
  }
}

export function writeAutomaticUpdatePreference(enabled: boolean) {
  try {
    localStorage.setItem(automaticUpdateStorageKey, String(enabled));
  } catch {
    // Keep the in-memory preference when WebView storage is unavailable.
  }
}

export function parseUpdateChannelPreference(value: string | null): UpdateChannel | null {
  return value === "stable" || value === "nightly" ? value : null;
}

export function readUpdateChannelPreference() {
  try {
    return parseUpdateChannelPreference(localStorage.getItem(updateChannelStorageKey));
  } catch {
    return null;
  }
}

export function writeUpdateChannelPreference(channel: UpdateChannel) {
  try {
    localStorage.setItem(updateChannelStorageKey, channel);
  } catch {
    // Keep the in-memory preference when WebView storage is unavailable.
  }
}
