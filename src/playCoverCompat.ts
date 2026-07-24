import { keyboardCodeForUsage } from "./control";
import {
  createMapping,
  defaultHardwareBindings,
  type DirectionPadMapping,
  type Mapping,
  type MouseCastSpellMapping,
  type Position,
  type Profile,
  type SingleTapMapping,
} from "./types";

type PlistObject = Record<string, unknown>;
type OrderedXmlNode = Record<string, unknown>;

export type PlayCoverImportResult = {
  profile: Profile;
  imported: number;
  skipped: number;
};

export type PlayCoverImportOptions = {
  invalidConfigMessage?: string;
  buttonLabel?: string;
  draggableLabel?: string;
  joystickLabel?: string;
};

export const MAX_PLAYMAP_BYTES = 1024 * 1024;
const MAX_PLIST_NODES = 10_000;
const MAX_PLIST_DEPTH = 16;
const MAX_MAPPING_MODELS = 1_024;
const APPLE_PLIST_DOCTYPE = /<!DOCTYPE\s+plist\s+PUBLIC\s+"-\/\/Apple\/\/DTD PLIST 1\.0\/\/EN"\s+"http:\/\/www\.apple\.com\/DTDs\/PropertyList-1\.0\.dtd"\s*>/i;

const object = (value: unknown): PlistObject | undefined => (
  value !== null && typeof value === "object" && !Array.isArray(value) ? value as PlistObject : undefined
);
const finite = (value: unknown): number | undefined => (
  typeof value === "number" && Number.isFinite(value) ? value : undefined
);
const integer = (value: unknown): number | undefined => {
  const result = finite(value);
  return result !== undefined && Number.isSafeInteger(result) ? result : undefined;
};
const clamp = (value: number, minimum: number, maximum: number) => Math.max(minimum, Math.min(maximum, value));

function xmlElementName(node: OrderedXmlNode): string | undefined {
  return Object.keys(node).find((key) => key !== ":@" && key !== "#text" && !key.startsWith("?"));
}

function xmlChildren(node: OrderedXmlNode, name: string): OrderedXmlNode[] {
  const value = node[name];
  return Array.isArray(value) ? value.filter((item): item is OrderedXmlNode => object(item) !== undefined) : [];
}

function xmlText(node: OrderedXmlNode, name: string): string {
  const text = xmlChildren(node, name).find((child) => typeof child["#text"] === "string")?.["#text"];
  return typeof text === "string" ? text : "";
}

function plistValue(node: OrderedXmlNode, depth: number, count: { value: number }): unknown {
  count.value += 1;
  if (count.value > MAX_PLIST_NODES || depth > MAX_PLIST_DEPTH) throw new Error("PlayCover plist exceeds safety limits");
  const name = xmlElementName(node);
  if (!name) throw new Error("PlayCover plist contains an invalid value");
  if (name === "string" || name === "key") return xmlText(node, name);
  if (name === "integer") {
    const value = Number(xmlText(node, name));
    if (!Number.isSafeInteger(value)) throw new Error("PlayCover plist contains an invalid integer");
    return value;
  }
  if (name === "real") {
    const value = Number(xmlText(node, name));
    if (!Number.isFinite(value)) throw new Error("PlayCover plist contains an invalid real number");
    return value;
  }
  if (name === "true") return true;
  if (name === "false") return false;
  if (name === "array") {
    return xmlChildren(node, name)
      .filter((child) => xmlElementName(child) !== undefined)
      .map((child) => plistValue(child, depth + 1, count));
  }
  if (name === "dict") {
    const children = xmlChildren(node, name).filter((child) => xmlElementName(child) !== undefined);
    const entries: [string, unknown][] = [];
    for (let index = 0; index < children.length; index += 2) {
      const keyNode = children[index];
      const valueNode = children[index + 1];
      if (xmlElementName(keyNode) !== "key" || !valueNode) throw new Error("PlayCover plist contains an invalid dictionary");
      const key = xmlText(keyNode, "key");
      if (!key || key === "__proto__" || key === "prototype" || key === "constructor") {
        throw new Error("PlayCover plist contains an unsafe dictionary key");
      }
      entries.push([key, plistValue(valueNode, depth + 1, count)]);
    }
    return Object.fromEntries(entries);
  }
  throw new Error(`PlayCover plist value ${name} is unsupported`);
}

export async function parsePlayCoverPlist(text: string, invalidConfigMessage = "Invalid PlayCover key mapping"): Promise<unknown> {
  if (new TextEncoder().encode(text).byteLength > MAX_PLAYMAP_BYTES) {
    throw new Error(invalidConfigMessage);
  }
  const xml = text.replace(APPLE_PLIST_DOCTYPE, "");
  if (/<!DOCTYPE|<!ENTITY/i.test(xml)) throw new Error(invalidConfigMessage);
  const { XMLParser, XMLValidator } = await import("fast-xml-parser");
  if (XMLValidator.validate(xml) !== true) throw new Error(invalidConfigMessage);
  const parser = new XMLParser({
    preserveOrder: true,
    ignoreAttributes: false,
    parseTagValue: false,
    processEntities: false,
    trimValues: true,
  });
  const document = parser.parse(xml) as OrderedXmlNode[];
  const plist = document.find((node) => xmlElementName(node) === "plist");
  const root = plist && xmlChildren(plist, "plist").find((node) => xmlElementName(node) !== undefined);
  if (!root || xmlElementName(root) !== "dict") throw new Error(invalidConfigMessage);
  try {
    return plistValue(root, 0, { value: 0 });
  } catch {
    throw new Error(invalidConfigMessage);
  }
}

