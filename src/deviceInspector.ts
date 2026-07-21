import type { DeviceApp } from "./types";

export function formatCapacity(bytes: number | null): string {
  if (bytes === null || !Number.isFinite(bytes) || bytes < 0) return "-";
  return `${Math.round(bytes / 1_000_000_000)} GB`;
}

export function filterDeviceApps(apps: DeviceApp[], query: string): DeviceApp[] {
  const needle = query.trim().toLocaleLowerCase();
  if (!needle) return apps;
  return apps.filter((app) =>
    app.name.toLocaleLowerCase().includes(needle)
    || app.bundle_id.toLocaleLowerCase().includes(needle));
}
