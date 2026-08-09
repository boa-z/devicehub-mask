import { describe, expect, it, vi } from "vitest";
import { waitForDeviceSession } from "./deviceSelection";
import type { DeviceInventory } from "./types";

function status(activeDeviceId: string | null, sessionStatus: string | null): DeviceInventory {
  return {
    active_device_id: activeDeviceId,
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
      resources: null,
    }],
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
    expect(request).toHaveBeenLastCalledWith("/api/devices");
  });

  it("rejects a target that never receives a registry entry", async () => {
    const request = vi.fn(async () => Response.json(status("tablet::usb", null)));
    await expect(waitForDeviceSession(request, "tablet::usb", 1, 0))
      .rejects.toThrow("device session was not registered");
  });
});
