import { groupDevices, isActiveSession, type DeviceGroup } from "../device-session/deviceConnections";
import type { Device, SessionPhase, SessionResources } from "../../types";

export type DashboardDeviceGroup = DeviceGroup & {
  primary: Device;
  phase: SessionPhase;
  latestUpdateMs: number | null;
  resources: SessionResources;
};

const emptyResources: SessionResources = {
  video: false,
  audio: false,
  performance: false,
  device_logs: false,
};

export function buildDashboardGroups(
  devices: Device[],
  selectedDeviceId: string | null,
  startupPriority: readonly string[] = [],
): DashboardDeviceGroup[] {
  return groupDevices(devices, startupPriority).map((group) => {
    const primary = group.devices.find((device) => device.id === selectedDeviceId)
      ?? group.devices.find(isActiveSession)
      ?? group.devices[0];
    return {
      ...group,
      primary,
      phase: primary.session_phase ?? "discovered",
      latestUpdateMs: group.devices.reduce<number | null>((latest, device) => {
        if (device.session_updated_at_ms === null) return latest;
        return latest === null ? device.session_updated_at_ms : Math.max(latest, device.session_updated_at_ms);
      }, null),
      resources: group.devices.reduce<SessionResources>((resources, device) => ({
        video: resources.video || device.resources?.video === true,
        audio: resources.audio || device.resources?.audio === true,
        performance: resources.performance || device.resources?.performance === true,
        device_logs: resources.device_logs || device.resources?.device_logs === true,
      }), { ...emptyResources }),
    };
  });
}

export function relativeUpdateTime(updatedAtMs: number | null, nowMs: number, locale: string) {
  if (updatedAtMs === null) return null;
  const elapsedSeconds = Math.max(0, Math.round((nowMs - updatedAtMs) / 1_000));
  const formatter = new Intl.RelativeTimeFormat(locale, { numeric: "auto" });
  if (elapsedSeconds < 60) return formatter.format(-elapsedSeconds, "second");
  const elapsedMinutes = Math.round(elapsedSeconds / 60);
  if (elapsedMinutes < 60) return formatter.format(-elapsedMinutes, "minute");
  const elapsedHours = Math.round(elapsedMinutes / 60);
  if (elapsedHours < 24) return formatter.format(-elapsedHours, "hour");
  return formatter.format(-Math.round(elapsedHours / 24), "day");
}
