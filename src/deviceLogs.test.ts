import { describe, expect, it } from "vitest";
import { deviceLogContext, filterDeviceLogs, formatDeviceLogLine } from "./deviceLogs";
import type { DeviceLogEntry } from "./types";

const entries: DeviceLogEntry[] = [
  {
    sequence: 1,
    received_at_ms: 1,
    message: "connection opened",
    level: "notice",
    process: "ExampleGame",
    pid: 42,
    subsystem: "com.example.network",
    category: "connection",
    filename: "Network.swift",
  },
  {
    sequence: 2,
    received_at_ms: 2,
    message: "legacy syslog line",
    level: null,
    process: null,
    pid: null,
    subsystem: null,
    category: null,
    filename: null,
  },
];

describe("device logs", () => {
  it("filters structured logs across message and metadata", () => {
    expect(filterDeviceLogs(entries, "examplegame", "all")).toEqual([entries[0]]);
    expect(filterDeviceLogs(entries, "NETWORK.SWIFT", "all")).toEqual([entries[0]]);
    expect(filterDeviceLogs(entries, "42", "notice")).toEqual([entries[0]]);
    expect(filterDeviceLogs(entries, "legacy", "notice")).toEqual([]);
    expect(filterDeviceLogs(entries, "", "all")).toEqual(entries);
  });

  it("formats structured context without placeholders for syslog", () => {
    expect(deviceLogContext(entries[0])).toBe("ExampleGame [42] com.example.network:connection Network.swift");
    expect(deviceLogContext(entries[1])).toBe("");
    expect(formatDeviceLogLine(entries[0], "12:00:00.000")).toContain("NOTICE ExampleGame [42]");
    expect(formatDeviceLogLine(entries[1], "12:00:00.000")).toBe("12:00:00.000 legacy syslog line");
  });
});
