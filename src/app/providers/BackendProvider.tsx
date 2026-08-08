import { invoke, isTauri } from "@tauri-apps/api/core";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { logFrontend } from "../../diagnostics";
import {
  BackendClient,
  browserBackendConnection,
  type BackendConnection,
} from "../../shared/backend/client";
import { BackendContext, type BackendContextValue } from "./backendContext";

export function BackendProvider({ children }: { children: ReactNode }) {
  const [connection, setConnection] = useState<BackendConnection | null>(null);
  const [error, setError] = useState<unknown | null>(null);

  useEffect(() => {
    let disposed = false;
    const pending = isTauri()
      ? invoke<BackendConnection>("backend_connection")
      : Promise.resolve().then(() => browserBackendConnection());
    void pending
      .then((next) => {
        if (disposed) return;
        logFrontend("info", "backend", "connection_ready", "Private backend connection acquired");
        setConnection(next);
        setError(null);
      })
      .catch((nextError) => {
        if (disposed) return;
        logFrontend("error", "backend", "connection_failed", nextError);
        setConnection(null);
        setError(nextError);
      });
    return () => {
      disposed = true;
    };
  }, []);

  const value = useMemo<BackendContextValue>(() => ({
    client: connection ? new BackendClient(connection) : null,
    connection,
    error,
  }), [connection, error]);

  return <BackendContext.Provider value={value}>{children}</BackendContext.Provider>;
}
