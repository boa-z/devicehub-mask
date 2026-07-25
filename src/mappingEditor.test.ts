import { describe, expect, it, vi } from "vitest";
import { convertEditorMappingType, createEditorMapping, duplicateEditorMapping } from "./mappingEditor";
import { createMapping, type Mapping } from "./types";

vi.stubGlobal("crypto", { randomUUID: () => "new-id" });

describe("mapping editor", () => {
  it("assigns the least-used iOS contact to new controls", () => {
    const existing = [0, 1, 2].map((pointer_id) => ({
      ...createMapping("SingleTap", { x: 0.5, y: 0.5 }),
      id: `mapping-${pointer_id}`,
      pointer_id,
    })) as Mapping[];
    expect(createEditorMapping("RepeatTap", { x: 0.2, y: 0.3 }, { width: 1000, height: 500 }, existing))
      .toMatchObject({ pointer_id: 3, position: { x: 0.2, y: 0.3 } });
  });

  it("duplicates a control with a new id, position, and contact", () => {
    const source = { ...createMapping("SingleTap", { x: 0.5, y: 0.5 }), id: "source", pointer_id: 0 } as Mapping;
    expect(duplicateEditorMapping(source, [source])).toMatchObject({
      id: "new-id",
      pointer_id: 1,
      position: { x: 0.525, y: 0.525 },
    });
  });

  it("keeps dual FPS contacts distinct", () => {
    const source = createMapping("Fps", { x: 0.5, y: 0.5 });
    if (source.type !== "Fps") throw new Error("unexpected mapping type");
    source.touch_mode = { type: "dual", another_pointer_id: 1, strategy: "overlap" };
    const duplicate = duplicateEditorMapping(source, [source]);
    if (duplicate.type !== "Fps" || duplicate.touch_mode.type !== "dual") throw new Error("unexpected mapping type");
    expect(duplicate.pointer_id).not.toBe(duplicate.touch_mode.another_pointer_id);
  });

  it("rebuilds a changed type while preserving compatible button fields", () => {
    const source = createMapping("SingleTap", { x: 0.25, y: 0.75 });
    if (source.type !== "SingleTap") throw new Error("unexpected mapping type");
    source.id = "skill";
    source.note = "Skill";
    source.bind = ["ShiftLeft", "KeyQ"];
    source.pointer_id = 3;
    source.duration = 80;

    expect(convertEditorMappingType(source, "RepeatTap", { width: 1000, height: 500 })).toEqual(expect.objectContaining({
      id: "skill",
      type: "RepeatTap",
      note: "Skill",
      position: { x: 0.25, y: 0.75 },
      bind: ["ShiftLeft", "KeyQ"],
      pointer_id: 3,
      duration: 80,
      interval: 100,
    }));
  });

  it("converts legacy controls into validated scrcpy controller shapes", () => {
    const legacyTap: Mapping = { id: "tap", type: "touch", label: "Jump", contactId: 2, x: 0.8, y: 0.7, key: "Space" };
    expect(convertEditorMappingType(legacyTap, "SingleTap", { width: 1000, height: 500 })).toMatchObject({
      id: "tap",
      type: "SingleTap",
      note: "Jump",
      position: { x: 0.8, y: 0.7 },
      bind: ["Space"],
      pointer_id: 2,
    });

    const legacyPad: Mapping = { id: "move", type: "dpad", label: "Move", contactId: 1, x: 0.2, y: 0.8, radius: 0.1, keys: { up: "KeyW", down: "KeyS", left: "KeyA", right: "KeyD" } };
    expect(convertEditorMappingType(legacyPad, "DirectionPad", { width: 1000, height: 500 })).toMatchObject({
      id: "move",
      type: "DirectionPad",
      note: "Move",
      pointer_id: 1,
      bind: { type: "Button", up: ["KeyW"], down: ["KeyS"], left: ["KeyA"], right: ["KeyD"] },
    });
  });

  it("drops incompatible source-only fields when changing controller families", () => {
    const source = createMapping("Swipe", { x: 0.4, y: 0.5 });
    if (source.type !== "Swipe") throw new Error("unexpected mapping type");
    source.positions = [{ x: 0.4, y: 0.5 }, { x: 0.9, y: 0.5 }];
    const converted = convertEditorMappingType(source, "SingleTap", { width: 1000, height: 500 });
    expect(converted.type).toBe("SingleTap");
    expect("positions" in converted).toBe(false);
    expect(converted.position).toEqual({ x: 0.4, y: 0.5 });
  });

  it("preserves directional bindings between compatible controller types", () => {
    const source = createMapping("DirectionPad", { x: 0.2, y: 0.8 });
    source.bind = { type: "Button", up: ["KeyW"], down: ["KeyS"], left: ["KeyA"], right: ["KeyD"] };

    const converted = convertEditorMappingType(source, "PadCastSpell", { width: 1000, height: 500 });
    expect(converted).toMatchObject({
      type: "PadCastSpell",
      pad_bind: source.bind,
      release_mode: "OnRelease",
    });
  });

  it("resets a release mode unsupported by the target controller", () => {
    const source = createMapping("MouseCastSpell", { x: 0.6, y: 0.7 });
    if (source.type !== "MouseCastSpell") throw new Error("unexpected mapping type");
    source.release_mode = "OnPress";

    const converted = convertEditorMappingType(source, "PadCastSpell", { width: 1000, height: 500 });
    expect(converted.type).toBe("PadCastSpell");
    if (converted.type !== "PadCastSpell") throw new Error("unexpected mapping type");
    expect(converted.release_mode).toBe("OnRelease");
  });
});
