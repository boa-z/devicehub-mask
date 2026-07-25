import { describe, expect, it } from "vitest";
import { createUndoHistory, reduceUndoHistory } from "./useUndoHistory";

describe("undo history", () => {
  it("undoes and redoes immutable updates", () => {
    let state = createUndoHistory({ value: 1 });
    state = reduceUndoHistory(state, { type: "update", update: { value: 2 }, timestamp: 1 });
    state = reduceUndoHistory(state, { type: "undo" });
    expect(state.present).toEqual({ value: 1 });
    state = reduceUndoHistory(state, { type: "redo" });
    expect(state.present).toEqual({ value: 2 });
  });

  it("coalesces consecutive updates with the same merge key", () => {
    let state = createUndoHistory("before");
    state = reduceUndoHistory(state, { type: "update", update: "a", mergeKey: "name", timestamp: 100 });
    state = reduceUndoHistory(state, { type: "update", update: "ab", mergeKey: "name", timestamp: 200 });
    state = reduceUndoHistory(state, { type: "update", update: "abc", mergeKey: "name", timestamp: 300 });
    expect(state.past).toEqual(["before"]);
    expect(reduceUndoHistory(state, { type: "undo" }).present).toBe("before");
  });

  it("keeps different fields as separate history steps", () => {
    let state = createUndoHistory({ name: "A", duration: 50 });
    state = reduceUndoHistory(state, { type: "update", update: { name: "AB", duration: 50 }, mergeKey: "name", timestamp: 100 });
    state = reduceUndoHistory(state, { type: "update", update: { name: "AB", duration: 80 }, mergeKey: "duration", timestamp: 200 });
    expect(state.past).toHaveLength(2);
    expect(reduceUndoHistory(state, { type: "undo" }).present).toEqual({ name: "AB", duration: 50 });
  });

  it("starts a new step after the merge window and clears redo on a new branch", () => {
    let state = createUndoHistory(0);
    state = reduceUndoHistory(state, { type: "update", update: 1, mergeKey: "drag", timestamp: 100, mergeWindowMs: 50 });
    state = reduceUndoHistory(state, { type: "update", update: 2, mergeKey: "drag", timestamp: 200, mergeWindowMs: 50 });
    state = reduceUndoHistory(state, { type: "undo" });
    expect(state.present).toBe(1);
    expect(state.future).toEqual([2]);
    state = reduceUndoHistory(state, { type: "update", update: 3, timestamp: 300 });
    expect(state.future).toEqual([]);
  });

  it("bounds retained snapshots and resets external documents", () => {
    let state = createUndoHistory(0);
    for (let value = 1; value <= 5; value += 1) {
      state = reduceUndoHistory(state, { type: "update", update: value, timestamp: value, limit: 2 });
    }
    expect(state.past).toEqual([3, 4]);
    expect(reduceUndoHistory(state, { type: "reset", value: 10 })).toEqual(createUndoHistory(10));
  });
});
