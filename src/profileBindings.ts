import type { AppBindingConflict, AppProfileBinding, ProfileResolution } from "./types";

export function sameProfileResolution(
  left: ProfileResolution | null,
  right: ProfileResolution | null,
) {
  return left === null
    ? right === null
    : right !== null && left.width === right.width && left.height === right.height;
}

export function bindingForScope(
  bundleId: string,
  targetResolution: ProfileResolution | null,
  bindings: readonly AppProfileBinding[],
) {
  return bindings.find((binding) => binding.bundle_id === bundleId
    && sameProfileResolution(binding.target_resolution, targetResolution));
}

export function conflictForScope(
  bundleId: string,
  targetResolution: ProfileResolution | null,
  conflicts: readonly AppBindingConflict[],
) {
  return conflicts.some((conflict) => conflict.bundle_id === bundleId
    && sameProfileResolution(conflict.target_resolution, targetResolution));
}

export function resolveAppProfileBinding(
  bundleId: string,
  frameSize: ProfileResolution,
  bindings: readonly AppProfileBinding[],
  conflicts: readonly AppBindingConflict[],
) {
  if (conflictForScope(bundleId, frameSize, conflicts)) return { binding: undefined, conflict: true };
  return { binding: bindingForScope(bundleId, frameSize, bindings), conflict: false };
}
