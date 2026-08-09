import { describe, expect, it } from "vitest";
import type { Device } from "../../types";
import { buildDashboardGroups, relativeUpdateTime } from "./deviceDashboardModel";

function device(overrides: Partial<Device>): Device {
  return {
    id: "phone::usb",
    udid: "phone",
    name: "iPhone",
    connection: "USB",
    pairing: "paired",
    session_status: null,
    session_phase: null,
    session_updated_at_ms: null,
    session_error: null,
    resources: null,
    ...overrides,
  };
}

describe("device dashboard model", () => {
  it("groups transports and preserves the selected transport as primary", () => {
    const groups = buildDashboardGroups([
      device({ id: "phone::wifi", connection: "Wi-Fi", session_phase: "connected", resources: { video: true, audio: false, performance: false, device_logs: false } }),
      device({ id: "phone::usb", session_phase: "connected", session_updated_at_ms: 20, resources: { video: false, audio: true, performance: true, device_logs: false } }),
    ], "phone::usb");

    expect(groups).toHaveLength(1);
    expect(groups[0].primary.id).toBe("phone::usb");
    expect(groups[0].latestUpdateMs).toBe(20);
    expect(groups[0].resources).toEqual({ video: true, audio: true, performance: true, device_logs: false });
  });

  it("prioritizes an active transport when the group is not focused", () => {
    const [group] = buildDashboardGroups([
      device({ id: "phone::usb" }),
      device({ id: "phone::wifi", connection: "Wi-Fi", session_phase: "recovering" }),
    ], null);

    expect(group.primary.id).toBe("phone::wifi");
    expect(group.phase).toBe("recovering");
  });

  it("formats server timestamps relative to the client clock", () => {
    expect(relativeUpdateTime(99_000, 100_000, "en-US")).toBe("1 second ago");
    expect(relativeUpdateTime(null, 100_000, "en-US")).toBeNull();
  });
});
