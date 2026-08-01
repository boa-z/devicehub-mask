import { describe, expect, it } from "vitest";
import {
  isUiControl,
  mappingBindings,
  pointerButtonCode,
  remainingTapDuration,
  singleTapReleaseDelay,
} from "./control";
import { createMapping, type PadCastSpellMapping, type SingleTapMapping } from "./types";

describe("browser input collection", () => {
  it("reads compound pad bindings without mutating the saved mapping", () => {
    const mapping = {
      ...createMapping("PadCastSpell", { x: 0.5, y: 0.5 }),
      bind: ["Space"],
      pad_bind: { type: "Button", up: ["KeyW"], down: [], left: [], right: [] },
    } as PadCastSpellMapping;

    expect(mappingBindings(mapping)).toEqual(["Space", "KeyW"]);
    expect(mapping.bind).toEqual(["Space"]);
  });

  it("holds short direct taps for at least fifty milliseconds", () => {
    expect(remainingTapDuration(100, 105)).toBe(45);
    expect(remainingTapDuration(100, 150)).toBe(0);
  });

  it("keeps a SingleTap active for its configured duration after a quick key release", () => {
    const mapping = {
      ...createMapping("SingleTap", { x: 0.5, y: 0.5 }),
      bind: ["KeyF"],
      duration: 100,
    } as SingleTapMapping;
    const started = new Map([["KeyF", 1000]]);

    expect(singleTapReleaseDelay([mapping], "KeyF", started, 1010)).toBe(90);
    expect(singleTapReleaseDelay([mapping], "KeyF", started, 1100)).toBe(0);
  });

  it("recognizes nested UI controls before capturing keyboard mappings", () => {
    let selector = "";
    const nestedControl = {
      closest(value: string) {
        selector = value;
        return {};
      },
    } as unknown as EventTarget;

    expect(isUiControl(nestedControl)).toBe(true);
    expect(selector).toContain("input");
    expect(isUiControl(null)).toBe(false);
  });

  it("uses stable mapping codes for the primary mouse buttons", () => {
    expect(pointerButtonCode(0)).toBe("MouseLeft");
    expect(pointerButtonCode(1)).toBe("MouseMiddle");
    expect(pointerButtonCode(2)).toBe("MouseRight");
    expect(pointerButtonCode(3)).toBeUndefined();
  });
});
