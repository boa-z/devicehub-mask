import type { KeymapCatalogDeviceContext, KeymapCatalogEntry } from "./types";

export type KeymapCatalogMatchLevel = "exact" | "app" | "device" | "other";

function sameResolution(
  left: { width: number; height: number },
  right: { width: number; height: number },
) {
  return left.width === right.width && left.height === right.height;
}

export function keymapCatalogMatchLevel(
  entry: KeymapCatalogEntry,
  bundleId: string | null,
  device: KeymapCatalogDeviceContext | null,
): KeymapCatalogMatchLevel {
  const appMatches = Boolean(bundleId && entry.match.bundle_ids.includes(bundleId));
  const productMatches = !device?.product_type
    || entry.match.product_types.length === 0
    || entry.match.product_types.includes(device.product_type);
  const frameMatches = !device
    || (sameResolution(entry.match.stream_resolution, device.frame_size)
      && entry.match.orientation === device.orientation);
  if (appMatches && productMatches && frameMatches) return "exact";
  if (appMatches && frameMatches) return "app";
  if (productMatches && frameMatches) return "device";
  return "other";
}

export function filterKeymapCatalogEntries(
  entries: readonly KeymapCatalogEntry[],
  bundleId: string | null,
  device: KeymapCatalogDeviceContext | null,
  query = "",
) {
  const normalizedQuery = query.trim().toLocaleLowerCase();
  return entries.filter((entry) => {
    if (bundleId && !entry.match.bundle_ids.includes(bundleId)) return false;
    if (device) {
      const productMatches = !device.product_type
        || entry.match.product_types.length === 0
        || entry.match.product_types.includes(device.product_type);
      const deviceMatches = productMatches
        && sameResolution(entry.match.stream_resolution, device.frame_size)
        && entry.match.orientation === device.orientation;
      if (!deviceMatches) return false;
    }
    return !normalizedQuery || [
      entry.id,
      entry.slug,
      entry.title,
      entry.description,
      entry.author,
      ...entry.match.bundle_ids,
      ...entry.match.product_types,
      `${entry.match.stream_resolution.width}x${entry.match.stream_resolution.height}`,
      `${entry.match.stream_resolution.width} x ${entry.match.stream_resolution.height}`,
      entry.match.orientation,
    ].some((value) => value.toLocaleLowerCase().includes(normalizedQuery));
  });
}
