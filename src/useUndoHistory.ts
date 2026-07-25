import { useCallback, useReducer } from "react";

export const DEFAULT_HISTORY_LIMIT = 100;
export const DEFAULT_HISTORY_MERGE_WINDOW_MS = 750;

export type UndoHistoryState<T> = {
  past: T[];
  present: T;
  future: T[];
  mergeKey: string | null;
  mergedAt: number;
};

export type UndoHistoryAction<T> =
  | { type: "update"; update: T | ((current: T) => T); mergeKey?: string; timestamp: number; limit?: number; mergeWindowMs?: number }
  | { type: "reset"; value: T }
  | { type: "undo" }
  | { type: "redo" };

export function createUndoHistory<T>(value: T): UndoHistoryState<T> {
  return { past: [], present: value, future: [], mergeKey: null, mergedAt: 0 };
}

export function reduceUndoHistory<T>(state: UndoHistoryState<T>, action: UndoHistoryAction<T>): UndoHistoryState<T> {
  if (action.type === "reset") return createUndoHistory(action.value);
  if (action.type === "undo") {
    const previous = state.past.at(-1);
    if (previous === undefined) return state;
    return {
      past: state.past.slice(0, -1),
      present: previous,
      future: [state.present, ...state.future],
      mergeKey: null,
      mergedAt: 0,
    };
  }
  if (action.type === "redo") {
    const [next, ...future] = state.future;
    if (next === undefined) return state;
    return {
      past: [...state.past, state.present],
      present: next,
      future,
      mergeKey: null,
      mergedAt: 0,
    };
  }

  const next = typeof action.update === "function"
    ? (action.update as (current: T) => T)(state.present)
    : action.update;
  if (Object.is(next, state.present)) return state;

  const mergeWindowMs = action.mergeWindowMs ?? DEFAULT_HISTORY_MERGE_WINDOW_MS;
  const mergesPrevious = Boolean(
    action.mergeKey
    && action.mergeKey === state.mergeKey
    && action.timestamp - state.mergedAt <= mergeWindowMs,
  );
  const limit = Math.max(1, action.limit ?? DEFAULT_HISTORY_LIMIT);
  const past = mergesPrevious ? state.past : [...state.past, state.present].slice(-limit);
  return {
    past,
    present: next,
    future: [],
    mergeKey: action.mergeKey ?? null,
    mergedAt: action.timestamp,
  };
}

type UpdateOptions = { mergeKey?: string };

export function useUndoHistory<T>(initializer: () => T) {
  const [state, dispatch] = useReducer(
    reduceUndoHistory<T>,
    undefined,
    () => createUndoHistory(initializer()),
  );
  const update = useCallback((value: T | ((current: T) => T), options: UpdateOptions = {}) => {
    dispatch({ type: "update", update: value, mergeKey: options.mergeKey, timestamp: performance.now() });
  }, []);
  const reset = useCallback((value: T) => dispatch({ type: "reset", value }), []);
  const undo = useCallback(() => dispatch({ type: "undo" }), []);
  const redo = useCallback(() => dispatch({ type: "redo" }), []);

  return {
    value: state.present,
    update,
    reset,
    undo,
    redo,
    canUndo: state.past.length > 0,
    canRedo: state.future.length > 0,
  };
}
