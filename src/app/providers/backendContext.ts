import { createContext, useContext } from "react";
import type { BackendClient, BackendConnection } from "../../shared/backend/client";

export type BackendContextValue = {
  client: BackendClient | null;
  connection: BackendConnection | null;
  error: unknown | null;
};

export const BackendContext = createContext<BackendContextValue | null>(null);

export function useBackend() {
  const value = useContext(BackendContext);
  if (!value) throw new Error("useBackend must be used inside BackendProvider");
  return value;
}
