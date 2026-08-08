const PREVIEWABLE_IMAGE_EXTENSIONS = new Set([
  ".png",
  ".jpg",
  ".jpeg",
  ".webp",
  ".gif",
  ".bmp",
]);

export function isPreviewableImageName(name: string): boolean {
  const lastDot = name.lastIndexOf(".");
  return lastDot > 0 && PREVIEWABLE_IMAGE_EXTENSIONS.has(name.slice(lastDot).toLowerCase());
}
