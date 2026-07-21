import { createContext, useContext } from "react";

export type UpdateContextValue = {
  automatic: boolean;
  checking: boolean;
  setAutomatic: (enabled: boolean) => void;
  checkNow: () => void;
};

export const UpdateContext = createContext<UpdateContextValue | null>(null);

export function useUpdates() {
  const context = useContext(UpdateContext);
  if (!context) throw new Error("useUpdates must be used within UpdateProvider");
  return context;
}
