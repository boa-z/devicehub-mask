import { describe, expect, it, vi } from "vitest";
import { clearDeviceInputCollections, directTouchCommand, type DeviceInputCollections } from "./useDeviceInput";

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
    mappedReleaseTimers: new Map([["KeyF", 100]]),
    mappedContactIds: new Map([["jump", 2]]),
    heldPointerBindings: new Map([[7, "MouseRight"]]),
  };
}

describe("device input lifecycle", () => {
  it("routes direct touches through the active input mode", () => {
    const contacts = [{ identity: 1, touching: true, x: 0.25, y: 0.75 }];

    expect(directTouchCommand("mapping", contacts)).toEqual({ type: "keymap_direct_touches", contacts });
    expect(directTouchCommand("keyboard", contacts)).toEqual({ type: "multi_touch", contacts });
  });

  it("clears every locally owned input resource on disconnect", () => {
    const collections = populatedCollections();
    const cancelReleaseTimer = vi.fn();

    clearDeviceInputCollections(collections, cancelReleaseTimer);

    expect(cancelReleaseTimer).toHaveBeenCalledTimes(2);
    expect(cancelReleaseTimer).toHaveBeenCalledWith(99);
    expect(cancelReleaseTimer).toHaveBeenCalledWith(100);
    for (const collection of Object.values(collections)) expect(collection.size).toBe(0);
  });
});
