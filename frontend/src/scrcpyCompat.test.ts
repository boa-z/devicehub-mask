import { describe, expect, it } from "vitest";
import { exportScrcpyMaskConfig, importScrcpyMaskConfig } from "./scrcpyCompat";

describe("scrcpy-mask compatibility", () => {
  it("imports taps and button direction pads with normalized coordinates", () => {
    const result = importScrcpyMaskConfig({
      version: "0.0.1",
      original_size: { width: 2000, height: 1000 },
      mappings: [
        {
          id: "tap",
          type: "SingleTap",
          pointer_id: 1,
          note: "Jump",
          bind: ["Space"],
          position: { x: 1500, y: 250 },
        },
        {
          id: "move",
          type: "DirectionPad",
          pointer_id: 1,
          note: "Move",
          bind: {
            type: "Button",
            up: ["KeyW"],
            down: ["KeyS"],
            left: ["KeyA"],
            right: ["KeyD"],
          },
          position: { x: 400, y: 700 },
          max_offset_x: 100,
          max_offset_y: 100,
        },
      ],
    }, "game");

    expect(result.imported).toBe(2);
    expect(result.skipped).toBe(0);
    expect(result.profile.mappings[0]).toMatchObject({
      type: "touch", contactId: 1, key: "Space", x: 0.75, y: 0.25,
    });
    expect(result.profile.mappings[1]).toMatchObject({
      type: "dpad", contactId: 0, x: 0.2, y: 0.7,
      keys: { up: "KeyW", down: "KeyS", left: "KeyA", right: "KeyD" },
    });
  });

  it("skips unsupported mappings and chord bindings", () => {
    const result = importScrcpyMaskConfig({
      version: "0.0.1",
      original_size: { width: 100, height: 100 },
      mappings: [
        { id: "chord", type: "SingleTap", pointer_id: 0, bind: ["ControlLeft", "KeyK"], position: { x: 10, y: 10 } },
        { id: "swipe", type: "Swipe", pointer_id: 1, bind: ["KeyQ"], position: { x: 20, y: 20 } },
      ],
    }, "game");

    expect(result.imported).toBe(0);
    expect(result.skipped).toBe(2);
  });

  it("round-trips supported mappings", () => {
    const original = importScrcpyMaskConfig({
      version: "0.0.1",
      original_size: { width: 1920, height: 1080 },
      mappings: [
        { id: "tap", type: "SingleTap", pointer_id: 0, bind: ["SuperLeft"], position: { x: 960, y: 540 } },
      ],
    }, "game").profile;
    const exported = exportScrcpyMaskConfig(original, 1920, 1080);
    const imported = importScrcpyMaskConfig(exported, "roundtrip").profile;

    expect(imported.mappings).toHaveLength(1);
    expect(imported.mappings[0]).toMatchObject({ type: "touch", key: "MetaLeft", x: 0.5, y: 0.5 });
  });
});
