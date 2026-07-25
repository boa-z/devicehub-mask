import { describe, expect, it } from "vitest";
import { parseAutomaticUpdatePreference, parseUpdateChannelPreference } from "./updatePreferences";

describe("update preferences", () => {
  it("enables automatic checks by default", () => {
    expect(parseAutomaticUpdatePreference(null)).toBe(true);
    expect(parseAutomaticUpdatePreference("true")).toBe(true);
  });

  it("honors an explicitly disabled preference", () => {
    expect(parseAutomaticUpdatePreference("false")).toBe(false);
  });

  it("accepts only supported update channels", () => {
    expect(parseUpdateChannelPreference("stable")).toBe("stable");
    expect(parseUpdateChannelPreference("nightly")).toBe("nightly");
    expect(parseUpdateChannelPreference("custom")).toBeNull();
    expect(parseUpdateChannelPreference(null)).toBeNull();
  });
});
