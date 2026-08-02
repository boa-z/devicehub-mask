import { describe, expect, it, vi } from "vitest";
import {
  gamepadInputNames,
  readGamepadButtonPress,
  readGamepadInput,
  isUiControl,
  mappingBindings,
  pointerButtonCode,
  remainingTapDuration,
  scrollBindingCode,
  singleTapReleaseDelay,
} from "./control";
import { createMapping, mappingBindingLabel, type DirectionPadMapping, type PadCastSpellMapping, type SingleTapMapping } from "./types";

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

  it("releases a synchronized SingleTap immediately with the binding", () => {
    const mapping = {
      ...createMapping("SingleTap", { x: 0.5, y: 0.5 }),
      bind: ["KeyF"],
      duration: 100,
      sync: true,
    } as SingleTapMapping;
    expect(singleTapReleaseDelay([mapping], "KeyF", new Map([["KeyF", 1000]]), 1010)).toBe(0);
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
    expect(pointerButtonCode(3)).toBe("MouseBack");
    expect(pointerButtonCode(4)).toBe("MouseForward");
    expect(pointerButtonCode(5)).toBe("MouseOther5");
  });

  it("normalizes wheel direction into scrcpy binding pulses", () => {
    expect(scrollBindingCode(1)).toBe("ScrollDown");
    expect(scrollBindingCode(-1)).toBe("ScrollUp");
    expect(scrollBindingCode(0)).toBeUndefined();
  });

  it("discovers scrcpy joystick axes and gamepad button bindings", () => {
    const stick = { ...createMapping("DirectionPad", { x: 0.5, y: 0.5 }), bind: { type: "JoyStick", x: "LeftStickX", y: "LeftStickY" } } as DirectionPadMapping;
    const tap = { ...createMapping("SingleTap", { x: 0.5, y: 0.5 }), bind: ["GamepadSouth"] } as SingleTapMapping;
    expect(gamepadInputNames([stick, tap])).toEqual({ buttons: ["GamepadSouth"], axes: ["LeftStickX", "LeftStickY"] });
    expect(mappingBindingLabel(stick)).toBe("LeftStickX/LeftStickY");
  });

  it("collects DirectionPad up-boost bindings as input dependencies", () => {
    const mapping = {
      ...createMapping("DirectionPad", { x: 0.5, y: 0.5 }),
      bind: { type: "Button", up: ["KeyW"], down: [], left: [], right: [] },
      up_boost_key: ["ShiftLeft"],
    } as DirectionPadMapping;
    expect(mappingBindings(mapping)).toEqual(["KeyW", "ShiftLeft"]);
  });

  it("reads standard Gamepad axes and buttons into mapping state", () => {
    const names = { buttons: ["GamepadSouth"], axes: ["LeftStickX", "LeftStickY"] };
    const buttons = Array.from({ length: 17 }, () => ({ pressed: false, value: 0 }));
    buttons[0] = { pressed: true, value: 1 };
    vi.stubGlobal("navigator", { getGamepads: () => [{ axes: [0.75, -0.25], buttons }] });
    expect(readGamepadInput(names)).toEqual({
      keys: ["GamepadSouth"],
      axes: { LeftStickX: 0.75, LeftStickY: -0.25 },
    });
    vi.unstubAllGlobals();
  });

  it("records non-standard Gamepad button indexes with stable names", () => {
    const buttons = Array.from({ length: 20 }, () => ({ pressed: false, value: 0 }));
    buttons[18] = { pressed: true, value: 1 };
    vi.stubGlobal("navigator", { getGamepads: () => [{ axes: [], buttons }] });
    expect(readGamepadButtonPress()).toBe("GamepadOther18");
    vi.unstubAllGlobals();
  });
});
