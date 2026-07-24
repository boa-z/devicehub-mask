import { describe, expect, it, vi } from "vitest";
import { requestPrivateBackend } from "./usePrivateBackend";

describe("private backend requests", () => {
  it("joins an absolute API path and replaces caller authorization", async () => {
    const fetcher = vi.fn(async (input: string | URL | Request, init?: RequestInit) => {
      void input;
      void init;
      return new Response(null, { status: 204 });
    });
    await requestPrivateBackend(
      { origin: "http://127.0.0.1:54321", token: "session-token" },
      "/api/performance",
      { headers: { authorization: "Bearer stale", "x-test": "value" } },
      fetcher,
    );

    expect(fetcher).toHaveBeenCalledOnce();
    const [url, init] = fetcher.mock.calls[0];
    const headers = new Headers(init?.headers);
    expect(url).toBe("http://127.0.0.1:54321/api/performance");
    expect(headers.get("authorization")).toBe("Bearer session-token");
    expect(headers.get("x-test")).toBe("value");
  });

  it("rejects paths that could escape the private origin boundary", async () => {
    await expect(requestPrivateBackend(
      { origin: "http://127.0.0.1:54321", token: "session-token" },
      "https://example.com/",
    )).rejects.toThrow("path must be absolute");
  });
});
