import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

export type HostWindow = {
  isAlwaysOnTop: () => Promise<boolean>;
  isFullscreen: () => Promise<boolean>;
  setAlwaysOnTop: (enabled: boolean) => Promise<void>;
  setFullscreen: (enabled: boolean) => Promise<void>;
  onResized: (listener: () => void) => Promise<() => void>;
};

export function currentHostWindow(): HostWindow {
  if (isTauri()) return getCurrentWindow();
  return {
    isAlwaysOnTop: async () => false,
    isFullscreen: async () => document.fullscreenElement !== null,
    setAlwaysOnTop: async () => {
      throw new Error("Always-on-top is available only in the desktop application");
    },
    setFullscreen: async (enabled) => {
      if (enabled && document.fullscreenElement === null) {
        await document.documentElement.requestFullscreen();
      } else if (!enabled && document.fullscreenElement !== null) {
        await document.exitFullscreen();
      }
    },
    onResized: async (listener) => {
      window.addEventListener("resize", listener);
      document.addEventListener("fullscreenchange", listener);
      return () => {
        window.removeEventListener("resize", listener);
        document.removeEventListener("fullscreenchange", listener);
      };
    },
  };
}
