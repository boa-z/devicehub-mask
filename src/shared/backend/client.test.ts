import { describe, expect, it, vi } from "vitest";
import { BackendClient, browserBackendConnection, requestPrivateBackend } from "./client";

describe("backend client", () => {
  it("adds bearer authentication and a device target independently", async () => {
    const fetcher = vi.fn((input: string | URL | Request, init?: RequestInit) => {
      void input;
      void init;
      return Promise.resolve(new Response(null, { status: 204 }));
    });
    const client = new BackendClient(
      { origin: "https://127.0.0.1:54321", token: "session-token" },
      fetcher,
    );

    await client.requestForDevice("phone::usb", "/api/device/details", {
      headers: { authorization: "Bearer stale", "x-test": "value" },
    });

    const [url, init] = fetcher.mock.calls[0];
    const headers = new Headers(init?.headers);
    expect(url).toBe("https://127.0.0.1:54321/api/device/details");
    expect(headers.get("authorization")).toBe("Bearer session-token");
    expect(headers.get("x-devicehub-device")).toBe("phone::usb");
    expect(headers.get("x-test")).toBe("value");
  });

  it("builds the authenticated websocket target without selecting a device", () => {
    const client = new BackendClient({ origin: "http://127.0.0.1:54321", token: "session-token" });

    expect(client.websocketUrl("/api/ws", { device_id: "phone::usb" }))
      .toBe("ws://127.0.0.1:54321/api/ws?device_id=phone%3A%3Ausb");
    expect(client.websocketProtocols()).toEqual(["devicehub-mask", "session-token"]);
  });
});

describe("backend connection compatibility", () => {
  it("keeps the existing private request behavior", async () => {
    const fetcher = vi.fn((input: string | URL | Request, init?: RequestInit) => {
      void input;
      void init;
      return Promise.resolve(new Response(null, { status: 204 }));
    });
    await requestPrivateBackend(
      { origin: "http://127.0.0.1:54321", token: "session-token" },
      "/api/status",
      undefined,
      fetcher,
    );
    expect(fetcher).toHaveBeenCalledOnce();
  });

  it("still reads a browser token from the fragment", () => {
    const values = new Map<string, string>();
    expect(browserBackendConnection({
      origin: "http://127.0.0.1:8080",
      hash: "#access_token=token",
      pathname: "/",
      search: "",
    }, {
      getItem: (key) => values.get(key) ?? null,
      setItem: (key, value) => values.set(key, value),
    }, () => undefined).token).toBe("token");
  });
});
