import { describe, expect, it } from "vitest";
import { defaultDeviceViewPreferences, deviceViewScaleFactor, parseDeviceViewPreferences } from "./deviceViewPreferences";

describe("device view preferences", () => {
  it("uses defaults for missing and invalid preferences", () => {
    expect(parseDeviceViewPreferences(null)).toEqual(defaultDeviceViewPreferences);
    expect(parseDeviceViewPreferences("broken")).toEqual(defaultDeviceViewPreferences);
  });

  it("preserves valid values and repairs invalid fields", () => {
    expect(parseDeviceViewPreferences(JSON.stringify({
      scale: "1.5",
      controlOverlayVisible: false,
      rotationControlsLocked: true,
      fullscreenToolbarAutoHide: false,
      deviceInspectorVisible: false,
      mappingInspectorVisible: false,
    }))).toEqual({
      scale: "1.5",
      controlOverlayVisible: false,
      rotationControlsLocked: true,
      fullscreenToolbarAutoHide: false,
      deviceInspectorVisible: false,
      mappingInspectorVisible: false,
    });
    expect(parseDeviceViewPreferences('{"scale":"3"}')).toEqual(defaultDeviceViewPreferences);
  });

  it("adds visible inspectors when migrating older preferences", () => {
    expect(parseDeviceViewPreferences(JSON.stringify({
      scale: "1",
      controlOverlayVisible: false,
      rotationControlsLocked: true,
      fullscreenToolbarAutoHide: false,
    }))).toEqual({
      scale: "1",
      controlOverlayVisible: false,
      rotationControlsLocked: true,
      fullscreenToolbarAutoHide: false,
      deviceInspectorVisible: true,
      mappingInspectorVisible: true,
    });
  });

  it("maps fit and fixed scales", () => {
    expect(deviceViewScaleFactor("fit")).toBeNull();
    expect(deviceViewScaleFactor("1")).toBe(1);
    expect(deviceViewScaleFactor("0.25")).toBe(0.25);
  });
});
