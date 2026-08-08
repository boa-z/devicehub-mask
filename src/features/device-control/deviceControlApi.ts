import type { BackendClient } from "../../shared/backend/client";

export function createDeviceControlApi(client: BackendClient) {
  return {
    async pasteText(deviceId: string, text: string) {
      const response = await client.requestForDevice(deviceId, "/api/device/text/paste", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ text }),
      });
      if (!response.ok) throw new Error((await response.text()) || `${response.status} ${response.statusText}`);
    },
  };
}
