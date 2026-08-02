import { createMapping, defaultHardwareBindings, keyMappingTypes, type KeyMapping, type Mapping, type Profile, type ScrcpyMappingType } from "./types";

type JsonObject = Record<string, unknown>;
export type ScrcpyImportResult = { profile: Profile; imported: number; skipped: number };
type ScrcpyImportOptions = { invalidConfigMessage?: string; dpadLabel?: string };

const object = (value: unknown): JsonObject | undefined => value !== null && typeof value === "object" && !Array.isArray(value) ? value as JsonObject : undefined;
const finite = (value: unknown): number | undefined => typeof value === "number" && Number.isFinite(value) ? value : undefined;
const integer = (value: unknown): number | undefined => finite(value) !== undefined && Number.isInteger(value) ? value as number : undefined;

const keyAliases: Record<string, string> = {
  SuperLeft: "MetaLeft",
  SuperRight: "MetaRight",
  "M-Left": "MouseLeft",
  "M-Middle": "MouseMiddle",
  "M-Right": "MouseRight",
  "M-Back": "MouseBack",
  "M-Forward": "MouseForward",
};

function keyIn(value: unknown): unknown {
  if (typeof value !== "string") return value;
  if (keyAliases[value]) return keyAliases[value];
  if (value.startsWith("M-Other-")) return `MouseOther${value.slice("M-Other-".length)}`;
  if (value.startsWith("G-Other-")) return `GamepadOther${value.slice("G-Other-".length)}`;
  if (value.startsWith("G-")) return `Gamepad${value.slice(2)}`;
  return value;
}

function bindingArray(value: unknown, convert: (key: unknown) => unknown): string[] | undefined {
  if (!Array.isArray(value)) return undefined;
  const result = value.map(convert);
  return result.every((key): key is string => typeof key === "string" && /^[A-Za-z0-9]+$/.test(key) && key.length <= 64)
    ? result
    : undefined;
}

function gamepadAxis(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined;
  if (/^(LeftStickX|LeftStickY|RightStickX|RightStickY|LeftZ|RightZ)$/.test(value)) return value;
  const other = /^Other-([0-9]+)$/.exec(value);
  return other && Number(other[1]) < 32 ? value : undefined;
}

function point(value: unknown, width: number, height: number) {
  const raw = object(value);
  const x = finite(raw?.x);
  const y = finite(raw?.y);
  return x === undefined || y === undefined || x < 0 || x > width || y < 0 || y > height
    ? undefined
    : { x: x / width, y: y / height };
}

function directionBinding(value: unknown, convert: (key: unknown) => unknown) {
  const raw = object(value);
  if (!raw || (raw.type !== "Button" && raw.type !== "JoyStick")) return undefined;
  if (raw.type === "JoyStick") {
    const x = gamepadAxis(raw.x);
    const y = gamepadAxis(raw.y);
    return x && y ? { ...raw, x, y } : undefined;
  }
  const up = bindingArray(raw.up, convert);
  const down = bindingArray(raw.down, convert);
  const left = bindingArray(raw.left, convert);
  const right = bindingArray(raw.right, convert);
  return up && down && left && right ? { ...raw, up, down, left, right } : undefined;
}

