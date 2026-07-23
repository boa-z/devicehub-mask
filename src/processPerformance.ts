import type { ProcessPerformance } from "./types";

export type ProcessSort = "cpu" | "memory";

function metric(value: number | null, fallback = -1) {
  return value != null && Number.isFinite(value) ? value : fallback;
}

export function sortProcesses(processes: ProcessPerformance[], sort: ProcessSort) {
  return [...processes].sort((left, right) => {
    const difference = sort === "cpu"
      ? metric(right.cpu_percent) - metric(left.cpu_percent)
      : metric(right.memory_bytes) - metric(left.memory_bytes);
    return difference || left.name.localeCompare(right.name) || left.pid - right.pid;
  });
}
