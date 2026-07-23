import { describe, expect, it } from "vitest";
import { appProfileBindingState, filterDeviceApps, filterProvisioningProfiles, formatCapacity, formatProfileDate } from "./deviceInspector";
import type { DeviceApp, ProvisioningProfile } from "./types";

const apps: DeviceApp[] = [
  {
    bundle_id: "com.example.camera",
    name: "Camera Tool",
    version: "1.0",
    bundle_version: "1",
    is_removable: true,
    is_first_party: false,
    is_developer_app: true,
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
    parse_error: "invalid CMS",
  },
];

describe("device inspector", () => {
  it("formats decimal device capacity without exposing invalid values", () => {
    expect(formatCapacity(127_900_000_000)).toBe("128 GB");
    expect(formatCapacity(null)).toBe("-");
    expect(formatCapacity(Number.NaN)).toBe("-");
  });

  it("filters apps by localized name or bundle identifier", () => {
    expect(filterDeviceApps(apps, " game ")).toEqual([apps[1]]);
    expect(filterDeviceApps(apps, "CAMERA")).toEqual([apps[0]]);
    expect(filterDeviceApps(apps, "")).toBe(apps);
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
