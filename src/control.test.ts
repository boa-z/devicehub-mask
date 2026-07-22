import { describe, expect, it } from "vitest";
import { buildTouchFrame, mappingBindings, touchFramesEqual } from "./control";
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
});
