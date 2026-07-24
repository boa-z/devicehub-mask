import { describe, expect, it, vi } from "vitest";
import { drawVideoFrame } from "./useDeviceVideoStream";

function drawingSurface() {
  const canvas = { width: 1, height: 1 } as HTMLCanvasElement;
  const context = {
    save: vi.fn(),
    restore: vi.fn(),
    translate: vi.fn(),
    rotate: vi.fn(),
    drawImage: vi.fn(),
  } as unknown as CanvasRenderingContext2D;
  const source = {} as CanvasImageSource;
  return { canvas, context, source };
}

describe("video frame presentation", () => {
  it("keeps portrait source dimensions", () => {
    const { canvas, context, source } = drawingSurface();
    expect(drawVideoFrame(canvas, context, source, 100, 200, "portrait")).toEqual({ width: 100, height: 200 });
    expect(context.drawImage).toHaveBeenCalledWith(source, 0, 0);
    expect(context.rotate).not.toHaveBeenCalled();
  });

  it("swaps dimensions and transforms landscape-right frames", () => {
    const { canvas, context, source } = drawingSurface();
    expect(drawVideoFrame(canvas, context, source, 100, 200, "landscape_right")).toEqual({ width: 200, height: 100 });
    expect(context.translate).toHaveBeenCalledWith(200, 0);
    expect(context.rotate).toHaveBeenCalledWith(Math.PI / 2);
  });
});
