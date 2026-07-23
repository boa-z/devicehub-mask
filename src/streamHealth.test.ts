import { describe, expect, it } from "vitest";
import { hasDecodedVideoActivity, isVideoStreamStalled, videoStallTimeoutMs } from "./streamHealth";
import type { StreamMetrics } from "./types";

const metrics = (decodedFps: number, sourceFps = decodedFps): StreamMetrics => ({
  source_fps: sourceFps,
  decoded_fps: decodedFps,
  published_fps: 0,
  sent_fps: 0,
  backend_dropped_fps: 0,
  jpeg_encode_ms: 0,
  frame_age_ms: 0,
  websocket_send_ms: 0,
  presentation_ack_ms: 0,
  megabits_per_second: 0,
});

describe("video stream health", () => {
  it("treats duplicate decoded frames as activity even when none are published", () => {
    expect(hasDecodedVideoActivity(metrics(60))).toBe(true);
  });

  it("does not treat incoming source frames as healthy when decoding has stopped", () => {
    expect(hasDecodedVideoActivity(metrics(0, 60))).toBe(false);
  });

  it("reports a stall only after prior activity exceeds the recovery window", () => {
    expect(isVideoStreamStalled(10_000, 0)).toBe(false);
    expect(isVideoStreamStalled(10_000, 10_000 - videoStallTimeoutMs)).toBe(false);
    expect(isVideoStreamStalled(10_001, 10_000 - videoStallTimeoutMs)).toBe(true);
  });
});
