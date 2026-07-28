import { spawnSync } from "node:child_process";

const cargo = process.platform === "win32" ? "cargo.exe" : "cargo";
const targetDirectories = ["src-tauri/target", "target"];

for (const targetDirectory of targetDirectories) {
  console.log(`Cleaning rebuildable Cargo artifacts in ${targetDirectory}...`);
  const result = spawnSync(
    cargo,
    [
      "clean",
      "--manifest-path",
      "Cargo.toml",
      "--target-dir",
      targetDirectory,
    ],
    { cwd: process.cwd(), stdio: "inherit" },
  );
  if (result.error) {
    console.error(`Unable to clean ${targetDirectory}: ${result.error.message}`);
    process.exit(1);
  }
  if (result.status !== 0) {
    console.error(`Cleaning ${targetDirectory} failed with exit code ${result.status ?? "unknown"}.`);
    process.exit(result.status ?? 1);
  }
}

console.log("Rust build artifacts removed. Cargo will recreate them on the next build.");
