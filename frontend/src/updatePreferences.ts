export const automaticUpdateStorageKey = "devicehub-mask.updates.automatic";

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
