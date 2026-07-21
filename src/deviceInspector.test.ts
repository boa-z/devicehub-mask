import { describe, expect, it } from "vitest";
import { filterDeviceApps, formatCapacity } from "./deviceInspector";
import type { DeviceApp } from "./types";

const apps: DeviceApp[] = [
  {
    bundle_id: "com.example.camera",
    name: "Camera Tool",
    version: "1.0",
    bundle_version: "1",
    is_removable: true,
    is_first_party: false,
    is_developer_app: true,
  },
  {
    bundle_id: "com.example.game",
    name: "Sample Game",
    version: null,
    bundle_version: null,
    is_removable: true,
    is_first_party: false,
    is_developer_app: false,
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
});
