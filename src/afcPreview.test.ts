import { describe, expect, it } from "vitest";
import { isPreviewableImageName } from "./afcPreview";

describe("isPreviewableImageName", () => {
  it("accepts the supported image extensions case-insensitively", () => {
    for (const name of ["photo.png", "photo.JPG", "photo.jpeg", "photo.WebP", "photo.GIF", "photo.bmp"]) {
      expect(isPreviewableImageName(name)).toBe(true);
    }
  });

  it("does not accept unsupported extensions or directory-like names", () => {
    for (const name of ["photo.heic", "photo.tiff", "photo.svg", "photo", ".png", "folder.png/item"]) {
      expect(isPreviewableImageName(name)).toBe(false);
    }
  });
});
