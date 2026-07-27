import type { Device, SessionPhase } from "./types";

export type DeviceGroup = {
  udid: string;
  name: string;
  devices: Device[];
  active: boolean;
};

const phasePriority: Record<SessionPhase, number> = {
  connected: 0,
  recovering: 1,
  connecting: 2,
  disconnecting: 3,
  failed: 4,
  discovered: 5,
  disconnected: 6,
};

function devicePriority(device: Device) {
  return device.session_phase ? phasePriority[device.session_phase] : 7;
}

export function isActiveSession(device: Device) {
  return device.session_phase === "connected"
    || device.session_phase === "recovering"
    || device.session_phase === "connecting"
    || device.session_phase === "disconnecting";
}

export function groupDevices(devices: Device[]): DeviceGroup[] {
  const groups = new Map<string, DeviceGroup>();
  for (const device of devices) {
    const group = groups.get(device.udid) ?? {
      udid: device.udid,
      name: device.name,
      devices: [],
      active: false,
    };
    group.devices.push(device);
    group.active ||= isActiveSession(device);
    groups.set(device.udid, group);
  }
  return [...groups.values()]
    .map((group) => ({
      ...group,
      devices: group.devices.sort((left, right) => devicePriority(left) - devicePriority(right)
        || left.connection.localeCompare(right.connection)),
    }))
    .sort((left, right) => devicePriority(left.devices[0]) - devicePriority(right.devices[0])
      || left.name.localeCompare(right.name));
}

export function connectedPhysicalDeviceCount(devices: Device[]) {
  return new Set(devices.filter(isActiveSession).map((device) => device.udid)).size;
}

export function canConnectTransport(device: Device, devices: Device[]) {
  return device.pairing !== "unpaired"
    && !isActiveSession(device)
    && !devices.some((candidate) => candidate.udid === device.udid && candidate.id !== device.id && isActiveSession(candidate));
}
