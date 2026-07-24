import { describe, expect, it } from "vitest";
import { bluetoothCaptureFilename, networkCaptureFilename, networkCaptureRunning } from "./networkCapture";
import type { NetworkCaptureStatus } from "./types";

function status(state: NetworkCaptureStatus["state"]): NetworkCaptureStatus {
  return {
    state,
    packet_count: 0,
    bytes_written: 0,
    elapsed_ms: 0,
    duration_seconds: null,
    stop_reason: null,
    error: null,
  };
}

describe("network capture", () => {
  it("recognizes only active capture states", () => {
    expect(networkCaptureRunning(status("starting"))).toBe(true);
    expect(networkCaptureRunning(status("capturing"))).toBe(true);
    expect(networkCaptureRunning(status("completed"))).toBe(false);
    expect(networkCaptureRunning(status("failed"))).toBe(false);
  });

  it("creates a portable timestamped pcap filename", () => {
    expect(networkCaptureFilename("Boa's iPhone / Lab", new Date("2026-07-24T01:02:03.004Z")))
      .toBe("devicehub-mask_Boa's iPhone - Lab_2026-07-24T01-02-03-004Z.pcap");
    expect(networkCaptureFilename("", new Date("2026-07-24T01:02:03.004Z")))
      .toContain("devicehub-mask_iPhone_");
  });

  it("creates a distinct Bluetooth capture filename", () => {
    expect(bluetoothCaptureFilename("Lab / iPhone", new Date("2026-07-24T01:02:03.004Z")))
      .toBe("devicehub-mask_Lab - iPhone_bluetooth_2026-07-24T01-02-03-004Z.pcap");
  });
});
