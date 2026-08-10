import { useEffect, useState } from "react";
import type { BackendRequest } from "../../shared/backend/client";
import { fetchManagedOperations, operationPollDelay, type ManagedOperation } from "./deviceOperations";

export function useManagedOperations(
  request: BackendRequest,
  deviceId: string | null,
  enabled: boolean,
  centerOpen: boolean,
) {
  const [operations, setOperations] = useState<ManagedOperation[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);

  useEffect(() => {
    setOperations([]);
    setError(null);
    if (!deviceId || !enabled) return;

    let disposed = false;
    let timer: number | null = null;
    let controller: AbortController | null = null;
    const poll = async () => {
      controller = new AbortController();
      let next: ManagedOperation[] = [];
      try {
        next = await fetchManagedOperations(request, controller.signal);
        if (disposed) return;
        setOperations(next);
        setError(null);
      } catch (reason) {
        if (disposed || controller.signal.aborted) return;
        setError(String(reason));
      }
      if (!disposed) timer = window.setTimeout(poll, operationPollDelay(next, centerOpen));
    };
    void poll();
    return () => {
      disposed = true;
      controller?.abort();
      if (timer !== null) window.clearTimeout(timer);
    };
  }, [centerOpen, deviceId, enabled, refreshKey, request]);

  return {
    operations,
    error,
    refresh: () => setRefreshKey((value) => value + 1),
  };
}
