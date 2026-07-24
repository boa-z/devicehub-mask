import { describe, expect, it } from "vitest";
import { clampToolbarPosition, fullscreenToolbarDockPosition, isFullscreenToolbarDock, nearestFullscreenToolbarDock, reconcileFullscreenToolbarDocks, resolveFullscreenToolbarDrop } from "./fullscreenToolbarLayout";

describe("fullscreen toolbar layout", () => {
  const container = { width: 1000, height: 700 };
  const toolbar = { width: 300, height: 44 };

  it("places stable slots inside the stage", () => {
    expect(fullscreenToolbarDockPosition("top-center", container, toolbar)).toEqual({ x: 350, y: 8 });
    expect(fullscreenToolbarDockPosition("bottom-right", container, toolbar)).toEqual({ x: 692, y: 648 });
    expect(fullscreenToolbarDockPosition("left-center", container, toolbar)).toEqual({ x: 8, y: 328 });
  });

  it("snaps to the nearest available slot", () => {
    expect(nearestFullscreenToolbarDock({ x: 500, y: 30 }, container, toolbar)).toBe("top-center");
    expect(nearestFullscreenToolbarDock({ x: 500, y: 30 }, container, toolbar, new Set(["top-center"]))).toBe("top-left");
  });

  it("clamps free dragging and validates persisted docks", () => {
    expect(clampToolbarPosition({ x: -20, y: 900 }, container, toolbar)).toEqual({ x: 8, y: 648 });
    expect(isFullscreenToolbarDock("right-center")).toBe(true);
    expect(isFullscreenToolbarDock("middle")).toBe(false);
  });

  it("gives hardware controls priority over an occupied slot", () => {
    expect(resolveFullscreenToolbarDrop(
      "hardware",
      { x: 500, y: 680 },
      { hardware: "top-center", function: "bottom-center" },
      container,
      toolbar,
      toolbar,
    )).toEqual({ hardware: "bottom-center", function: "bottom-left" });
    expect(resolveFullscreenToolbarDrop(
      "function",
      { x: 500, y: 30 },
      { hardware: "top-center", function: "bottom-center" },
      container,
      toolbar,
      toolbar,
    ).function).not.toBe("top-center");
  });

  it("keeps function controls away from hardware controls in adjacent overlapping slots", () => {
    const narrowContainer = { width: 600, height: 400 };
    const hardwareSize = { width: 280, height: 44 };
    const functionSize = { width: 360, height: 44 };

    expect(resolveFullscreenToolbarDrop(
      "function",
      { x: 188, y: 30 },
      { hardware: "top-center", function: "bottom-center" },
      narrowContainer,
      hardwareSize,
      functionSize,
    )).toEqual({ hardware: "top-center", function: "left-center" });

    expect(resolveFullscreenToolbarDrop(
      "hardware",
      { x: 300, y: 30 },
      { hardware: "bottom-center", function: "top-left" },
      narrowContainer,
      hardwareSize,
      functionSize,
    )).toEqual({ hardware: "top-center", function: "left-center" });
  });

  it("moves only function controls when a resized stage creates an overlap", () => {
    expect(reconcileFullscreenToolbarDocks(
      { hardware: "top-center", function: "top-left" },
      { width: 600, height: 400 },
      { width: 280, height: 44 },
      { width: 360, height: 44 },
    )).toEqual({ hardware: "top-center", function: "left-center" });

    expect(reconcileFullscreenToolbarDocks(
      { hardware: "top-center", function: "bottom-center" },
      container,
      toolbar,
      toolbar,
    )).toEqual({ hardware: "top-center", function: "bottom-center" });
  });
});
