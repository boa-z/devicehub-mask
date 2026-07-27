import { describe, expect, it, vi } from "vitest";
import { waitForDeviceSession } from "./deviceSelection";
import type { DeviceStatus } from "./types";

function status(activeDeviceId: string | null, sessionStatus: string | null): DeviceStatus {
  return {
    status: sessionStatus ?? "disconnected",
    phase: sessionStatus === "connecting" ? "connecting" : "disconnected",
    updated_at_ms: 0,
    active_udid: activeDeviceId ? "device" : null,
    active_device_id: activeDeviceId,
    error: null,
    orientation: "portrait",
    devices: [{
      id: "tablet::usb",
      udid: "device",
      name: "iPad",
      connection: "USB",
      pairing: "paired",
      session_status: sessionStatus,
      session_phase: sessionStatus === "connecting" ? "connecting" : null,
      session_updated_at_ms: sessionStatus === null ? null : 0,
      session_error: null,
    }],
    location: { available: false, active: false, backend: null, latitude: null, longitude: null, error: null },
  };
}

describe("device session selection", () => {
  it("waits for both manager selection and a registry-backed status", async () => {
    const request = vi.fn()
      .mockResolvedValueOnce(Response.json(status(null, null)))
      .mockResolvedValueOnce(Response.json(status("tablet::usb", "connecting")));

    await expect(waitForDeviceSession(request, "tablet::usb", 100, 0))
      .resolves.toMatchObject({ active_device_id: "tablet::usb" });
    expect(request).toHaveBeenCalledTimes(2);
  });

  it("rejects a target that never receives a registry entry", async () => {
    const request = vi.fn(async () => Response.json(status("tablet::usb", null)));
    await expect(waitForDeviceSession(request, "tablet::usb", 1, 0))
      .rejects.toThrow("device session was not registered");
  });
});
