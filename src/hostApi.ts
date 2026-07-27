import { isTauri } from "@tauri-apps/api/core";
import { requestBrowserHost } from "./backendConnection";

export type HostCapabilities = {
  always_on_top: boolean;
  system_fullscreen: boolean;
  native_file_dialogs: boolean;
  browser_file_transfer: boolean;
  device_audio: boolean;
  clipboard_sync: boolean;
  app_updates: boolean;
  open_host_directories: boolean;
  mutable_debug_logging: boolean;
};

export type BrowserHostStatus<TBuild> = {
  capabilities: HostCapabilities;
  build: TBuild;
};

export function runningInDesktopHost() {
  return isTauri();
}

const desktopCapabilities: HostCapabilities = {
  always_on_top: true,
  system_fullscreen: true,
  native_file_dialogs: true,
  browser_file_transfer: false,
  device_audio: true,
  clipboard_sync: true,
  app_updates: true,
  open_host_directories: true,
  mutable_debug_logging: true,
};

export function readHostCapabilities() {
  return runningInDesktopHost()
    ? Promise.resolve(desktopCapabilities)
    : browserHostJson<BrowserHostStatus<unknown>>("/api/host").then((status) => status.capabilities);
}

export async function browserHostJson<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await requestBrowserHost(path, init);
  if (!response.ok) throw new Error((await response.text()) || `${response.status} ${response.statusText}`);
  return response.json() as Promise<T>;
}

export async function browserHostRequest(path: string, init?: RequestInit): Promise<void> {
  const response = await requestBrowserHost(path, init);
  if (!response.ok) throw new Error((await response.text()) || `${response.status} ${response.statusText}`);
}
