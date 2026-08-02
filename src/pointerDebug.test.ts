import { describe, expect, it } from "vitest";
import { diffPointerDebugContacts, displayToNativePoint, type PointerDebugContact } from "./pointerDebug";

const keymapContact = (x: number, y: number, touching = true): PointerDebugContact => ({
  identity: 2,
  touching,
  x,
  y,
  source: "keymap",
});

describe("pointer debug coordinates", () => {
  it("converts display coordinates to native landscape-right coordinates", () => {
    expect(displayToNativePoint(0.25, 0.5, "landscape_right", { width: 1000, height: 500 })).toEqual({
      displayX: 0.25,
      displayY: 0.5,
      nativeX: 0.5,
      nativeY: 0.75,
      displayPixelX: 250,
      displayPixelY: 250,
      nativePixelX: 250,
      nativePixelY: 749,
    });
  });

  it("clamps invalid normalized coordinates before converting them", () => {
    expect(displayToNativePoint(-1, 2, "portrait", { width: 100, height: 200 })).toMatchObject({
      displayX: 0,
      displayY: 1,
      nativeX: 0,
      nativeY: 1,
      displayPixelX: 0,
      displayPixelY: 199,
    });
  });
});

describe("pointer debug events", () => {
  it("reports down, move, and release when contacts change", () => {
    const previous = new Map<string, PointerDebugContact>();
    expect(diffPointerDebugContacts(previous, [keymapContact(0.1, 0.2)], 10).map((event) => event.action)).toEqual(["down"]);
    previous.set("keymap:2", keymapContact(0.1, 0.2));
    expect(diffPointerDebugContacts(previous, [keymapContact(0.3, 0.4)], 20).map((event) => event.action)).toEqual(["move"]);
    previous.set("keymap:2", keymapContact(0.3, 0.4));
    expect(diffPointerDebugContacts(previous, [], 30).map((event) => event.action)).toEqual(["up"]);
  });
});
