import { describe, expect, it } from "vitest";
import { appProfileBindingState, filterCrashReports, filterDeviceApps, filterProvisioningProfiles, formatCapacity, formatElapsed, formatFileSize, formatProfileDate, formatReportDate, formatStorageUsage, isEligibleWdaRunner, normalizeDeviceNameInput, shouldRefreshDeviceInspector, sortDeviceApps } from "./deviceInspector";
import type { DeviceApp, DeviceCrashReport, ProvisioningProfile } from "./types";

const apps: DeviceApp[] = [
  {
    bundle_id: "com.example.camera",
    name: "Camera Tool",
    version: "1.0",
    bundle_version: "1",
    is_removable: true,
    is_first_party: false,
    is_developer_app: true,
    documents_available: true,
    static_disk_usage_bytes: 2_000_000,
    dynamic_disk_usage_bytes: 8_000_000,
    total_disk_usage_bytes: 10_000_000,
    is_running: true,
  },
  {
    bundle_id: "com.example.game",
    name: "Sample Game",
    version: null,
    bundle_version: null,
    is_removable: true,
    is_first_party: false,
    is_developer_app: false,
    documents_available: false,
    static_disk_usage_bytes: null,
    dynamic_disk_usage_bytes: null,
    total_disk_usage_bytes: null,
    is_running: null,
  },
];

const profiles: ProvisioningProfile[] = [
  {
    name: "Game Development",
    uuid: "VALID-UUID",
    team_identifiers: ["TEAM123"],
    application_identifier: "TEAM123.com.example.game",
    creation_date: "2026-01-01T00:00:00Z",
    expiration_date: "2027-01-01T00:00:00Z",
    provisioned_devices: 2,
    is_expired: false,
    get_task_allow: true,
    removal_supported: true,
    parse_error: null,
  },
  {
    name: "Old Distribution",
    uuid: "EXPIRED-UUID",
    team_identifiers: ["TEAM999"],
    application_identifier: null,
    creation_date: null,
    expiration_date: "2025-01-01T00:00:00Z",
    provisioned_devices: 0,
    is_expired: true,
    get_task_allow: false,
    removal_supported: true,
    parse_error: null,
  },
  {
    name: "Unreadable profile 3",
    uuid: "invalid-3",
    team_identifiers: [],
    application_identifier: null,
    creation_date: null,
    expiration_date: null,
    provisioned_devices: 0,
    is_expired: false,
    get_task_allow: false,
    removal_supported: false,
    parse_error: "invalid CMS",
  },
];

const crashReports: DeviceCrashReport[] = [
  { path: "/JetsamEvent-2026-07-24.ips", name: "JetsamEvent-2026-07-24.ips", size_bytes: 1_250_000, modified: "2026-07-24T01:02:03Z" },
  { path: "/Retired/Game-2026-07-23.ips", name: "Game-2026-07-23.ips", size_bytes: 999, modified: "2026-07-23T01:02:03Z" },
];

