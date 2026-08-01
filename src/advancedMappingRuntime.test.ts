import { describe, expect, it } from "vitest";
import { AdvancedMappingRuntime } from "./advancedMappingRuntime";
import type { CancelCastMapping, FireMapping, FpsMapping, ObservationMapping, PadCastSpellMapping } from "./types";

const frame = { width: 1000, height: 500 };
const hooks = { before_script: "", after_script: "" };

function observation(overrides: Partial<ObservationMapping> = {}): ObservationMapping {
  return {
    id: "observation", type: "Observation", note: "", position: { x: 0.5, y: 0.5 }, bind: ["KeyO"],
    pointer_id: 0, random_offset_x: 0, random_offset_y: 0, script_hooks: hooks,
    max_radius: 20, sensitivity_x: 1, sensitivity_y: 1, ...overrides,
  };
}

function fps(overrides: Partial<FpsMapping> = {}): FpsMapping {
  return {
    id: "fps", type: "Fps", note: "", position: { x: 0.5, y: 0.5 }, bind: ["KeyV"], pointer_id: 0,
    sensitivity_x: 1, sensitivity_y: 1, max_offset_x: 10, max_offset_y: 10,
    touch_mode: { type: "single", interval: 20 }, ...overrides,
  };
}

function fire(overrides: Partial<FireMapping> = {}): FireMapping {
  return {
    id: "fire", type: "Fire", note: "", position: { x: 0.8, y: 0.7 }, bind: ["MouseLeft"],
    pointer_id: 1, random_offset_x: 0, random_offset_y: 0, script_hooks: hooks,
    preserve_fps_control: true, sensitivity_x: 1, sensitivity_y: 1, ...overrides,
  };
}

