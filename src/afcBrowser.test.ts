import { describe, expect, it } from "vitest";
import { normalizeAfcPath, sortAfcEntries } from "./afcBrowser";
import type { DeviceFileEntry } from "./types";

const entries: DeviceFileEntry[] = [
  { name: "photo10.jpg", path: "/photo10.jpg", kind: "file", size_bytes: 10, modified: "2026-07-20T00:00:00Z" },
  { name: "DCIM", path: "/DCIM", kind: "directory", size_bytes: 0, modified: "2026-07-22T00:00:00Z" },
  { name: "photo2.jpg", path: "/photo2.jpg", kind: "file", size_bytes: 20, modified: "2026-07-21T00:00:00Z" },
  { name: "linked", path: "/linked", kind: "other", size_bytes: 0, modified: "invalid" },
];

describe("AFC browser", () => {
  it("normalizes paths with the backend byte and component rules", () => {
    expect(normalizeAfcPath("")).toBe("/");
    expect(normalizeAfcPath("DCIM//100APPLE")).toBe("/DCIM/100APPLE");
    expect(normalizeAfcPath("/DCIM/../Downloads")).toBeNull();
    expect(normalizeAfcPath("/DCIM\\100APPLE")).toBeNull();
    expect(normalizeAfcPath(`/DCIM/${"界".repeat(86)}`)).toBeNull();
    expect(normalizeAfcPath(`/${"a".repeat(1_024)}`)).toBeNull();
  });

  it("keeps directories first while applying natural name order", () => {
    expect(sortAfcEntries(entries, "name", "ascending", "en-US").map((entry) => entry.name))
      .toEqual(["DCIM", "photo2.jpg", "photo10.jpg", "linked"]);
    expect(sortAfcEntries(entries, "name", "descending", "en-US").map((entry) => entry.name))
      .toEqual(["DCIM", "photo10.jpg", "photo2.jpg", "linked"]);
  });

  it("sorts size and modification time without mutating the source", () => {
    const original = entries.map((entry) => entry.name);
    expect(sortAfcEntries(entries, "size", "descending", "en-US").map((entry) => entry.name))
      .toEqual(["DCIM", "photo2.jpg", "photo10.jpg", "linked"]);
    expect(sortAfcEntries(entries, "modified", "ascending", "en-US").map((entry) => entry.name))
      .toEqual(["DCIM", "photo10.jpg", "photo2.jpg", "linked"]);
    expect(entries.map((entry) => entry.name)).toEqual(original);
  });
});
