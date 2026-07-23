export const performanceHudItems = [
  "system_cpu",
  "graphics_fps",
  "process_count",
  "gpu_memory",
  "device_network_rx",
  "device_network_tx",
  "source_fps",
  "decoded_fps",
  "presented_fps",
  "bandwidth",
  "jpeg_encode",
  "frame_age",
] as const;

export const devicePerformanceHudItems = new Set<PerformanceHudItem>([
  "system_cpu",
  "graphics_fps",
  "process_count",
  "gpu_memory",
  "device_network_rx",
  "device_network_tx",
]);

export type PerformanceHudItem = (typeof performanceHudItems)[number];

export type PerformanceHudPreferences = {
  enabled: boolean;
  items: PerformanceHudItem[];
};

export const defaultPerformanceHudPreferences: PerformanceHudPreferences = {
  enabled: false,
  items: ["system_cpu", "graphics_fps", "presented_fps", "bandwidth"],
};

const storageKey = "devicehub-mask.performance-hud";
const itemSet = new Set<string>(performanceHudItems);

export function parsePerformanceHudPreferences(value: string | null): PerformanceHudPreferences {
  if (value === null) return { ...defaultPerformanceHudPreferences, items: [...defaultPerformanceHudPreferences.items] };
  try {
    const parsed: unknown = JSON.parse(value);
    if (parsed === null || typeof parsed !== "object") throw new Error("invalid preference");
    const candidate = parsed as { enabled?: unknown; items?: unknown };
    const items = Array.isArray(candidate.items)
      ? [...new Set(candidate.items.filter((item): item is PerformanceHudItem => typeof item === "string" && itemSet.has(item)))]
      : [...defaultPerformanceHudPreferences.items];
    return {
      enabled: typeof candidate.enabled === "boolean" ? candidate.enabled : defaultPerformanceHudPreferences.enabled,
      items,
    };
  } catch {
    return { ...defaultPerformanceHudPreferences, items: [...defaultPerformanceHudPreferences.items] };
  }
}

export function readPerformanceHudPreferences(): PerformanceHudPreferences {
  try {
    return parsePerformanceHudPreferences(localStorage.getItem(storageKey));
  } catch {
    return parsePerformanceHudPreferences(null);
  }
}

export function savePerformanceHudPreferences(preferences: PerformanceHudPreferences) {
  try {
    localStorage.setItem(storageKey, JSON.stringify(preferences));
  } catch {
    // Preferences remain active for this session when storage is unavailable.
  }
}
