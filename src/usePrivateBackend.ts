import { invoke, isTauri } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  browserBackendConnection,
  requestPrivateBackend,
  type BackendConnection,
  type BackendRequest,
} from "./backendConnection";
import { logFrontend } from "./diagnostics";

export { browserBackendConnection, requestPrivateBackend } from "./backendConnection";
export type { BackendConnection, BackendRequest } from "./backendConnection";

export function usePrivateBackend(
  onUnavailable: (error: unknown) => void,
  notReadyMessage: string,
  selectedDeviceId: string | null,
) {
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
    const headers = new Headers(init?.headers);
    if (selectedDeviceId) headers.set("x-devicehub-device", selectedDeviceId);
    return requestPrivateBackend(backend, path, { ...init, headers });
  }, [backend, notReadyMessage, selectedDeviceId]);

  return { backend, request };
}
