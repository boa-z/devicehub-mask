import type { RunningProcess } from "./types";

export function filterRunningProcesses(processes: RunningProcess[], query: string): RunningProcess[] {
  const normalized = query.trim().toLocaleLowerCase();
  if (!normalized) return processes;
  return processes.filter((process) => [
    process.name,
    process.app_name ?? "",
    String(process.pid),
  ].some((value) => value.toLocaleLowerCase().includes(normalized)));
}
