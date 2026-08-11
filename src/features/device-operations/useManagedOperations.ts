import { useCallback, useEffect, useRef, useState } from "react";
import type { BackendRequest } from "../../shared/backend/client";
import { cancelManagedOperation, fetchManagedOperations, operationPollDelay, type ManagedOperation } from "./deviceOperations";

export function useManagedOperations(
  request: BackendRequest,
  deviceId: string | null,
  enabled: boolean,
  centerOpen: boolean,
) {
  const [operations, setOperations] = useState<ManagedOperation[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  const cancelControllersRef = useRef(new Set<AbortController>());

  useEffect(() => {
    const controllers = cancelControllersRef.current;
    setActionError(null);
    return () => {
      for (const controller of controllers) controller.abort();
      controllers.clear();
    };
  }, [deviceId]);

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

  const cancelOperation = useCallback(async (operationId: number) => {
    if (!deviceId || !enabled) return;
    const controller = new AbortController();
    cancelControllersRef.current.add(controller);
    setActionError(null);
    setOperations((current) => current.map((operation) => (
      operation.id === operationId ? { ...operation, phase: "cancelling" } : operation
    )));
    try {
      await cancelManagedOperation(request, operationId, controller.signal);
      if (!controller.signal.aborted) setRefreshKey((value) => value + 1);
    } catch (reason) {
      if (!controller.signal.aborted) {
        setOperations((current) => current.map((operation) => (
          operation.id === operationId && operation.phase === "cancelling"
            ? { ...operation, phase: "running" }
            : operation
        )));
        setActionError(reason instanceof Error ? reason.message : String(reason));
      }
    } finally {
      cancelControllersRef.current.delete(controller);
    }
  }, [deviceId, enabled, request]);

  return {
    operations,
    error,
    actionError,
    cancelOperation,
    clearActionError: () => setActionError(null),
    refresh: () => setRefreshKey((value) => value + 1),
  };
}
