import { describe, expect, it } from "vitest";
import { mergeConsoleLines } from "../appConsole";
import type { AppConsoleSnapshot } from "../types";

function snapshot(lines: AppConsoleSnapshot["lines"], reset = false): AppConsoleSnapshot {
  return {
    phase: "running",
    bundle_id: "com.example.App",
    started_at_ms: 1,
    ended_at_ms: null,
    total_bytes: 0,
    total_lines: lines.length,
    dropped_lines: 0,
    next_sequence: (lines.at(-1)?.sequence ?? 0) + 1,
    reset,
    lines,
    last_error: null,
  };
}

describe("app console output", () => {
  it("merges only newer incremental lines", () => {
    expect(mergeConsoleLines(
      [{ sequence: 1, text: "first" }],
      snapshot([
        { sequence: 1, text: "duplicate" },
        { sequence: 2, text: "second" },
      ]),
    )).toEqual([
      { sequence: 1, text: "first" },
      { sequence: 2, text: "second" },
    ]);
  });

  it("replaces local output after a server-side cursor reset", () => {
    expect(mergeConsoleLines(
      [{ sequence: 1, text: "stale" }],
      snapshot([{ sequence: 12, text: "retained" }], true),
    )).toEqual([{ sequence: 12, text: "retained" }]);
  });

  it("bounds the local incremental history", () => {
    const current = Array.from({ length: 1_000 }, (_, index) => ({
      sequence: index + 1,
      text: `line ${index + 1}`,
    }));
    const merged = mergeConsoleLines(current, snapshot([{ sequence: 1_001, text: "latest" }]));
    expect(merged).toHaveLength(1_000);
    expect(merged[0].sequence).toBe(2);
    expect(merged.at(-1)?.text).toBe("latest");
  });
});
