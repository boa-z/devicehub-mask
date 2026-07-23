import type { DeviceLogEntry, DeviceLogLevel } from "./types";

export type DeviceLogLevelFilter = "all" | DeviceLogLevel;

export function filterDeviceLogs(
  entries: DeviceLogEntry[],
  query: string,
  level: DeviceLogLevelFilter,
): DeviceLogEntry[] {
  const needle = query.trim().toLocaleLowerCase();
  return entries.filter((entry) => {
    if (level !== "all" && entry.level !== level) return false;
    if (!needle) return true;
    return [
      entry.message,
      entry.process,
      entry.pid?.toString(),
      entry.subsystem,
      entry.category,
      entry.filename,
    ].some((value) => value?.toLocaleLowerCase().includes(needle));
  });
}

export function deviceLogContext(entry: DeviceLogEntry): string {
  const process = entry.process
    ? `${entry.process}${entry.pid === null ? "" : ` [${entry.pid}]`}`
    : entry.pid === null ? "" : `PID ${entry.pid}`;
  const label = [entry.subsystem, entry.category].filter(Boolean).join(":");
  return [process, label, entry.filename].filter(Boolean).join(" ");
}

export function formatDeviceLogLine(entry: DeviceLogEntry, formattedTime: string): string {
  return [
    formattedTime,
    entry.level?.toUpperCase(),
    deviceLogContext(entry),
    entry.message,
  ].filter(Boolean).join(" ");
}
