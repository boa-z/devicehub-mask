import { describe, expect, it, vi } from "vitest";
import { createEditorMapping, duplicateEditorMapping } from "./mappingEditor";
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
});