describe("device inspector", () => {
  it("normalizes safe Unicode device names", () => {
    expect(normalizeDeviceNameInput("  Boa 的 iPhone  ")).toBe("Boa 的 iPhone");
    expect(normalizeDeviceNameInput("bad\nname")).toBeNull();
    expect(normalizeDeviceNameInput("界".repeat(64))).toBe("界".repeat(64));
    expect(normalizeDeviceNameInput("😀".repeat(64))).toBeNull();
  });
  it("refreshes only the inspector data affected by a device event", () => {
    expect(shouldRefreshDeviceInspector("app_installed", "apps")).toBe(true);
    expect(shouldRefreshDeviceInspector("app_uninstalled", "info")).toBe(false);
    expect(shouldRefreshDeviceInspector("disk_usage_changed", "info")).toBe(true);
    expect(shouldRefreshDeviceInspector("disk_usage_changed", "apps")).toBe(true);
    expect(shouldRefreshDeviceInspector("activation_state_changed", "info")).toBe(true);
    expect(shouldRefreshDeviceInspector("lock_state_changed", "info")).toBe(false);
    expect(shouldRefreshDeviceInspector("device_name_changed", "apps")).toBe(false);
  });

  it("sorts app storage descending and keeps unavailable usage last", () => {
    const smaller = { ...apps[0], bundle_id: "com.example.small", name: "Small", total_disk_usage_bytes: 5_000_000 };
    expect(sortDeviceApps([apps[1], smaller, apps[0]], "storage", "en-US"))
      .toEqual([apps[0], smaller, apps[1]]);
    expect(sortDeviceApps([apps[1], apps[0]], "name", "en-US"))
      .toEqual([apps[0], apps[1]]);
  });

  it("formats decimal device capacity without exposing invalid values", () => {
    expect(formatCapacity(127_900_000_000)).toBe("128 GB");
    expect(formatCapacity(null)).toBe("-");
    expect(formatCapacity(Number.NaN)).toBe("-");
    expect(formatStorageUsage(120_000_000_000, 45_000_000_000)).toBe("75 GB / 120 GB (63%)");
    expect(formatStorageUsage(100, 101)).toBe("-");
    expect(formatStorageUsage(null, 50)).toBe("-");
  });

  it("filters apps by localized name or bundle identifier", () => {
    expect(filterDeviceApps(apps, " game ")).toEqual([apps[1]]);
    expect(filterDeviceApps(apps, "CAMERA")).toEqual([apps[0]]);
    expect(filterDeviceApps(apps, "")).toBe(apps);
  });

  it("offers WDA startup only for developer xctrunner applications", () => {
    expect(isEligibleWdaRunner({ ...apps[0], bundle_id: "com.example.WDARunner.xctrunner" })).toBe(true);
    expect(isEligibleWdaRunner(apps[0])).toBe(false);
    expect(isEligibleWdaRunner({ ...apps[1], bundle_id: "com.example.WDARunner.xctrunner" })).toBe(false);
  });

  it("filters and formats crash reports", () => {
    expect(filterCrashReports(crashReports, "jetsam")).toEqual([crashReports[0]]);
    expect(filterCrashReports(crashReports, "retired")).toEqual([crashReports[1]]);
    expect(filterCrashReports(crashReports, "")).toBe(crashReports);
    expect(formatFileSize(999)).toBe("999 B");
    expect(formatFileSize(1_250_000)).toBe("1.3 MB");
    expect(formatFileSize(5_250_000_000)).toBe("5.3 GB");
    expect(formatElapsed(3_754_000)).toBe("1h 02m");
    expect(formatReportDate("bad", "en-US")).toBe("-");
    expect(formatReportDate("2026-07-24T01:02:03Z", "en-US")).not.toBe("-");
  });

  it("classifies app profile bindings without hiding conflicts", () => {
    const bindings = { "com.example.camera": "camera", "com.example.game": "game" };
    expect(appProfileBindingState("com.example.camera", "camera", bindings, [])).toBe("active");
    expect(appProfileBindingState("com.example.game", "camera", bindings, [])).toBe("other");
    expect(appProfileBindingState("com.example.notes", "camera", bindings, [])).toBe("unbound");
    expect(appProfileBindingState("com.example.game", "game", bindings, ["com.example.game"])).toBe("conflict");
  });

  it("filters provisioning profiles by metadata and status", () => {
    expect(filterProvisioningProfiles(profiles, "team123", "all")).toEqual([profiles[0]]);
    expect(filterProvisioningProfiles(profiles, "", "valid")).toEqual([profiles[0]]);
    expect(filterProvisioningProfiles(profiles, "", "expired")).toEqual([profiles[1]]);
    expect(filterProvisioningProfiles(profiles, "", "invalid")).toEqual([profiles[2]]);
  });

  it("formats profile dates and rejects malformed values", () => {
    expect(formatProfileDate("2026-07-22T00:00:00Z", "en-US")).toBe("Jul 22, 2026");
    expect(formatProfileDate("not-a-date", "en-US")).toBe("-");
    expect(formatProfileDate(null, "en-US")).toBe("-");
  });
});
