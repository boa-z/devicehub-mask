import { invoke } from "@tauri-apps/api/core";
import { Update } from "@tauri-apps/plugin-updater";
import { browserHostJson, runningInDesktopHost, type BrowserHostStatus } from "./hostApi";

export type UpdateChannel = "stable" | "nightly";

export type BuildInfo = {
  version: string;
  build: string;
  commit: string;
  updateChannel: UpdateChannel;
};

type UpdateMetadata = {
  rid: number;
  currentVersion: string;
  version: string;
  date?: string;
  body?: string;
  rawJson: Record<string, unknown>;
};

export function readBuildInfo() {
  return runningInDesktopHost()
    ? invoke<BuildInfo>("build_info")
    : browserHostJson<BrowserHostStatus<BuildInfo>>("/api/host").then((status) => status.build);
}

export async function checkUpdateChannel(channel: UpdateChannel) {
  const metadata = await invoke<UpdateMetadata | null>("check_for_update", { channel });
  return metadata ? new Update(metadata) : null;
}
