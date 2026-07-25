import { createContext, useContext } from "react";
import type { BuildInfo, UpdateChannel } from "./buildInfo";

export type UpdateContextValue = {
  automatic: boolean;
  checking: boolean;
  buildInfo: BuildInfo | null;
  channel: UpdateChannel;
  setAutomatic: (enabled: boolean) => void;
  setChannel: (channel: UpdateChannel) => void;
  checkNow: () => void;
};

export const UpdateContext = createContext<UpdateContextValue | null>(null);

export function useUpdates() {
  const context = useContext(UpdateContext);
  if (!context) throw new Error("useUpdates must be used within UpdateProvider");
  return context;
}
