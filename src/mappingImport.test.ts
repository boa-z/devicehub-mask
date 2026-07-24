import { describe, expect, it } from "vitest";
import {
  importMappingFile,
  mappingImportSources,
  uniqueImportedProfileName,
  type MappingImportContext,
  type MappingImportFile,
} from "./mappingImport";
import { createMapping } from "./types";

const context: MappingImportContext = {
  profileName: "imported",
  frameSize: { width: 1000, height: 500 },
  invalidMessages: {
    "devicehub-mask": "invalid native",
    "scrcpy-mask": "invalid scrcpy",
    playcover: "invalid playcover",
  },
  playCoverLabels: { button: "Button", draggable: "Drag", joystick: "Joystick" },
  dpadLabel: "Direction pad",
};

function file(name: string, value: unknown): MappingImportFile {
  const text = JSON.stringify(value);
  return { name, size: new TextEncoder().encode(text).byteLength, text: async () => text };
}

describe("mapping import", () => {
  it("registers stable metadata for every supported source", () => {
    expect(mappingImportSources.map((source) => source.id)).toEqual(["devicehub-mask", "scrcpy-mask", "playcover"]);
    expect(new Set(mappingImportSources.map((source) => source.id)).size).toBe(mappingImportSources.length);
  });

  it("imports native profiles through the shared result contract", async () => {
    const mapping = createMapping("SingleTap", { x: 0.5, y: 0.5 });
    const result = await importMappingFile("devicehub-mask", file("native.json", {
      version: 1,
      mappings: [mapping],
      hardwareBindings: { home: "KeyH" },
      bundleIdentifiers: ["com.example.game"],
    }), context);
    expect(result).toMatchObject({ imported: 1, skipped: 0 });
    expect(result.profile).toMatchObject({ name: "imported", hardwareBindings: { home: "KeyH" }, bundleIdentifiers: ["com.example.game"] });
  });

  it("routes scrcpy-mask JSON only through the selected adapter", async () => {
    const value = { version: "0.0.1", original_size: { width: 1000, height: 500 }, mappings: [] };
    await expect(importMappingFile("scrcpy-mask", file("game.json", value), context)).resolves.toMatchObject({ imported: 0 });
    await expect(importMappingFile("devicehub-mask", file("game.json", value), context)).rejects.toThrow("invalid native");
  });

  it("creates bounded, conflict-free profile names", () => {
    expect(uniqueImportedProfileName("My Game.scrcpy-mask.json", ["My-Game", "My-Game-import-2"]))
      .toBe("My-Game-import-3");
    const longName = "a".repeat(80);
    const unique = uniqueImportedProfileName(`${longName}.json`, [longName]);
    expect(unique).toHaveLength(80);
    expect(unique).toMatch(/-import-2$/);
  });
});
