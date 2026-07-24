import { importPlayCoverConfig, MAX_PLAYMAP_BYTES, parsePlayCoverPlist } from "./playCoverCompat";
import { importScrcpyMaskConfig } from "./scrcpyCompat";
import { defaultHardwareBindings, type HardwareBindings, type Profile } from "./types";

export type MappingImportSourceId = "devicehub-mask" | "scrcpy-mask" | "playcover";

export type MappingImportSource = {
  id: MappingImportSourceId;
  extensions: readonly string[];
  accept: string;
  maxBytes: number;
};

export type MappingImportResult = {
  profile: Profile;
  imported: number;
  skipped: number;
};

export type MappingImportFile = {
  name: string;
  size: number;
  text: () => Promise<string>;
};

export type MappingImportContext = {
  profileName: string;
  frameSize: { width: number; height: number };
  invalidMessages: Record<MappingImportSourceId, string>;
  playCoverLabels: {
    button: string;
    draggable: string;
    joystick: string;
  };
  dpadLabel: string;
};

const MAX_JSON_IMPORT_BYTES = 4 * 1024 * 1024;

function object(value: unknown): Record<string, unknown> | undefined {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : undefined;
}

function nativeProfile(value: unknown, name: string, invalidMessage: string): MappingImportResult {
  const input = object(value);
  if (input?.version !== 1 || !Array.isArray(input.mappings)) throw new Error(invalidMessage);
  const hardware = object(input.hardwareBindings) ?? {};
  const bundleIdentifiers = Array.isArray(input.bundleIdentifiers)
    ? input.bundleIdentifiers.filter((item): item is string => typeof item === "string")
    : [];
  const profile: Profile = {
    version: 1,
    name,
    mappings: input.mappings as Profile["mappings"],
    hardwareBindings: { ...defaultHardwareBindings, ...hardware } as HardwareBindings,
    bundleIdentifiers,
  };
  return { profile, imported: profile.mappings.length, skipped: 0 };
}

function parseJson(text: string, invalidMessage: string) {
  try {
    return JSON.parse(text) as unknown;
  } catch {
    throw new Error(invalidMessage);
  }
}

type MappingImportAdapter = MappingImportSource & {
  importText: (text: string, context: MappingImportContext) => Promise<MappingImportResult>;
};

const mappingImportAdapters: readonly MappingImportAdapter[] = [
  {
    id: "devicehub-mask",
    extensions: [".json"],
    accept: "application/json,.json",
    maxBytes: MAX_JSON_IMPORT_BYTES,
    importText: async (text, context) => nativeProfile(
      parseJson(text, context.invalidMessages["devicehub-mask"]),
      context.profileName,
      context.invalidMessages["devicehub-mask"],
    ),
  },
  {
    id: "scrcpy-mask",
    extensions: [".scrcpy-mask.json", ".json"],
    accept: "application/json,.json",
    maxBytes: MAX_JSON_IMPORT_BYTES,
    importText: async (text, context) => importScrcpyMaskConfig(
      parseJson(text, context.invalidMessages["scrcpy-mask"]),
      context.profileName,
      {
        invalidConfigMessage: context.invalidMessages["scrcpy-mask"],
        dpadLabel: context.dpadLabel,
      },
    ),
  },
  {
    id: "playcover",
    extensions: [".playmap"],
    accept: "application/xml,text/xml,.playmap",
    maxBytes: MAX_PLAYMAP_BYTES,
    importText: async (text, context) => importPlayCoverConfig(
      await parsePlayCoverPlist(text, context.invalidMessages.playcover),
      context.profileName,
      context.frameSize,
      {
        invalidConfigMessage: context.invalidMessages.playcover,
        buttonLabel: context.playCoverLabels.button,
        draggableLabel: context.playCoverLabels.draggable,
        joystickLabel: context.playCoverLabels.joystick,
      },
    ),
  },
];

export const mappingImportSources: readonly MappingImportSource[] = mappingImportAdapters;

export function mappingImportSource(id: MappingImportSourceId) {
  const source = mappingImportSources.find((candidate) => candidate.id === id);
  if (!source) throw new Error(`Unsupported mapping import source: ${id}`);
  return source;
}

export function uniqueImportedProfileName(fileName: string, existing: readonly string[]) {
  const importedName = fileName
    .replace(/(?:\.scrcpy-mask)?\.(?:json|playmap)$/i, "")
    .replace(/[^A-Za-z0-9_-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 80);
  const baseName = importedName || `import-${Date.now()}`;
  let name = baseName;
  let suffix = 2;
  while (existing.includes(name)) {
    const ending = `-import-${suffix}`;
    name = `${baseName.slice(0, 80 - ending.length)}${ending}`;
    suffix += 1;
  }
  return name;
}

export async function importMappingFile(
  sourceId: MappingImportSourceId,
  file: MappingImportFile,
  context: MappingImportContext,
): Promise<MappingImportResult> {
  const source = mappingImportSource(sourceId);
  const invalidMessage = context.invalidMessages[sourceId];
  if (file.size <= 0 || file.size > source.maxBytes) throw new Error(invalidMessage);
  const text = await file.text();
  const adapter = mappingImportAdapters.find((candidate) => candidate.id === sourceId);
  if (!adapter) throw new Error(`Unsupported mapping import source: ${sourceId}`);
  return adapter.importText(text, context);
}
