import type { StreamMetrics } from "./types";

export const videoStallTimeoutMs = 7_000;
export const videoSourceFreshnessMs = 2_500;

export function hasSourceVideoActivity(metrics: StreamMetrics): boolean {
  return Number.isFinite(metrics.source_fps) && metrics.source_fps > 0;
}

export function hasDecodedVideoActivity(metrics: StreamMetrics): boolean {
  return Number.isFinite(metrics.decoded_fps) && metrics.decoded_fps > 0;
}

export function isVideoStreamStalled(
  now: number,
  lastSourceAt: number,
  lastDecodedAt: number,
  timeoutMs = videoStallTimeoutMs,
  sourceFreshnessMs = videoSourceFreshnessMs,
): boolean {
  return Number.isFinite(now)
    && Number.isFinite(lastSourceAt)
    && Number.isFinite(lastDecodedAt)
    && lastSourceAt > 0
    && lastDecodedAt > 0
    && now - lastSourceAt <= sourceFreshnessMs
    && now - lastDecodedAt > timeoutMs;
}
