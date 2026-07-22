import { describe, expect, it } from "vitest";
import { validLocationCoordinates } from "./location";

describe("location coordinates", () => {
  it("accepts valid coordinate boundaries", () => {
    expect(validLocationCoordinates(-90, -180)).toBe(true);
    expect(validLocationCoordinates(90, 180)).toBe(true);
    expect(validLocationCoordinates(25.033, 121.5654)).toBe(true);
  });

  it("rejects missing, non-finite, and out-of-range coordinates", () => {
    expect(validLocationCoordinates(null, 0)).toBe(false);
    expect(validLocationCoordinates(0, null)).toBe(false);
    expect(validLocationCoordinates(Number.NaN, 0)).toBe(false);
    expect(validLocationCoordinates(90.1, 0)).toBe(false);
    expect(validLocationCoordinates(0, -180.1)).toBe(false);
  });
});
