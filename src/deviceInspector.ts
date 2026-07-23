import type { DeviceApp, DeviceCrashReport, ProvisioningProfile } from "./types";

export type ProfileStatusFilter = "all" | "valid" | "expired" | "invalid";
export type AppProfileBindingState = "unbound" | "active" | "other" | "conflict";

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
  return `${(bytes / 1_000_000).toFixed(bytes < 10_000_000 ? 1 : 0)} MB`;
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
