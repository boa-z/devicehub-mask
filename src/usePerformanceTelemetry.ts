import { useEffect, useState } from "react";
import { logFrontend } from "./diagnostics";
import type { PerformanceView } from "./types";
import type { BackendRequest } from "./usePrivateBackend";

type Options = {
  activeUdid: string | null;
  backendReady: boolean;
  enabled: boolean;
  request: BackendRequest;
};

export function usePerformanceTelemetry({ activeUdid, backendReady, enabled, request }: Options) {
  const [view, setView] = useState<PerformanceView | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!backendReady) return;
    const method = enabled ? "PUT" : "DELETE";
    void request("/api/performance/sampling", { method }).then((response) => {
      if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
    }).catch((samplingError) => {
      logFrontend("warn", "performance", "set_sampling", samplingError);
      if (enabled) setError(String(samplingError));
    });
  }, [backendReady, enabled, request]);

  useEffect(() => {
    if (!enabled) {
      setView(null);
      setError(null);
      return;
    }
    setView(null);
    setError(null);
    let disposed = false;
    let loading = false;
    let failureLogged = false;
    const refresh = async () => {
      if (loading) return;
      loading = true;
      try {
        const response = await request("/api/performance");
        if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
        const next = await response.json() as PerformanceView;
        if (!disposed) {
          setView(next);
          setError(null);
          failureLogged = false;
        }
      } catch (refreshError) {
        if (!disposed) {
          setError(String(refreshError));
          if (!failureLogged) {
            failureLogged = true;
            logFrontend("warn", "performance", "read_telemetry", refreshError);
          }
        }
      } finally {
        loading = false;
      }
    };
    void refresh();
    const timer = window.setInterval(() => void refresh(), 1_000);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [activeUdid, enabled, request]);

  return { view, error };
}

export function useDeviceLogDemand({ backendReady, enabled, request }: Omit<Options, "activeUdid">) {
  useEffect(() => {
    if (!backendReady) return;
    const method = enabled ? "PUT" : "DELETE";
    void request("/api/device/logs/streaming", { method }).then((response) => {
      if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
    }).catch((demandError) => logFrontend("warn", "device_logs", "set_streaming", demandError));
  }, [backendReady, enabled, request]);
}
