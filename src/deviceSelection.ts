import type { DeviceStatus } from "./types";
import type { BackendRequest } from "./usePrivateBackend";

const defaultTimeoutMs = 2_000;
const defaultPollIntervalMs = 50;

function delay(durationMs: number) {
  return new Promise<void>((resolve) => globalThis.setTimeout(resolve, durationMs));
}

/** Wait until the manager has published the target's session registry entry. */
export async function waitForDeviceSession(
  request: BackendRequest,
  deviceId: string,
  timeoutMs = defaultTimeoutMs,
  pollIntervalMs = defaultPollIntervalMs,
): Promise<DeviceStatus> {
  const deadline = performance.now() + timeoutMs;
  let lastError: unknown = null;
  do {
    try {
      const response = await request("/api/status");
      if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
      const status = await response.json() as DeviceStatus;
      const target = status.devices.find((device) => device.id === deviceId);
      if (status.active_device_id === deviceId && target && target.session_status !== null) return status;
    } catch (error) {
      lastError = error;
    }
    await delay(pollIntervalMs);
  } while (performance.now() < deadline);

  const detail = lastError ? `: ${String(lastError)}` : "";
  throw new Error(`device session was not registered within ${timeoutMs} ms${detail}`);
}
