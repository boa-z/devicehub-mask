import { describe, expect, it } from "vitest";
import { sortProcesses } from "./processPerformance";
import type { ProcessPerformance } from "./types";

const processes: ProcessPerformance[] = [
  { pid: 30, name: "Memory", cpu_percent: 1, memory_bytes: 500 },
  { pid: 20, name: "CPU", cpu_percent: 25, memory_bytes: 100 },
  { pid: 10, name: "Missing", cpu_percent: null, memory_bytes: null },
];

describe("process performance", () => {
  it("sorts missing metrics after CPU and memory leaders", () => {
    expect(sortProcesses(processes, "cpu").map(({ pid }) => pid)).toEqual([20, 30, 10]);
    expect(sortProcesses(processes, "memory").map(({ pid }) => pid)).toEqual([30, 20, 10]);
  });

  it("does not mutate the backend snapshot", () => {
    const original = [...processes];
    sortProcesses(processes, "cpu");
    expect(processes).toEqual(original);
  });
});
