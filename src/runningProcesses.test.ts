import { describe, expect, it } from "vitest";
import { filterRunningProcesses } from "./runningProcesses";
import type { RunningProcess } from "./types";

const processes: RunningProcess[] = [
  { pid: 42, name: "Example", app_name: "Example App", is_application: true },
  { pid: 7, name: "networkd", app_name: null, is_application: false },
];

describe("running process inventory", () => {
  it("searches process names, app names, and exact PID text", () => {
    expect(filterRunningProcesses(processes, "example app")).toEqual([processes[0]]);
    expect(filterRunningProcesses(processes, "NETWORK")).toEqual([processes[1]]);
    expect(filterRunningProcesses(processes, "42")).toEqual([processes[0]]);
  });

  it("preserves the backend order when no query is active", () => {
    expect(filterRunningProcesses(processes, "  ")).toBe(processes);
  });
});