function position(model: PlistObject): Position | undefined {
  const transform = object(model.transform);
  const x = finite(transform?.xCoord);
  const y = finite(transform?.yCoord);
  if (x === undefined || y === undefined || x < 0 || x > 1 || y < 0 || y > 1) return undefined;
  return { x, y };
}

function label(model: PlistObject, fallback: string, index: number): string {
  const keyName = typeof model.keyName === "string" ? model.keyName.replace(/\s+/g, " ").trim().slice(0, 64) : "";
  return keyName || `${fallback} ${index + 1}`;
}

function binding(model: PlistObject, key = "keyCode"): string | undefined {
  const usage = integer(model[key]);
  return usage === undefined ? undefined : keyboardCodeForUsage(usage);
}

function radiusPixels(model: PlistObject, frame: { width: number; height: number }): number {
  const size = finite(object(model.transform)?.size) ?? 10;
  return clamp(size / 200 * Math.min(frame.width, frame.height), 8, Math.min(frame.width, frame.height) * 0.4);
}

export function importPlayCoverConfig(
  value: unknown,
  profileName: string,
  frame: { width: number; height: number },
  options: PlayCoverImportOptions = {},
): PlayCoverImportResult {
  const config = object(value);
  if (!config || config.version !== "2.0.0" || frame.width <= 0 || frame.height <= 0) {
    throw new Error(options.invalidConfigMessage ?? "Invalid PlayCover key mapping");
  }
  const categories: unknown[] = [config.buttonModels, config.draggableButtonModels, config.joystickModel, config.mouseAreaModel];
  if (categories.some((category) => !Array.isArray(category))) {
    throw new Error(options.invalidConfigMessage ?? "Invalid PlayCover key mapping");
  }
  const totalModels = categories.reduce<number>((total, category) => total + (category as unknown[]).length, 0);
  if (totalModels > MAX_MAPPING_MODELS) throw new Error(options.invalidConfigMessage ?? "Invalid PlayCover key mapping");

  const mappings: Mapping[] = [];
  let skipped = 0;
  let pointer = 0;
  const nextPointer = () => pointer++ % 5;
  const addButtons = (models: unknown[], draggable: boolean) => {
    models.forEach((raw, index) => {
      const model = object(raw);
      const point = model && position(model);
      const key = model && binding(model);
      if (!model || !point || !key) {
        skipped += 1;
        return;
      }
      if (draggable) {
        const mapping = createMapping("MouseCastSpell", point, frame) as MouseCastSpellMapping;
        const radius = radiusPixels(model, frame);
        mappings.push({
          ...mapping,
          id: `playcover-drag-${index}`,
          note: label(model, options.draggableLabel ?? "Drag", index),
          bind: [key],
          pointer_id: nextPointer(),
          center: point,
          cast_radius: radius,
          drag_radius: radius,
        });
      } else {
        const mapping = createMapping("SingleTap", point, frame) as SingleTapMapping;
        mappings.push({
          ...mapping,
          id: `playcover-button-${index}`,
          note: label(model, options.buttonLabel ?? "Button", index),
          bind: [key],
          pointer_id: nextPointer(),
        });
      }
    });
  };
  addButtons(config.buttonModels as unknown[], false);
  addButtons(config.draggableButtonModels as unknown[], true);

  (config.joystickModel as unknown[]).forEach((raw, index) => {
    const model = object(raw);
    const point = model && position(model);
    const up = model && binding(model, "upKeyCode");
    const down = model && binding(model, "downKeyCode");
    const left = model && binding(model, "leftKeyCode");
    const right = model && binding(model, "rightKeyCode");
    if (!model || !point || !up || !down || !left || !right) {
      skipped += 1;
      return;
    }
    const mapping = createMapping("DirectionPad", point, frame) as DirectionPadMapping;
    const radius = radiusPixels(model, frame);
    mappings.push({
      ...mapping,
      id: `playcover-joystick-${index}`,
      note: label(model, options.joystickLabel ?? "Joystick", index),
      bind: { type: "Button", up: [up], down: [down], left: [left], right: [right] },
      pointer_id: nextPointer(),
      max_offset_x: radius,
      max_offset_y: radius,
    });
  });

  skipped += (config.mouseAreaModel as unknown[]).length;
  const bundleIdentifier = typeof config.bundleIdentifier === "string"
    && config.bundleIdentifier.length <= 255
    && /^[A-Za-z0-9.-]+$/.test(config.bundleIdentifier)
    ? config.bundleIdentifier
    : undefined;
  return {
    profile: {
      version: 1,
      name: profileName,
      mappings,
      hardwareBindings: { ...defaultHardwareBindings },
      bundleIdentifiers: bundleIdentifier ? [bundleIdentifier] : [],
    },
    imported: mappings.length,
    skipped,
  };
}
