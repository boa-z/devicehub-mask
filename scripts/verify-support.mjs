import { statfsSync } from "node:fs";

export const GIBIBYTE = 1024n ** 3n;
export const VERIFY_MIN_FREE_BYTES = 8n * GIBIBYTE;
export const VERIFY_FULL_MIN_FREE_BYTES = 12n * GIBIBYTE;

export function verificationEnvironment(environment = process.env) {
  return {
    ...environment,
    CARGO_INCREMENTAL: environment.CARGO_INCREMENTAL ?? "0",
    CARGO_BUILD_JOBS: environment.CARGO_BUILD_JOBS ?? "1",
  };
}

export function availableBytes(path = process.cwd()) {
  const filesystem = statfsSync(path, { bigint: true });
  return filesystem.bavail * filesystem.bsize;
}

export function formatGibibytes(bytes) {
  return `${(Number(bytes) / Number(GIBIBYTE)).toFixed(1)} GiB`;
}

export function requireFreeSpace(freeBytes, minimumBytes, mode) {
  if (freeBytes >= minimumBytes) {
    return;
  }

  throw new Error(
    `${mode} requires at least ${formatGibibytes(minimumBytes)} of free disk space; ` +
      `${formatGibibytes(freeBytes)} is available. Run npm run clean:rust to remove ` +
      "rebuildable Cargo artifacts, then retry.",
  );
}
