import { invoke, isTauri } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import { logFrontend } from "./diagnostics";

export type BackendConnection = { origin: string; token: string };
export type BackendRequest = (path: string, init?: RequestInit) => Promise<Response>;
type FetchFunction = (input: string | URL | Request, init?: RequestInit) => Promise<Response>;
const HEADLESS_TOKEN_KEY = "devicehub.headless.token";

export function browserBackendConnection(
  location: Pick<Location, "origin" | "hash" | "pathname" | "search"> = window.location,
  storage: Pick<Storage, "getItem" | "setItem"> = window.sessionStorage,
  clearFragment: (url: string) => void = (url) => window.history.replaceState(null, "", url),
): BackendConnection {
  const fragment = new URLSearchParams(location.hash.replace(/^#/, ""));
  const supplied = fragment.get("access_token")?.trim();
  if (supplied) {
    storage.setItem(HEADLESS_TOKEN_KEY, supplied);
    clearFragment(`${location.pathname}${location.search}`);
  }
  const token = supplied || storage.getItem(HEADLESS_TOKEN_KEY)?.trim();
  if (!token) {
    throw new Error("Headless access token is missing. Open the URL printed by devicehub-headless.");
  }
  return { origin: location.origin, token };
}

export function requestPrivateBackend(
  backend: BackendConnection,
  path: string,
  init: RequestInit = {},
  fetcher: FetchFunction = fetch,
) {
  if (!path.startsWith("/")) return Promise.reject(new Error("private backend path must be absolute"));
  const headers = new Headers(init.headers);
  headers.set("authorization", `Bearer ${backend.token}`);
  return fetcher(`${backend.origin}${path}`, { ...init, headers });
}

export function usePrivateBackend(onUnavailable: (error: unknown) => void, notReadyMessage: string) {
  const [backend, setBackend] = useState<BackendConnection | null>(null);
  const unavailableRef = useRef(onUnavailable);
  unavailableRef.current = onUnavailable;

  useEffect(() => {
    let disposed = false;
    const connection = isTauri()
      ? invoke<BackendConnection>("backend_connection")
      : Promise.resolve().then(() => browserBackendConnection());
    void connection
      .then((connection) => {
        if (disposed) return;
        logFrontend("info", "backend", "connection_ready", "Private backend connection acquired");
        setBackend(connection);
      })
      .catch((error) => {
        if (disposed) return;
        logFrontend("error", "backend", "connection_failed", error);
        unavailableRef.current(error);
      });
    return () => {
      disposed = true;
    };
  }, []);

  const request = useCallback<BackendRequest>((path, init) => {
    if (!backend) return Promise.reject(new Error(notReadyMessage));
    return requestPrivateBackend(backend, path, init);
  }, [backend, notReadyMessage]);

  return { backend, request };
}
