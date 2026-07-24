import type { DeviceApp, DeviceCrashReport, DeviceEvent, ProvisioningProfile } from "./types";

export type ProfileStatusFilter = "all" | "valid" | "expired" | "invalid";
export type AppProfileBindingState = "unbound" | "active" | "other" | "conflict";
export type DeviceInspectorTab = "info" | "apps" | "files" | "profiles" | "crashes";

export function normalizeDeviceNameInput(name: string): string | null {
  const normalized = name.trim();
  const characters = Array.from(normalized).length;
  if (characters === 0 || characters > 64 || new TextEncoder().encode(normalized).byteLength > 255) return null;
  return Array.from(normalized).some((character) => /\p{Cc}/u.test(character)) ? null : normalized;
}

export function shouldRefreshDeviceInspector(kind: DeviceEvent["kind"], tab: DeviceInspectorTab): boolean {
  if (kind === "app_installed" || kind === "app_uninstalled") return tab === "apps";
  return (kind === "activation_state_changed" || kind === "disk_usage_changed" || kind === "device_name_changed") && tab === "info";
}

export function appProfileBindingState(
  bundleId: string,
  activeProfile: string,
  bindings: Record<string, string>,
  conflicts: readonly string[],
): AppProfileBindingState {
  if (conflicts.includes(bundleId)) return "conflict";
  const owner = bindings[bundleId];
  if (!owner) return "unbound";
  return owner === activeProfile ? "active" : "other";
}

export function formatCapacity(bytes: number | null): string {
  if (bytes === null || !Number.isFinite(bytes) || bytes < 0) return "-";
  return `${Math.round(bytes / 1_000_000_000)} GB`;
}

export function formatStorageUsage(capacity: number | null, available: number | null): string {
  if (capacity === null || available === null
    || !Number.isFinite(capacity) || !Number.isFinite(available)
    || capacity <= 0 || available < 0 || available > capacity) return "-";
  const used = capacity - available;
  return `${formatCapacity(used)} / ${formatCapacity(capacity)} (${Math.round(used * 100 / capacity)}%)`;
}

export function filterDeviceApps(apps: DeviceApp[], query: string): DeviceApp[] {
  const needle = query.trim().toLocaleLowerCase();
  if (!needle) return apps;
  return apps.filter((app) =>
    app.name.toLocaleLowerCase().includes(needle)
    || app.bundle_id.toLocaleLowerCase().includes(needle));
}

export function isEligibleWdaRunner(app: DeviceApp): boolean {
  return app.is_developer_app && app.bundle_id.endsWith(".xctrunner");
}

export function filterCrashReports(reports: DeviceCrashReport[], query: string): DeviceCrashReport[] {
  const needle = query.trim().toLocaleLowerCase();
  if (!needle) return reports;
  return reports.filter((report) =>
    report.name.toLocaleLowerCase().includes(needle)
    || report.path.toLocaleLowerCase().includes(needle));
}

export function formatFileSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "-";
  if (bytes < 1_000) return `${bytes} B`;
  if (bytes < 1_000_000) return `${(bytes / 1_000).toFixed(bytes < 10_000 ? 1 : 0)} KB`;
  if (bytes >= 1_000_000_000) return `${(bytes / 1_000_000_000).toFixed(bytes < 10_000_000_000 ? 1 : 0)} GB`;
  return `${(bytes / 1_000_000).toFixed(bytes < 10_000_000 ? 1 : 0)} MB`;
}

export function formatElapsed(milliseconds: number): string {
  if (!Number.isFinite(milliseconds) || milliseconds < 0) return "-";
  const seconds = Math.floor(milliseconds / 1_000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${String(seconds % 60).padStart(2, "0")}s`;
  return `${Math.floor(minutes / 60)}h ${String(minutes % 60).padStart(2, "0")}m`;
}

export function formatReportDate(value: string, locale: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime())
    ? "-"
    : new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(date);
}

export function filterProvisioningProfiles(
  profiles: ProvisioningProfile[],
  query: string,
  status: ProfileStatusFilter,
): ProvisioningProfile[] {
  const needle = query.trim().toLocaleLowerCase();
  return profiles.filter((profile) => {
    const matchesStatus = status === "all"
      || (status === "invalid" && profile.parse_error !== null)
      || (status === "expired" && profile.parse_error === null && profile.is_expired)
      || (status === "valid" && profile.parse_error === null && !profile.is_expired);
    if (!matchesStatus) return false;
    if (!needle) return true;
    return [profile.name, profile.uuid, profile.application_identifier, ...profile.team_identifiers]
      .some((value) => value?.toLocaleLowerCase().includes(needle));
  });
}

export function formatProfileDate(value: string | null, locale: string): string {
  if (!value) return "-";
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? "-" : new Intl.DateTimeFormat(locale, { dateStyle: "medium" }).format(date);
}
