import { describe, expect, it } from "vitest";
import { decodeDeviceConditionSelection, deviceConditionSelectionExists, encodeDeviceConditionSelection } from "./deviceConditions";

describe("device condition selections", () => {
  const groups = [{
    identifier: "Network [Link]",
    profiles: [{ identifier: "LTE, lossy", description: "Lossy LTE" }],
  }];

  it("round trips arbitrary enumerated identifiers without delimiter ambiguity", () => {
    const value = encodeDeviceConditionSelection({
      groupIdentifier: "Network [Link]",
      profileIdentifier: "LTE, lossy",
    });
    expect(decodeDeviceConditionSelection(value)).toEqual({
      groupIdentifier: "Network [Link]",
      profileIdentifier: "LTE, lossy",
    });
    expect(deviceConditionSelectionExists(groups, value)).toBe(true);
  });

  it("rejects malformed and unavailable selections", () => {
    expect(decodeDeviceConditionSelection("not-json")).toBeNull();
    expect(decodeDeviceConditionSelection('["only one"]')).toBeNull();
    expect(deviceConditionSelectionExists(groups, '["Network [Link]","5G"]')).toBe(false);
  });
});
