import { describe, expect, it } from "vitest";
import { defaultPerformanceHudPreferences, parsePerformanceHudPreferences } from "./performanceHudPreferences";

describe("performance HUD preferences", () => {
  it("uses defaults for missing or invalid data", () => {
    expect(parsePerformanceHudPreferences(null)).toEqual(defaultPerformanceHudPreferences);
    expect(parsePerformanceHudPreferences("not json")).toEqual(defaultPerformanceHudPreferences);
    expect(parsePerformanceHudPreferences("null")).toEqual(defaultPerformanceHudPreferences);
  });

  it("filters unknown and duplicate metric identifiers", () => {
    expect(parsePerformanceHudPreferences(JSON.stringify({
      enabled: true,
      items: ["system_cpu", "unknown", "system_cpu", "bandwidth"],
    }))).toEqual({ enabled: true, items: ["system_cpu", "bandwidth"] });
  });

  it("allows an empty metric selection", () => {
    expect(parsePerformanceHudPreferences('{"enabled":true,"items":[]}')).toEqual({ enabled: true, items: [] });
  });

  it("accepts device network metrics", () => {
    expect(parsePerformanceHudPreferences(
      '{"enabled":true,"items":["device_network_rx","device_network_tx"]}',
    )).toEqual({ enabled: true, items: ["device_network_rx", "device_network_tx"] });
  });
});
