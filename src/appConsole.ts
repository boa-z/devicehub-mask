import type { AppConsoleLine, AppConsoleSnapshot } from "./types";

const maxLocalLines = 1_000;

export function mergeConsoleLines(
  current: AppConsoleLine[],
  snapshot: AppConsoleSnapshot,
): AppConsoleLine[] {
  if (snapshot.reset) return snapshot.lines.slice(-maxLocalLines);
  const last = current.at(-1)?.sequence ?? 0;
  return [...current, ...snapshot.lines.filter((line) => line.sequence > last)].slice(-maxLocalLines);
}
