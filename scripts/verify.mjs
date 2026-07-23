import { spawnSync } from "node:child_process";

const npmCli = process.env.npm_execpath;
const npm = npmCli ? process.execPath : process.platform === "win32" ? "npm.cmd" : "npm";
const npmPrefix = npmCli ? [npmCli] : [];
const cargo = process.platform === "win32" ? "cargo.exe" : "cargo";
const full = process.argv.includes("--full");

const checks = [
  [npm, [...npmPrefix, "run", "docs:check"], "documentation"],
  [npm, [...npmPrefix, "run", "lint"], "frontend lint"],
  [npm, [...npmPrefix, "run", "test"], "frontend tests"],
  [npm, [...npmPrefix, "run", "build"], "frontend build"],
  [cargo, ["fmt", "--manifest-path", "src-tauri/Cargo.toml", "--all", "--", "--check"], "Rust formatting"],
  [cargo, ["test", "--manifest-path", "src-tauri/Cargo.toml", "--locked"], "Rust tests"],
  [cargo, ["clippy", "--manifest-path", "src-tauri/Cargo.toml", "--all-targets", "--locked", "--", "-D", "warnings"], "Rust lint"],
];

if (full) {
  checks.push([npm, [...npmPrefix, "run", "tauri:build:debug"], "desktop debug build"]);
}

for (const [command, args, label] of checks) {
  console.log(`\n==> ${label}`);
  const result = spawnSync(command, args, { cwd: process.cwd(), env: process.env, stdio: "inherit" });
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
