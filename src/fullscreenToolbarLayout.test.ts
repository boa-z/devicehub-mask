import { describe, expect, it } from "vitest";
import { clampToolbarPosition, fullscreenToolbarDockPosition, isFullscreenToolbarDock, nearestFullscreenToolbarDock, resolveFullscreenToolbarDrop } from "./fullscreenToolbarLayout";

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
});
