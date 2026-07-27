import { describe, expect, it } from "vitest";
import type { Device, SessionPhase } from "../types";
import { canConnectTransport, connectedPhysicalDeviceCount, groupDevices, isActiveSession } from "../deviceConnections";

function device(id: string, udid: string, connection: string, phase: SessionPhase | null): Device {
  return {
    id,
    udid,
    name: udid === "phone" ? "iPhone" : "iPad",
    connection,
    pairing: "paired",
    session_status: phase,
    session_phase: phase,
    session_updated_at_ms: phase ? 1 : null,
    session_error: null,
  };
}

describe("device connection center", () => {
  it("groups USB and Wi-Fi transports by physical device", () => {
    const groups = groupDevices([
      device("phone::wifi", "phone", "Wi-Fi", "disconnected"),
      device("tablet::usb", "tablet", "USB", "connected"),
      device("phone::usb", "phone", "USB", "connecting"),
    ]);

    expect(groups.map((group) => group.udid)).toEqual(["tablet", "phone"]);
    expect(groups[1].devices.map((entry) => entry.id)).toEqual(["phone::usb", "phone::wifi"]);
  });

  it("counts active physical devices rather than transport rows", () => {
    const devices = [
      device("phone::usb", "phone", "USB", "connected"),
      device("phone::wifi", "phone", "Wi-Fi", "recovering"),
      device("tablet::usb", "tablet", "USB", "disconnected"),
    ];
    expect(connectedPhysicalDeviceCount(devices)).toBe(1);
    expect(isActiveSession(devices[1])).toBe(true);
    expect(isActiveSession(devices[2])).toBe(false);
  });

  it("requires the active transport to disconnect before switching transports", () => {
    const usb = device("phone::usb", "phone", "USB", "connected");
    const wifi = device("phone::wifi", "phone", "Wi-Fi", "disconnected");
    const tablet = device("tablet::usb", "tablet", "USB", "disconnected");

    expect(canConnectTransport(wifi, [usb, wifi, tablet])).toBe(false);
    expect(canConnectTransport(tablet, [usb, wifi, tablet])).toBe(true);
  });
});
