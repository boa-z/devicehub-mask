import { keyboardUsage } from "./control";
import { defaultHardwareBindings, type DpadMapping, type Mapping, type Profile, type TouchMapping } from "./types";

type JsonObject = Record<string, unknown>;

export type ScrcpyImportResult = {
  profile: Profile;
  imported: number;
  skipped: number;
};

type ScrcpyImportOptions = {
  invalidConfigMessage?: string;
  dpadLabel?: string;
};

function object(value: unknown): JsonObject | undefined {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as JsonObject
    : undefined;
}

function finite(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function normalizeKey(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined;
  const code = value === "SuperLeft" ? "MetaLeft" : value === "SuperRight" ? "MetaRight" : value;
  return keyboardUsage(code) === undefined ? undefined : code;
}

function singleBinding(value: unknown): string | undefined {
  if (!Array.isArray(value)) return undefined;
  const keys = value.map(normalizeKey).filter((key): key is string => key !== undefined);
  return keys.length === 1 && keys.length === value.length ? keys[0] : undefined;
}

function position(value: unknown, width: number, height: number) {
  const point = object(value);
  const x = finite(point?.x);
  const y = finite(point?.y);
  if (x === undefined || y === undefined) return undefined;
  return {
    x: Math.max(0, Math.min(1, x / width)),
    y: Math.max(0, Math.min(1, y / height)),
  };
}

function contactId(value: unknown, used: Set<number>) {
  const requested = finite(value);
  const candidate = requested !== undefined && Number.isInteger(requested) && requested >= 0 && requested < 5
    ? requested
    : undefined;
  const identity = candidate !== undefined && !used.has(candidate)
    ? candidate
    : [0, 1, 2, 3, 4].find((id) => !used.has(id));
  if (identity !== undefined) used.add(identity);
  return identity;
}

export function importScrcpyMaskConfig(value: unknown, profileName: string, options: ScrcpyImportOptions = {}): ScrcpyImportResult {
  const config = object(value);
  const size = object(config?.original_size);
  const width = finite(size?.width);
  const height = finite(size?.height);
  if (!config || !Array.isArray(config.mappings) || !width || !height || width <= 0 || height <= 0) {
    throw new Error(options.invalidConfigMessage ?? "Invalid scrcpy-mask mapping configuration");
  }

  const mappings: Mapping[] = [];
  const used = new Set<number>();
  let skipped = 0;
  for (const raw of config.mappings) {
    const item = object(raw);
    if (!item || mappings.length >= 5) {
      skipped += 1;
      continue;
    }
    const identity = contactId(item.pointer_id, used);
    const point = position(item.position, width, height);
    if (identity === undefined || !point) {
      skipped += 1;
      continue;
    }
    const id = typeof item.id === "string" && item.id ? `scrcpy-${item.id}` : crypto.randomUUID();
    const note = typeof item.note === "string" && item.note ? item.note : undefined;

    if (item.type === "SingleTap") {
      const key = singleBinding(item.bind);
      if (!key) {
        used.delete(identity);
        skipped += 1;
        continue;
      }
      mappings.push({ id, type: "touch", label: note ?? key, contactId: identity, ...point, key });
      continue;
    }

    if (item.type === "DirectionPad") {
      const binding = object(item.bind);
      if (binding?.type !== "Button") {
        used.delete(identity);
        skipped += 1;
        continue;
      }
      const keys = {
        up: singleBinding(binding.up) ?? "",
        down: singleBinding(binding.down) ?? "",
        left: singleBinding(binding.left) ?? "",
        right: singleBinding(binding.right) ?? "",
      };
      if (!Object.values(keys).some(Boolean)) {
        used.delete(identity);
        skipped += 1;
        continue;
      }
      const maxOffset = Math.max(finite(item.max_offset_x) ?? width * 0.1, finite(item.max_offset_y) ?? height * 0.1);
      mappings.push({
        id,
        type: "dpad",
        label: note ?? options.dpadLabel ?? "Direction pad",
        contactId: identity,
        ...point,
        radius: Math.max(0.01, Math.min(0.5, maxOffset / Math.min(width, height))),
        keys,
      });
      continue;
    }

    used.delete(identity);
    skipped += 1;
  }

  return {
    profile: {
      version: 1,
      name: profileName,
      hardwareBindings: { ...defaultHardwareBindings },
      mappings,
    },
    imported: mappings.length,
    skipped,
  };
}

function scrcpyKey(code: string) {
  return code === "MetaLeft" ? "SuperLeft" : code === "MetaRight" ? "SuperRight" : code;
}

function exportTouch(mapping: TouchMapping, width: number, height: number) {
  return {
    id: mapping.id,
    bind: [scrcpyKey(mapping.key)],
    duration: 50,
    note: mapping.label,
    pointer_id: mapping.contactId,
    position: { x: mapping.x * width, y: mapping.y * height },
    random_offset_x: 0,
    random_offset_y: 0,
    script_hooks: { before_script: "", after_script: "" },
    sync: false,
    type: "SingleTap",
  };
}

function exportDpad(mapping: DpadMapping, width: number, height: number) {
  const offset = mapping.radius * Math.min(width, height);
  return {
    id: mapping.id,
    bind: {
      type: "Button",
      up: mapping.keys.up ? [scrcpyKey(mapping.keys.up)] : [],
      down: mapping.keys.down ? [scrcpyKey(mapping.keys.down)] : [],
      left: mapping.keys.left ? [scrcpyKey(mapping.keys.left)] : [],
      right: mapping.keys.right ? [scrcpyKey(mapping.keys.right)] : [],
    },
    enable_randomization: false,
    initial_duration: 0,
    max_offset_x: offset,
    max_offset_y: offset,
    note: mapping.label,
    pointer_id: mapping.contactId,
    position: { x: mapping.x * width, y: mapping.y * height },
    random_distance_max_scale: 1,
    random_distance_min_scale: 1,
    random_offset_x: 0,
    random_offset_y: 0,
    jitter_offset_x: 0,
    jitter_offset_y: 0,
    script_hooks: { before_script: "", after_script: "" },
    type: "DirectionPad",
    up_boost_key: null,
    up_boost_scale: 2,
  };
}

export function exportScrcpyMaskConfig(profile: Profile, width: number, height: number) {
  return {
    version: "0.0.1",
    original_size: { width, height },
    mappings: profile.mappings.map((mapping) => mapping.type === "touch"
      ? exportTouch(mapping, width, height)
      : exportDpad(mapping, width, height)),
  };
}
