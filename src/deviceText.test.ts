import { describe, expect, it } from "vitest";
import { maxPasteTextCharacters, truncatePasteText } from "./deviceText";

describe("device text", () => {
  it("limits text by Unicode code points rather than UTF-16 code units", () => {
    expect(Array.from(truncatePasteText("😀".repeat(maxPasteTextCharacters + 1)))).toHaveLength(
      maxPasteTextCharacters,
    );
    expect(truncatePasteText("界".repeat(maxPasteTextCharacters + 1))).toBe(
      "界".repeat(maxPasteTextCharacters),
    );
  });
});
