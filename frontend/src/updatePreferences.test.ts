import { describe, expect, it } from "vitest";
import { parseAutomaticUpdatePreference } from "./updatePreferences";

describe("update preferences", () => {
  it("enables automatic checks by default", () => {
    expect(parseAutomaticUpdatePreference(null)).toBe(true);
    expect(parseAutomaticUpdatePreference("true")).toBe(true);
  });

  it("honors an explicitly disabled preference", () => {
    expect(parseAutomaticUpdatePreference("false")).toBe(false);
  });
});
