import type { BluetoothCaptureStatus, NetworkCaptureStatus } from "./types";

export const networkCaptureDurations = [10, 30, 60, 120, 300] as const;

export function networkCaptureRunning(status: NetworkCaptureStatus | BluetoothCaptureStatus): boolean {
  return status.state === "starting" || status.state === "capturing";
}

export function bluetoothCaptureFilename(deviceName: string, now = new Date()): string {
  const safeName = deviceName.trim().replace(/[<>:"/\\|?*]+/g, "-") || "iPhone";
  const timestamp = now.toISOString().replace(/[:.]/g, "-");
  return `devicehub-mask_${safeName}_bluetooth_${timestamp}.pcap`;
}

export function networkCaptureFilename(deviceName: string, now = new Date()): string {
  const safeName = deviceName.trim().replace(/[<>:"/\\|?*]+/g, "-") || "iPhone";
  const timestamp = now.toISOString().replace(/[:.]/g, "-");
  return `devicehub-mask_${safeName}_${timestamp}.pcap`;
}
