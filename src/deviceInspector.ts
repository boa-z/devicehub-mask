import type { DeviceApp, ProvisioningProfile } from "./types";

export type ProfileStatusFilter = "all" | "valid" | "expired" | "invalid";

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
