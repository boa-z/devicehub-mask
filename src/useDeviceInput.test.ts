import { describe, expect, it, vi } from "vitest";
import { acceptsPointerDelta, clearDeviceInputCollections, directTouchCommand, pointerInputMappings, rawInputTriggered, type DeviceInputCollections } from "./useDeviceInput";
import { createMapping, type Profile } from "./types";

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

  it("does not capture the pointer for one-shot MouseCast mappings", () => {
    const onPress = createMapping("MouseCastSpell", { x: 0.5, y: 0.5 });
    const onRelease = createMapping("MouseCastSpell", { x: 0.5, y: 0.5 });
    if (onPress.type !== "MouseCastSpell" || onRelease.type !== "MouseCastSpell") throw new Error("unexpected mapping type");
    onPress.release_mode = "OnPress";
    onRelease.release_mode = "OnRelease";

    expect(acceptsPointerDelta(onPress)).toBe(false);
    expect(acceptsPointerDelta(onRelease)).toBe(true);
  });

  it("initializes every eligible MouseCast mapping sharing a pointer button", () => {
    const observation = createMapping("Observation", { x: 0.5, y: 0.5 });
    const onPress = createMapping("MouseCastSpell", { x: 0.5, y: 0.5 });
    const onRelease = createMapping("MouseCastSpell", { x: 0.5, y: 0.5 });
    if (observation.type !== "Observation" || onPress.type !== "MouseCastSpell" || onRelease.type !== "MouseCastSpell") throw new Error("unexpected mapping type");
    observation.bind = ["MouseLeft"];
    onPress.bind = ["MouseLeft"];
    onPress.release_mode = "OnPress";
    onRelease.bind = ["MouseLeft"];
    const profile = {
      version: 2,
      name: "test",
      mappings: [observation, onPress, onRelease],
      hardwareBindings: { home: "", lock: "", "volume-up": "", "volume-down": "", mute: "", siri: "", action: "" },
      bundleIdentifiers: [],
      targetResolution: null,
    } as Profile;

    expect(pointerInputMappings(profile, "MouseLeft", new Set(["MouseLeft"])).map((mapping) => mapping.id))
      .toEqual([observation.id, onRelease.id]);
  });

  it("requires every RawInput chord key before switching modes", () => {
    const raw = createMapping("RawInput", { x: 0.5, y: 0.5 });
    if (raw.type !== "RawInput") throw new Error("unexpected mapping type");
    raw.bind = ["ShiftLeft", "MouseLeft"];
    const profile = {
      version: 2,
      name: "test",
      mappings: [raw],
      hardwareBindings: { home: "", lock: "", "volume-up": "", "volume-down": "", mute: "", siri: "", action: "" },
      bundleIdentifiers: [],
      targetResolution: null,
    } as Profile;

    expect(rawInputTriggered(profile, new Set(["ShiftLeft"]))).toBe(false);
    expect(rawInputTriggered(profile, new Set(["ShiftLeft", "MouseLeft"]))).toBe(true);
  });
});
