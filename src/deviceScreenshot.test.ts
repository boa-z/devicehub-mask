import { describe, expect, it } from "vitest";
import { parsePngDimensions } from "./deviceScreenshot";

function pngHeader(width: number, height: number) {
  const bytes = new Uint8Array(24);
  bytes.set([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  bytes.set([0x49, 0x48, 0x44, 0x52], 12);
  const view = new DataView(bytes.buffer);
  view.setUint32(8, 13);
  view.setUint32(16, width);
  view.setUint32(20, height);
  return bytes;
}

describe("device screenshots", () => {
  it("reads dimensions from a PNG IHDR", () => {
    expect(parsePngDimensions(pngHeader(2160, 1620))).toEqual({ width: 2160, height: 1620 });
  });

  it("rejects truncated, invalid, and empty PNG headers", () => {
    expect(parsePngDimensions(new Uint8Array(8))).toBeNull();
    expect(parsePngDimensions(new Uint8Array(24))).toBeNull();
    expect(parsePngDimensions(pngHeader(0, 100))).toBeNull();
  });
});
