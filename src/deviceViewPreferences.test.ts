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
      fullscreenHardwareToolbarDock: "bottom-right",
      fullscreenFunctionToolbarDock: "left-center",
    }))).toEqual({
      scale: "1.5",
      controlOverlayVisible: false,
      rotationControlsLocked: true,
      fullscreenToolbarAutoHide: false,
      deviceInspectorVisible: false,
      mappingInspectorVisible: false,
      fullscreenHardwareToolbarDock: "bottom-right",
      fullscreenFunctionToolbarDock: "left-center",
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
      fullscreenHardwareToolbarDock: "top-center",
      fullscreenFunctionToolbarDock: "bottom-center",
    });
  });

  it("repairs invalid toolbar docks independently", () => {
    expect(parseDeviceViewPreferences(JSON.stringify({
      fullscreenHardwareToolbarDock: "invalid",
      fullscreenFunctionToolbarDock: "right-center",
    }))).toMatchObject({
      fullscreenHardwareToolbarDock: "top-center",
      fullscreenFunctionToolbarDock: "right-center",
    });
    expect(parseDeviceViewPreferences(JSON.stringify({
      fullscreenHardwareToolbarDock: "top-left",
      fullscreenFunctionToolbarDock: "top-left",
    }))).toMatchObject({
      fullscreenHardwareToolbarDock: "top-left",
      fullscreenFunctionToolbarDock: "bottom-center",
    });
    expect(parseDeviceViewPreferences(JSON.stringify({
      fullscreenHardwareToolbarDock: "bottom-center",
      fullscreenFunctionToolbarDock: "bottom-center",
    }))).toMatchObject({
      fullscreenHardwareToolbarDock: "bottom-center",
      fullscreenFunctionToolbarDock: "top-center",
    });
  });

  it("maps fit and fixed scales", () => {
    expect(deviceViewScaleFactor("fit")).toBeNull();
    expect(deviceViewScaleFactor("1")).toBe(1);
    expect(deviceViewScaleFactor("0.25")).toBe(0.25);
  });
});
