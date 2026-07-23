import type { NetworkCaptureStatus } from "./types";

export const networkCaptureDurations = [10, 30, 60, 120, 300] as const;

export function networkCaptureRunning(status: NetworkCaptureStatus): boolean {
  return status.state === "starting" || status.state === "capturing";
}

export function networkCaptureFilename(deviceName: string, now = new Date()): string {
  const safeName = deviceName.trim().replace(/[<>:"/\\|?*]+/g, "-") || "iPhone";
  const timestamp = now.toISOString().replace(/[:.]/g, "-");
  return `devicehub-mask_${safeName}_${timestamp}.pcap`;
}
