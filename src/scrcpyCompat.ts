import { createMapping, defaultHardwareBindings, keyMappingTypes, type KeyMapping, type Mapping, type Profile, type ScrcpyMappingType } from "./types";

type JsonObject = Record<string, unknown>;
export type ScrcpyImportResult = { profile: Profile; imported: number; skipped: number };
type ScrcpyImportOptions = { invalidConfigMessage?: string; dpadLabel?: string };

const object = (value: unknown): JsonObject | undefined => value !== null && typeof value === "object" && !Array.isArray(value) ? value as JsonObject : undefined;
const finite = (value: unknown): number | undefined => typeof value === "number" && Number.isFinite(value) ? value : undefined;
const clamp = (value: number) => Math.max(0, Math.min(1, value));
const keyIn = (key: unknown) => key === "SuperLeft" ? "MetaLeft" : key === "SuperRight" ? "MetaRight" : key;
const bindings = (value: unknown, convert: (key: unknown) => unknown): unknown => Array.isArray(value) ? value.map(convert) : value;

function point(value: unknown, width: number, height: number) {
  const raw = object(value);
  const x = finite(raw?.x);
  const y = finite(raw?.y);
  return x === undefined || y === undefined ? undefined : { x: clamp(x / width), y: clamp(y / height) };
}

function directionBinding(value: unknown, convert: (key: unknown) => unknown) {
  const raw = object(value);
  if (!raw) return value;
  if (raw.type === "Button") return { ...raw, up: bindings(raw.up, convert), down: bindings(raw.down, convert), left: bindings(raw.left, convert), right: bindings(raw.right, convert) };
  return raw;
}

function importMapping(raw: JsonObject, width: number, height: number): KeyMapping | undefined {
  if (typeof raw.type !== "string" || raw.type === "Press" || !keyMappingTypes.includes(raw.type as never)) return undefined;
  const position = point(raw.position, width, height);
  if (!position) return undefined;
  const defaults = createMapping(raw.type as ScrcpyMappingType, position, { width, height });
  const mapping: JsonObject = {
    ...defaults,
    ...raw,
    id: typeof raw.id === "string" && raw.id ? raw.id : crypto.randomUUID(),
    note: typeof raw.note === "string" ? raw.note : "",
    position,
    bind: bindings(raw.bind, keyIn),
  };
  if (Array.isArray(raw.positions)) mapping.positions = raw.positions.map((item) => point(item, width, height)).filter(Boolean);
  if (Array.isArray(raw.items)) mapping.items = raw.items.map((item) => {
    const value = object(item);
    const itemPosition = point(value?.position, width, height);
    return value && itemPosition ? { ...value, position: itemPosition } : undefined;
  }).filter(Boolean);
  if (raw.center) mapping.center = point(raw.center, width, height) ?? position;
  if (raw.type === "DirectionPad") mapping.bind = directionBinding(raw.bind, keyIn);
  if (raw.type === "PadCastSpell") mapping.pad_bind = directionBinding(raw.pad_bind, keyIn);
  return mapping as unknown as KeyMapping;
}

export function importScrcpyMaskConfig(value: unknown, profileName: string, options: ScrcpyImportOptions = {}): ScrcpyImportResult {
  const config = object(value);
  const size = object(config?.original_size);
  const width = finite(size?.width);
  const height = finite(size?.height);
  if (!config || !Array.isArray(config.mappings) || !width || !height || width <= 0 || height <= 0) throw new Error(options.invalidConfigMessage ?? "Invalid scrcpy-mask mapping configuration");
  const mappings: Mapping[] = [];
  let skipped = 0;
  for (const value of config.mappings) {
    const raw = object(value);
    const mapping = raw && importMapping(raw, width, height);
    if (mapping) mappings.push(mapping); else skipped += 1;
  }
  return { profile: { version: 2, name: profileName, hardwareBindings: { ...defaultHardwareBindings }, bundleIdentifiers: [], targetResolution: null, mappings }, imported: mappings.length, skipped };
}
