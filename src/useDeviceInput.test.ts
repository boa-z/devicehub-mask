import { describe, expect, it, vi } from "vitest";
import { clearDeviceInputCollections, type DeviceInputCollections } from "./useDeviceInput";

function populatedCollections(): DeviceInputCollections {
  return {
    held: new Set(["KeyW"]),
    heldSince: new Map([["KeyW", 10]]),
    mappingOffsets: new Map([["aim", { x: 0.4, y: 0.6 }]]),
    heldHardware: new Map([["KeyH", "home"]]),
    forwardedKeyboard: new Map([["KeyA", 0x04]]),
    directTouches: new Map([[12, { identity: 3, touching: true, x: 0.2, y: 0.7 }]]),
    directTouchStartedAt: new Map([[12, 20]]),
    directTouchReleaseTimers: new Map([[12, 99]]),
  };
}

describe("device input lifecycle", () => {
  it("clears every locally owned input resource on disconnect", () => {
    const collections = populatedCollections();
    const cancelReleaseTimer = vi.fn();

    clearDeviceInputCollections(collections, cancelReleaseTimer);

    expect(cancelReleaseTimer).toHaveBeenCalledExactlyOnceWith(99);
    for (const collection of Object.values(collections)) expect(collection.size).toBe(0);
  });
});