describe("advanced mapping runtime", () => {
  it("holds observation while its chord is held and clamps movement to max radius", () => {
    const runtime = new AdvancedMappingRuntime();
    runtime.configure([observation()], frame);
    const held = new Set(["KeyO"]);
    expect(runtime.keyDown("KeyO", held, 0)).toEqual({ handled: true, capturePointer: true });
    runtime.pointerDelta(100, 0, 1);
    expect(runtime.frame(held, 1).contacts[0]).toMatchObject({ identity: 0, x: 0.52, y: 0.5 });
    runtime.keyUp("KeyO");
    held.clear();
    expect(runtime.frame(held, 2).contacts).toEqual([]);
  });

  it("toggles FPS independently of key release and recenters a single touch", () => {
    const runtime = new AdvancedMappingRuntime();
    runtime.configure([fps()], frame);
    const held = new Set(["KeyV"]);
    runtime.keyDown("KeyV", held, 0);
    runtime.keyUp("KeyV");
    held.clear();
    expect(runtime.frame(held, 1).contacts).toHaveLength(1);
    runtime.pointerDelta(11, 0, 2);
    expect(runtime.frame(held, 2).contacts).toEqual([]);
    expect(runtime.hasTimedWork()).toBe(true);
    expect(runtime.frame(held, 22).contacts[0]).toMatchObject({ identity: 0, x: 0.501, y: 0.5 });
    held.add("KeyV");
    runtime.keyDown("KeyV", held, 23);
    expect(runtime.frame(held, 23).contacts).toEqual([]);
  });

  it("alternates pointer identities during dual-touch FPS recentering", () => {
    const runtime = new AdvancedMappingRuntime();
    runtime.configure([fps({ touch_mode: { type: "dual", another_pointer_id: 2, strategy: "overlap" } })], frame);
    const held = new Set(["KeyV"]);
    runtime.keyDown("KeyV", held, 0);
    runtime.pointerDelta(11, 0, 1);
    const transition = runtime.frame(held, 1).contacts;
    expect(transition.map((contact) => contact.identity).sort()).toEqual([0, 2]);
    expect(runtime.frame(held, 17).contacts.map((contact) => contact.identity)).toEqual([2]);
  });

  it("releases the old FPS touch before a delayed dual-touch handoff", () => {
    const runtime = new AdvancedMappingRuntime();
    runtime.configure([fps({ touch_mode: { type: "dual", another_pointer_id: 2, strategy: "delay", interval: 30 } })], frame);
    const held = new Set(["KeyV"]);
    runtime.keyDown("KeyV", held, 0);
    runtime.pointerDelta(11, 0, 1);
    expect(runtime.frame(held, 1).contacts).toEqual([]);
    expect(runtime.frame(held, 30).contacts).toEqual([]);
    expect(runtime.frame(held, 31).contacts.map((contact) => contact.identity)).toEqual([2]);
  });

  it("preserves FPS while stationary fire is held", () => {
    const runtime = new AdvancedMappingRuntime();
    runtime.configure([fps(), fire()], frame);
    runtime.keyDown("KeyV", new Set(["KeyV"]), 0);
    const held = new Set(["MouseLeft"]);
    runtime.keyDown("MouseLeft", held, 1);
    runtime.pointerDelta(5, 0, 2);
    const contacts = runtime.frame(held, 2).contacts;
    expect(contacts).toHaveLength(2);
    expect(contacts.find((contact) => contact.identity === 0)?.x).toBe(0.505);
    expect(contacts.find((contact) => contact.identity === 1)).toMatchObject({ x: 0.8, y: 0.7 });
  });

  it("hands pointer control to non-preserving fire and restores FPS at center", () => {
    const runtime = new AdvancedMappingRuntime();
    runtime.configure([fps(), fire({ preserve_fps_control: false })], frame);
    runtime.keyDown("KeyV", new Set(["KeyV"]), 0);
    const held = new Set(["MouseLeft"]);
    runtime.keyDown("MouseLeft", held, 1);
    runtime.pointerDelta(10, 5, 2);
    expect(runtime.frame(held, 2).contacts).toEqual([
      expect.objectContaining({ identity: 1, x: 0.81, y: 0.71 }),
    ]);
    runtime.keyUp("MouseLeft");
    held.clear();
    expect(runtime.frame(held, 3).contacts).toEqual([
      expect.objectContaining({ identity: 0, x: 0.5, y: 0.5 }),
    ]);
  });

  it("animates an active cast to the cancel point and releases it", () => {
    const cast: PadCastSpellMapping = {
      id: "cast", type: "PadCastSpell", note: "", position: { x: 0.7, y: 0.7 }, bind: ["KeyQ"],
      pointer_id: 3, random_offset_x: 0, random_offset_y: 0, script_hooks: hooks,
      block_direction_pad: true, drag_radius: 100, enable_randomization: false,
      pad_bind: { type: "Button", up: ["KeyW"], down: ["KeyS"], left: ["KeyA"], right: ["KeyD"] },
      release_mode: "OnRelease",
    };
    const cancel: CancelCastMapping = {
      id: "cancel", type: "CancelCast", note: "", position: { x: 0.2, y: 0.2 }, bind: ["KeyC"], script_hooks: hooks,
    };
    const runtime = new AdvancedMappingRuntime();
    runtime.configure([cast, cancel], frame);
    runtime.keyDown("KeyQ", new Set(["KeyQ"]), 0);
    expect(runtime.frame(new Set(["KeyQ"]), 0).blockDirectionPad).toBe(true);
    runtime.keyDown("KeyC", new Set(["KeyQ", "KeyC"]), 10);
    const halfway = runtime.frame(new Set(["KeyQ", "KeyC"]), 85);
    expect(halfway.blockDirectionPad).toBe(false);
    expect(halfway.contacts[0].identity).toBe(3);
    expect(halfway.contacts[0].x).toBeCloseTo(0.45);
    expect(halfway.contacts[0].y).toBeCloseTo(0.45);
    expect(runtime.frame(new Set(), 160).contacts).toEqual([]);
  });
});
