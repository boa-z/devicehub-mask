import { describe, expect, it } from "vitest";
import { importScrcpyMaskConfig } from "./scrcpyCompat";
import { keyMappingTypes } from "./types";

const position = { x: 500, y: 250 };
const hooks = { before_script: "", after_script: "" };
const pointer = { bind: ["Space"], pointer_id: 1, position, note: "test" };
const random = { ...pointer, random_offset_x: 0, random_offset_y: 0, script_hooks: hooks };

function fixtures() {
  return [
    { ...random, type: "SingleTap", duration: 50, sync: false },
    { ...random, type: "RepeatTap", duration: 50, interval: 100 },
    { ...random, type: "MultipleTap", items: [{ position, duration: 50, wait: 0 }] },
    { ...pointer, type: "Swipe", duration: 100, enable_randomization: false, positions: [position, { x: 600, y: 250 }], script_hooks: hooks },
    { ...pointer, type: "DirectionPad", bind: { type: "Button", up: ["KeyW"], down: ["KeyS"], left: ["KeyA"], right: ["KeyD"] }, max_offset_x: 100, max_offset_y: 100, script_hooks: hooks },
    { ...random, type: "MouseCastSpell", center: position, cast_radius: 100, drag_radius: 80, release_mode: "OnRelease" },
    { ...random, type: "PadCastSpell", pad_bind: { type: "Button", up: [], down: [], left: [], right: [] }, drag_radius: 80, release_mode: "OnRelease" },
    { type: "CancelCast", id: "cancel", bind: ["Escape"], note: "", position, script_hooks: hooks },
    { ...random, type: "Observation", max_radius: 0, sensitivity_x: 0.8, sensitivity_y: 0.8 },
    { ...pointer, type: "Fps", sensitivity_x: 0.8, sensitivity_y: 0.8, max_offset_x: 0, max_offset_y: 0, touch_mode: { type: "single", interval: 0 } },
    { ...random, type: "Fire", preserve_fps_control: true, sensitivity_x: 0.8, sensitivity_y: 0.8 },
    { type: "RawInput", id: "raw", bind: ["F1"], note: "", position },
    { type: "Script", id: "script", bind: ["F2"], note: "", position, pressed_script: "", held_script: "", released_script: "", interval: 300 },
  ].map((mapping, index) => ({ id: `mapping-${index}`, ...mapping }));
}

describe("scrcpy-mask compatibility", () => {
  it("imports every scrcpy-mask controller type without the five-mapping limit", () => {
    const result = importScrcpyMaskConfig({ version: "0.0.1", original_size: { width: 1000, height: 1000 }, mappings: fixtures() }, "game");
    expect(result.imported).toBe(13);
    expect(result.skipped).toBe(0);
    expect(result.profile.mappings.map((mapping) => mapping.type)).toEqual(keyMappingTypes.slice(1));
    expect(result.profile.mappings[0]).toMatchObject({ type: "SingleTap", position: { x: 0.5, y: 0.25 }, bind: ["Space"] });
  });

  it("preserves nested positions and converts macOS modifier names", () => {
    const config = { version: "0.0.1", original_size: { width: 1000, height: 500 }, mappings: [{ ...random, id: "multi", type: "MultipleTap", bind: ["SuperLeft"], items: [{ position, duration: 50, wait: 0 }] }] };
    const imported = importScrcpyMaskConfig(config, "game").profile;
    expect(imported.mappings[0]).toMatchObject({ bind: ["MetaLeft"], position: { x: 0.5, y: 0.5 }, items: [{ position: { x: 0.5, y: 0.5 } }] });
  });

  it("converts scrcpy mouse, gamepad, and joystick bindings to active DeviceHub names", () => {
    const config = {
      version: "0.0.1",
      original_size: { width: 1000, height: 500 },
      mappings: [
        { ...pointer, id: "mouse", type: "SingleTap", bind: ["M-Left"] },
        { ...pointer, id: "mouse-other", type: "SingleTap", bind: ["M-Other-5"] },
        { ...pointer, id: "gamepad", type: "SingleTap", bind: ["G-South"] },
        {
          ...pointer,
          id: "stick",
          type: "DirectionPad",
          bind: { type: "JoyStick", x: "LeftStickX", y: "LeftStickY" },
          max_offset_x: 100,
          max_offset_y: 100,
        },
        { ...pointer, id: "boost", type: "DirectionPad", bind: { type: "Button", up: ["KeyW"], down: [], left: [], right: [] }, up_boost_key: ["M-Left"], max_offset_x: 100, max_offset_y: 100 },
        { ...pointer, id: "boost-null", type: "DirectionPad", bind: { type: "Button", up: ["KeyW"], down: [], left: [], right: [] }, up_boost_key: null, max_offset_x: 100, max_offset_y: 100 },
      ],
    };

    const imported = importScrcpyMaskConfig(config, "game").profile;
    expect(imported.mappings[0]).toMatchObject({ bind: ["MouseLeft"] });
    expect(imported.mappings[1]).toMatchObject({ bind: ["MouseOther5"] });
    expect(imported.mappings[2]).toMatchObject({ bind: ["GamepadSouth"] });
    expect(imported.mappings[3]).toMatchObject({ bind: { type: "JoyStick", x: "LeftStickX", y: "LeftStickY" } });
    expect(imported.mappings[4]).toMatchObject({ up_boost_key: ["MouseLeft"] });
    expect(imported.mappings[5]).toMatchObject({ up_boost_key: null });
  });

  it("skips malformed mappings instead of clamping or creating an invalid profile", () => {
    const result = importScrcpyMaskConfig({
      version: "0.0.1",
      original_size: { width: 1000, height: 500 },
      mappings: [
        { ...pointer, id: "outside", type: "SingleTap", position: { x: 1001, y: 250 } },
        { ...pointer, id: "path", type: "Swipe", positions: [position] },
        { ...pointer, id: "axis", type: "DirectionPad", bind: { type: "JoyStick", x: "Other-32", y: "LeftStickY" } },
        { ...pointer, id: "fps-default-id", type: "Fps", pointer_id: undefined, touch_mode: { type: "dual", another_pointer_id: 0, strategy: "overlap" } },
        { ...pointer, id: "duplicate", type: "SingleTap" },
        { ...pointer, id: "duplicate", type: "SingleTap" },
      ],
    }, "game");

    expect(result.imported).toBe(1);
    expect(result.skipped).toBe(5);
    expect("position" in result.profile.mappings[0] && result.profile.mappings[0].position).toEqual({ x: 0.5, y: 0.5 });
    expect(() => importScrcpyMaskConfig({ version: "0.0.2", original_size: { width: 1000, height: 500 }, mappings: [] }, "game")).toThrow();
  });
});
