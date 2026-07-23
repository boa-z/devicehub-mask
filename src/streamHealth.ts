import type { StreamMetrics } from "./types";

export const videoStallTimeoutMs = 7_000;

export function hasDecodedVideoActivity(metrics: StreamMetrics): boolean {
  return Number.isFinite(metrics.decoded_fps) && metrics.decoded_fps > 0;
}

export function isVideoStreamStalled(now: number, lastActivityAt: number, timeoutMs = videoStallTimeoutMs): boolean {
  return Number.isFinite(now)
    && Number.isFinite(lastActivityAt)
    && lastActivityAt > 0
    && now - lastActivityAt > timeoutMs;
}
