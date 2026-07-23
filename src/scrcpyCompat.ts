import { createMapping, defaultHardwareBindings, scrcpyMappingTypes, type DpadMapping, type Mapping, type Profile, type ScrcpyMapping, type ScrcpyMappingType, type TouchMapping } from "./types";

type JsonObject = Record<string, unknown>;
export type ScrcpyImportResult = { profile: Profile; imported: number; skipped: number };
type ScrcpyImportOptions = { invalidConfigMessage?: string; dpadLabel?: string };

const object = (value: unknown): JsonObject | undefined => value !== null && typeof value === "object" && !Array.isArray(value) ? value as JsonObject : undefined;
const finite = (value: unknown): number | undefined => typeof value === "number" && Number.isFinite(value) ? value : undefined;
const clamp = (value: number) => Math.max(0, Math.min(1, value));
const keyIn = (key: unknown) => key === "SuperLeft" ? "MetaLeft" : key === "SuperRight" ? "MetaRight" : key;
const keyOut = (key: unknown) => key === "MetaLeft" ? "SuperLeft" : key === "MetaRight" ? "SuperRight" : key;
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

function importMapping(raw: JsonObject, width: number, height: number): ScrcpyMapping | undefined {
  if (typeof raw.type !== "string" || !scrcpyMappingTypes.includes(raw.type as never)) return undefined;
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
  return mapping as unknown as ScrcpyMapping;
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
  return { profile: { version: 1, name: profileName, hardwareBindings: { ...defaultHardwareBindings }, bundleIdentifiers: [], mappings }, imported: mappings.length, skipped };
}

function exportTouch(mapping: TouchMapping, width: number, height: number) {
  return { id: mapping.id, bind: [keyOut(mapping.key)], duration: 50, note: mapping.label, pointer_id: mapping.contactId, position: { x: mapping.x * width, y: mapping.y * height }, random_offset_x: 0, random_offset_y: 0, script_hooks: { before_script: "", after_script: "" }, sync: false, type: "SingleTap" };
}

function exportDpad(mapping: DpadMapping, width: number, height: number) {
  const offset = mapping.radius * Math.min(width, height);
  return { id: mapping.id, bind: { type: "Button", up: mapping.keys.up ? [keyOut(mapping.keys.up)] : [], down: mapping.keys.down ? [keyOut(mapping.keys.down)] : [], left: mapping.keys.left ? [keyOut(mapping.keys.left)] : [], right: mapping.keys.right ? [keyOut(mapping.keys.right)] : [] }, enable_randomization: false, initial_duration: 0, max_offset_x: offset, max_offset_y: offset, note: mapping.label, pointer_id: mapping.contactId, position: { x: mapping.x * width, y: mapping.y * height }, random_distance_max_scale: 1, random_distance_min_scale: 1, random_offset_x: 0, random_offset_y: 0, jitter_offset_x: 0, jitter_offset_y: 0, script_hooks: { before_script: "", after_script: "" }, type: "DirectionPad", up_boost_key: null, up_boost_scale: 2 };
}

function exportMapping(mapping: ScrcpyMapping, width: number, height: number) {
  const scale = (position: { x: number; y: number }) => ({ x: position.x * width, y: position.y * height });
  const value: JsonObject = { ...mapping, position: scale(mapping.position), bind: bindings("bind" in mapping ? mapping.bind : [], keyOut) };
  if (mapping.type === "MultipleTap") value.items = mapping.items.map((item) => ({ ...item, position: scale(item.position) }));
  if (mapping.type === "Swipe") value.positions = mapping.positions.map(scale);
  if (mapping.type === "MouseCastSpell") value.center = scale(mapping.center);
  if (mapping.type === "DirectionPad") value.bind = directionBinding(mapping.bind, keyOut);
  if (mapping.type === "PadCastSpell") value.pad_bind = directionBinding(mapping.pad_bind, keyOut);
  return value;
}

export function exportScrcpyMaskConfig(profile: Profile, width: number, height: number) {
  return { version: "0.0.1", original_size: { width, height }, mappings: profile.mappings.map((mapping) => mapping.type === "touch" ? exportTouch(mapping, width, height) : mapping.type === "dpad" ? exportDpad(mapping, width, height) : exportMapping(mapping, width, height)) };
}
