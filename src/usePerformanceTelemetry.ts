import { useEffect, useState } from "react";
import { logFrontend } from "./diagnostics";
import type { PerformanceView } from "./types";
import type { BackendRequest } from "./usePrivateBackend";

type Options = {
  activeDeviceId: string | null;
  backendReady: boolean;
  enabled: boolean;
  request: BackendRequest;
};

export function usePerformanceTelemetry({ activeDeviceId, backendReady, enabled, request }: Options) {
  const [view, setView] = useState<PerformanceView | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!backendReady || !activeDeviceId || !enabled) return;
    let disposed = false;
    void request("/api/performance/sampling", { method: "PUT" }).then((response) => {
      if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
    }).catch((samplingError) => {
      if (disposed) return;
      logFrontend("warn", "performance", "set_sampling", samplingError);
      setError(String(samplingError));
    });
    return () => {
      disposed = true;
      void request("/api/performance/sampling", { method: "DELETE" }).catch(() => undefined);
    };
  }, [activeDeviceId, backendReady, enabled, request]);

  useEffect(() => {
    if (!enabled || !activeDeviceId) {
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
  }, [activeDeviceId, enabled, request]);

  return { view, error };
}

export function useDeviceLogDemand({ activeDeviceId, backendReady, enabled, request }: Options) {
  useEffect(() => {
    if (!backendReady || !activeDeviceId || !enabled) return;
    let disposed = false;
    void request("/api/device/logs/streaming", { method: "PUT" }).then((response) => {
      if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
    }).catch((demandError) => {
      if (!disposed) logFrontend("warn", "device_logs", "set_streaming", demandError);
    });
    return () => {
      disposed = true;
      void request("/api/device/logs/streaming", { method: "DELETE" }).catch(() => undefined);
    };
  }, [activeDeviceId, backendReady, enabled, request]);
}
