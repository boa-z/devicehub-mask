import { describe, expect, it } from "vitest";
import { filterKeymapCatalogEntries, keymapCatalogMatchLevel } from "./keymapCatalog";
import type { KeymapCatalogEntry } from "./types";

const entry: KeymapCatalogEntry = {
  id: "entry-1",
  slug: "example-game-landscape",
  title: "Example Game",
  description: "",
  author: "",
  updated_at: "",
  profile: {
    format: "devicehub-mask",
    format_version: 2,
    url: "profiles/entry-1/profile.json",
    sha256: "a".repeat(64),
    bytes: 1,
  },
  match: {
    bundle_ids: ["com.example.game"],
    stream_resolution: { width: 2796, height: 1290 },
    orientation: "landscape_left",
    product_types: ["iPhone16,1"],
  },
};

const device = {
  product_type: "iPhone16,1",
  frame_size: { width: 2796, height: 1290 },
  orientation: "landscape_left" as const,
};

describe("keymap catalog filters", () => {
  it("recognizes an exact app and device match", () => {
    expect(keymapCatalogMatchLevel(entry, "com.example.game", device)).toBe("exact");
  });

  it("filters entries by the selected device target", () => {
    const wrongDevice = { ...device, frame_size: { width: 1290, height: 2796 } };
    expect(filterKeymapCatalogEntries([entry], "com.example.game", device)).toEqual([entry]);
    expect(filterKeymapCatalogEntries([entry], "com.example.game", wrongDevice)).toEqual([]);
    expect(filterKeymapCatalogEntries([entry], "com.example.game", null)).toEqual([entry]);
  });

  it("searches catalog titles, app bundle IDs, and device targets", () => {
    expect(filterKeymapCatalogEntries([entry], null, null, "example game")).toEqual([entry]);
    expect(filterKeymapCatalogEntries([entry], null, null, "com.example.game")).toEqual([entry]);
    expect(filterKeymapCatalogEntries([entry], null, null, "iPhone16,1")).toEqual([entry]);
    expect(filterKeymapCatalogEntries([entry], null, null, "2796x1290")).toEqual([entry]);
    expect(filterKeymapCatalogEntries([entry], null, null, "not-present")).toEqual([]);
  });
});
