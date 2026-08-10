import { describe, expect, it } from "vitest";
import {
  operationPollDelay,
  operationWorkspace,
  parseManagedOperations,
  type ManagedOperation,
} from "./deviceOperations";

function operation(overrides: Partial<ManagedOperation> = {}): ManagedOperation {
  return {
    id: 1,
    kind: "device_backup",
    phase: "running",
    stage: "copying",
    label: null,
    progress_percent: 25,
    cancellable: true,
    started_at_ms: 1_000,
    updated_at_ms: 2_000,
    error: null,
    ...overrides,
  };
}

describe("device operations", () => {
  it("rejects malformed backend operation records", () => {
    expect(() => parseManagedOperations([{ ...operation(), kind: "unknown" }])).toThrow();
    expect(parseManagedOperations([operation({ phase: "succeeded" })])).toHaveLength(1);
  });

  it("polls active work quickly and backs off when idle", () => {
    expect(operationPollDelay([operation()], false)).toBe(750);
    expect(operationPollDelay([], true)).toBe(2_500);
    expect(operationPollDelay([], false)).toBe(5_000);
  });

  it("routes activity to the workspace that owns its controls", () => {
    expect(operationWorkspace("device_file_export")).toBe("afc");
    expect(operationWorkspace("network_capture")).toBe("performance");
    expect(operationWorkspace("log_archive")).toBe("logs");
    expect(operationWorkspace("developer_image_mount")).toBe("device");
  });
});
