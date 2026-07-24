import { describe, expect, it } from "vitest";
import { buildMappingRuntimeFrame, buildTouchFrame, isUiControl, mappingBindings, mergeTouchContacts, remainingTapDuration, touchFramesEqual } from "./control";
import { createMapping, type PadCastSpellMapping, type RepeatTapMapping, type SingleTapMapping, type SwipeMapping } from "./types";

describe("mapping controller runtime", () => {
  it("pulses repeat taps according to duration and interval", () => {
    const mapping = { ...createMapping("RepeatTap", { x: 0.5, y: 0.5 }), bind: ["Space"], duration: 50, interval: 100 } as RepeatTapMapping;
    const held = new Set(["Space"]);
    const started = new Map([["Space", 1000]]);
    expect(buildTouchFrame([mapping], held, undefined, 1020, started)[0].touching).toBe(true);
    expect(buildTouchFrame([mapping], held, undefined, 1080, started)[0].touching).toBe(false);
    expect(buildTouchFrame([mapping], held, undefined, 1160, started)[0].touching).toBe(true);
  });

  it("interpolates swipe paths over their configured duration", () => {
    const mapping = { ...createMapping("Swipe", { x: 0.2, y: 0.4 }), bind: ["KeyF"], duration: 100, positions: [{ x: 0.2, y: 0.4 }, { x: 0.8, y: 0.4 }] } as SwipeMapping;
    const contact = buildTouchFrame([mapping], new Set(["KeyF"]), undefined, 1050, new Map([["KeyF", 1000]]))[0];
    expect(contact.touching).toBe(true);
    expect(contact.x).toBeCloseTo(0.5);
  });

  it("allows many saved mappings while limiting each HID frame to five contacts", () => {
    const mappings = Array.from({ length: 8 }, (_, identity) => ({ ...createMapping("SingleTap", { x: 0.5, y: 0.5 }), id: String(identity), bind: ["Space"], pointer_id: identity % 5 } as SingleTapMapping));
    expect(buildTouchFrame(mappings, new Set(["Space"]), undefined, 10, new Map([["Space", 0]]))).toHaveLength(5);
  });

  it("highlights only the mapping that owns a reused contact identity", () => {
    const first = { ...createMapping("SingleTap", { x: 0.2, y: 0.3 }), id: "first", bind: ["KeyQ"], pointer_id: 0 } as SingleTapMapping;
    const second = { ...createMapping("SingleTap", { x: 0.7, y: 0.8 }), id: "second", bind: ["KeyE"], pointer_id: 0 } as SingleTapMapping;
    const frame = buildMappingRuntimeFrame([first, second], new Set(["KeyQ", "KeyE"]), undefined, 10, new Map([["KeyQ", 0], ["KeyE", 0]]));

    expect(frame.activeMappingIds).toEqual(new Set(["first"]));
    expect(frame.contacts).toEqual([{ identity: 0, touching: true, x: 0.2, y: 0.3 }]);
  });

  it("assigns a physical key to only one mapping", () => {
    const first = { ...createMapping("SingleTap", { x: 0.2, y: 0.3 }), id: "first", bind: ["KeyQ"], pointer_id: 0 } as SingleTapMapping;
    const second = { ...createMapping("SingleTap", { x: 0.7, y: 0.8 }), id: "second", bind: ["KeyQ"], pointer_id: 1 } as SingleTapMapping;
    const frame = buildMappingRuntimeFrame([first, second], new Set(["KeyQ"]), undefined, 10, new Map([["KeyQ", 0]]));

    expect(frame.activeMappingIds).toEqual(new Set(["first"]));
    expect(frame.contacts).toEqual([
      { identity: 0, touching: true, x: 0.2, y: 0.3 },
      { identity: 1, touching: false, x: 0.7, y: 0.8 },
    ]);
  });

  it("reads compound pad bindings without mutating the saved mapping", () => {
    const mapping = { ...createMapping("PadCastSpell", { x: 0.5, y: 0.5 }), bind: ["Space"], pad_bind: { type: "Button", up: ["KeyW"], down: [], left: [], right: [] } } as PadCastSpellMapping;
    expect(mappingBindings(mapping)).toEqual(["Space", "KeyW"]);
    expect(mapping.bind).toEqual(["Space"]);
  });

  it("detects duplicate HID frames without hiding phase or coordinate changes", () => {
    const frame = [{ identity: 1, touching: true, x: 0.25, y: 0.75 }];
    expect(touchFramesEqual(frame, [{ ...frame[0] }])).toBe(true);
    expect(touchFramesEqual(frame, [{ ...frame[0], touching: false }])).toBe(false);
    expect(touchFramesEqual(frame, [{ ...frame[0], x: 0.26 }])).toBe(false);
    expect(touchFramesEqual(null, frame)).toBe(false);
  });

  it("keeps the explicit release coordinate ahead of an inactive mapping with the same id", () => {
    expect(mergeTouchContacts(
      [{ identity: 0, touching: false, x: 0.1, y: 0.1 }],
      [],
      [{ identity: 0, touching: false, x: 0.8, y: 0.7 }],
    )).toEqual([{ identity: 0, touching: false, x: 0.8, y: 0.7 }]);
  });

  it("holds short direct taps for at least fifty milliseconds", () => {
    expect(remainingTapDuration(100, 105)).toBe(45);
    expect(remainingTapDuration(100, 150)).toBe(0);
    expect(remainingTapDuration(100, 180)).toBe(0);
  });

  it("recognizes nested UI controls before capturing keyboard mappings", () => {
    let selector = "";
    const nestedControl = {
      closest(value: string) {
        selector = value;
        return {};
      },
    } as unknown as EventTarget;
    const deviceSurface = { closest: () => null } as unknown as EventTarget;

    expect(isUiControl(nestedControl)).toBe(true);
    expect(selector).toContain("input");
    expect(selector).toContain("[contenteditable='true']");
    expect(isUiControl(deviceSurface)).toBe(false);
    expect(isUiControl(null)).toBe(false);
  });
});
