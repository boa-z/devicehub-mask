import type { DeviceConditionGroup } from "./types";

export type DeviceConditionSelection = {
  groupIdentifier: string;
  profileIdentifier: string;
};

export function encodeDeviceConditionSelection(selection: DeviceConditionSelection): string {
  return JSON.stringify([selection.groupIdentifier, selection.profileIdentifier]);
}

export function decodeDeviceConditionSelection(value: string): DeviceConditionSelection | null {
  try {
    const parsed: unknown = JSON.parse(value);
    if (!Array.isArray(parsed) || parsed.length !== 2
      || typeof parsed[0] !== "string" || typeof parsed[1] !== "string") return null;
    return { groupIdentifier: parsed[0], profileIdentifier: parsed[1] };
  } catch {
    return null;
  }
}

export function deviceConditionSelectionExists(groups: DeviceConditionGroup[], value: string): boolean {
  const selection = decodeDeviceConditionSelection(value);
  if (!selection) return false;
  return groups.some((group) => group.identifier === selection.groupIdentifier
    && group.profiles.some((profile) => profile.identifier === selection.profileIdentifier));
}
