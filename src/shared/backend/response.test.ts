import { describe, expect, it } from "vitest";
import { BackendResponseError, readBackendJson, requireBackendSuccess } from "./response";

describe("backend responses", () => {
  it("preserves structured API error metadata", async () => {
    const response = new Response(JSON.stringify({
      error: {
        code: "device_locked",
        message: "Unlock the device",
        retryable: true,
        suggested_action: "unlock_device",
      },
    }), { status: 409, headers: { "content-type": "application/json" } });

    const error = await requireBackendSuccess(response).catch((reason) => reason);
    expect(error).toBeInstanceOf(BackendResponseError);
    expect(error).toMatchObject({
      status: 409,
      code: "device_locked",
      message: "Unlock the device",
      retryable: true,
      suggestedAction: "unlock_device",
    });
  });

  it("keeps plain text errors and rejects malformed success payloads", async () => {
    await expect(requireBackendSuccess(new Response("service unavailable", { status: 503 })))
      .rejects.toMatchObject({ message: "service unavailable", retryable: true });
    await expect(readBackendJson(new Response("not-json", { status: 200 })))
      .rejects.toMatchObject({ code: "invalid_response", retryable: true });
  });

  it("returns successful JSON payloads", async () => {
    await expect(readBackendJson<{ count: number }>(new Response('{"count":2}')))
      .resolves.toEqual({ count: 2 });
  });
});
