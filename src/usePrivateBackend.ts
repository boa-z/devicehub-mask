import { useCallback, useEffect } from "react";
import { useBackend } from "./app/providers/backendContext";
import type { BackendRequest } from "./backendConnection";

export { browserBackendConnection, requestPrivateBackend } from "./backendConnection";
export type { BackendConnection, BackendRequest } from "./backendConnection";

export function usePrivateBackend(
  onUnavailable: (error: unknown) => void,
  notReadyMessage: string,
) {
  const { client, connection, error } = useBackend();
  useEffect(() => {
    if (error) onUnavailable(error);
  }, [error, onUnavailable]);

  const request = useCallback<BackendRequest>((path, init) => {
    if (!client) return Promise.reject(new Error(notReadyMessage));
    return client.request(path, init);
  }, [client, notReadyMessage]);

  return { backend: connection, request };
}