function importMapping(raw: JsonObject, width: number, height: number): KeyMapping | undefined {
  if (typeof raw.type !== "string" || raw.type === "Press" || !keyMappingTypes.includes(raw.type as never)) return undefined;
  const position = point(raw.position, width, height);
  if (!position) return undefined;
  const defaults = createMapping(raw.type as ScrcpyMappingType, position, { width, height });
  const defaultPointerId = "pointer_id" in defaults ? defaults.pointer_id : undefined;
  const pointerId = integer(raw.pointer_id ?? defaultPointerId);
  if (raw.pointer_id !== undefined && (pointerId === undefined || pointerId < 0 || pointerId > 4)) return undefined;
  const mapping: JsonObject = {
    ...defaults,
    ...raw,
    id: typeof raw.id === "string" && raw.id ? raw.id : crypto.randomUUID(),
    note: typeof raw.note === "string" ? raw.note : "",
    position,
  };
  if (raw.bind !== undefined) {
    const bind = raw.type === "DirectionPad" ? directionBinding(raw.bind, keyIn) : bindingArray(raw.bind, keyIn);
    if (!bind) return undefined;
    mapping.bind = bind;
  }
  if (raw.up_boost_key !== undefined) {
    const upBoost = raw.up_boost_key === null ? null : bindingArray(raw.up_boost_key, keyIn);
    if (upBoost === undefined) return undefined;
    mapping.up_boost_key = upBoost;
  }
  if (raw.positions !== undefined) {
    if (!Array.isArray(raw.positions) || raw.positions.length < 2 || raw.positions.length > 32) return undefined;
    const positions = raw.positions.map((item) => point(item, width, height));
    if (positions.some((item) => !item)) return undefined;
    mapping.positions = positions;
  }
  if (raw.items !== undefined) {
    if (!Array.isArray(raw.items) || raw.items.length === 0 || raw.items.length > 32) return undefined;
    const items = raw.items.map((item) => {
      const value = object(item);
      const itemPosition = point(value?.position, width, height);
      const duration = finite(value?.duration);
      const wait = finite(value?.wait);
      return value && itemPosition && duration !== undefined && duration >= 1 && duration <= 60_000
        && wait !== undefined && wait >= 0 && wait <= 60_000
        ? { ...value, position: itemPosition }
        : undefined;
    });
    if (items.some((item) => !item)) return undefined;
    mapping.items = items;
  }
  if (raw.center !== undefined) {
    const center = point(raw.center, width, height);
    if (!center) return undefined;
    mapping.center = center;
  }
  if (raw.type === "DirectionPad") {
    const bind = directionBinding(raw.bind, keyIn);
    if (!bind) return undefined;
    mapping.bind = bind;
  }
  if (raw.type === "PadCastSpell") {
    const bind = directionBinding(raw.pad_bind, keyIn);
    if (!bind) return undefined;
    mapping.pad_bind = bind;
  }
  if (raw.type === "Fps") {
    const touchMode = object(raw.touch_mode);
    if (!touchMode || (touchMode.type !== "single" && touchMode.type !== "dual")) return undefined;
    const anotherPointerId = integer(touchMode.another_pointer_id);
    if (touchMode.type === "dual"
      && (anotherPointerId === undefined || anotherPointerId < 0 || anotherPointerId > 4 || anotherPointerId === pointerId)) return undefined;
  }
  return mapping as unknown as KeyMapping;
}

export function importScrcpyMaskConfig(value: unknown, profileName: string, options: ScrcpyImportOptions = {}): ScrcpyImportResult {
  const config = object(value);
  const size = object(config?.original_size);
  const width = integer(size?.width);
  const height = integer(size?.height);
  if (!config || config.version !== "0.0.1" || !Array.isArray(config.mappings) || !width || !height || width <= 0 || height <= 0 || width > 16_384 || height > 16_384) throw new Error(options.invalidConfigMessage ?? "Invalid scrcpy-mask mapping configuration");
  const mappings: Mapping[] = [];
  const ids = new Set<string>();
  let skipped = 0;
  for (const value of config.mappings) {
    if (mappings.length >= 512) {
      skipped += 1;
      continue;
    }
    const raw = object(value);
    const mapping = raw && importMapping(raw, width, height);
    if (mapping && !ids.has(mapping.id)) {
      ids.add(mapping.id);
      mappings.push(mapping);
    } else skipped += 1;
  }
  return { profile: { version: 2, name: profileName, hardwareBindings: { ...defaultHardwareBindings }, bundleIdentifiers: [], targetResolution: null, mappings }, imported: mappings.length, skipped };
}
