import { invoke } from "@tauri-apps/api/core";
import { Update } from "@tauri-apps/plugin-updater";

export type UpdateChannel = "stable" | "nightly";

export type BuildInfo = {
  version: string;
  build: string;
  commit: string;
  updaterVersion: string;
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
  return invoke<BuildInfo>("build_info");
}

export async function checkUpdateChannel(channel: UpdateChannel) {
  const metadata = await invoke<UpdateMetadata | null>("check_for_update", { channel });
  return metadata ? new Update(metadata) : null;
}
