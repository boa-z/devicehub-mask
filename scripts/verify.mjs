import { spawnSync } from "node:child_process";

import {
  VERIFY_FULL_MIN_FREE_BYTES,
  VERIFY_MIN_FREE_BYTES,
  availableBytes,
  formatGibibytes,
  requireFreeSpace,
  verificationEnvironment,
} from "./verify-support.mjs";

const npmCli = process.env.npm_execpath;
const npm = npmCli ? process.execPath : process.platform === "win32" ? "npm.cmd" : "npm";
const npmPrefix = npmCli ? [npmCli] : [];
const cargo = process.platform === "win32" ? "cargo.exe" : "cargo";
const full = process.argv.includes("--full");
const mode = full ? "Full local verification" : "Local verification";
const minimumFreeBytes = full ? VERIFY_FULL_MIN_FREE_BYTES : VERIFY_MIN_FREE_BYTES;
const freeBytes = availableBytes();
const env = verificationEnvironment();

try {
  requireFreeSpace(freeBytes, minimumFreeBytes, mode);
} catch (error) {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
}

console.log(`${mode}: ${formatGibibytes(freeBytes)} free (minimum ${formatGibibytes(minimumFreeBytes)}).`);
console.log(
  `Cargo verification settings: CARGO_INCREMENTAL=${env.CARGO_INCREMENTAL}, ` +
    `CARGO_BUILD_JOBS=${env.CARGO_BUILD_JOBS}.`,
);
console.log("Physical-device tests are excluded; run npm run verify:device explicitly.");

const checks = [
  [process.execPath, ["--test", "scripts/verify-support.node.mjs"], "verification preflight tests"],
  [process.execPath, ["--check", "scripts/clean-rust.mjs"], "Rust cleanup script syntax"],
  [process.execPath, ["--check", "scripts/package-headless.mjs"], "headless package script syntax"],
  [npm, [...npmPrefix, "run", "docs:check"], "documentation"],
  [npm, [...npmPrefix, "run", "rust:boundaries"], "Rust architecture boundaries"],
  [npm, [...npmPrefix, "run", "lint"], "frontend lint"],
  [npm, [...npmPrefix, "run", "test"], "frontend tests"],
  [npm, [...npmPrefix, "run", "build"], "frontend build"],
  [cargo, ["fmt", "--manifest-path", "Cargo.toml", "--all", "--", "--check"], "Rust formatting"],
  [cargo, ["test", "--manifest-path", "Cargo.toml", "--workspace", "--locked"], "Rust tests"],
  [cargo, ["clippy", "--manifest-path", "Cargo.toml", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings"], "Rust lint"],
];

if (full) {
  checks.push([npm, [...npmPrefix, "run", "tauri:build:debug"], "desktop debug build"]);
}

for (const [command, args, label] of checks) {
  console.log(`\n==> ${label}`);
  const result = spawnSync(command, args, { cwd: process.cwd(), env, stdio: "inherit" });
  if (result.error) {
    console.error(`Unable to start ${label}: ${result.error.message}`);
    process.exit(1);
  }
  if (result.status !== 0) {
    console.error(`${label} failed with exit code ${result.status ?? "unknown"}`);
    process.exit(result.status ?? 1);
  }
}

console.log(`\nVerification passed${full ? ", including the desktop debug build" : ""}.`);
