import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";

const cargo = process.platform === "win32" ? "cargo.exe" : "cargo";
const metadata = spawnSync(
  cargo,
  ["metadata", "--manifest-path", "Cargo.toml", "--format-version", "1", "--locked"],
  { cwd: process.cwd(), encoding: "utf8", maxBuffer: 32 * 1024 * 1024 },
);

if (metadata.error) {
  console.error(`Unable to inspect Rust workspace: ${metadata.error.message}`);
  process.exit(1);
}
if (metadata.status !== 0) {
  process.stderr.write(metadata.stderr);
  process.exit(metadata.status ?? 1);
}

const workspace = JSON.parse(metadata.stdout);
const coreForbidden = new Set([
  "axum",
  "idevice",
  "rmcp",
  "rodio",
  "tauri",
  "tokio",
  "tower-http",
  "wry",
]);
const packages = new Map(workspace.packages.map((pkg) => [pkg.id, pkg]));
const nodes = new Map(workspace.resolve.nodes.map((node) => [node.id, node]));
const runtimeForbidden = new Set(["axum", "rmcp", "rodio", "tauri", "tower-http", "wry"]);

function checkBoundary(packageName, forbidden) {
  const rootPackage = workspace.packages.find((pkg) => pkg.name === packageName);
  if (!rootPackage) {
    console.error(`Rust boundary check failed: ${packageName} is not a workspace package`);
    process.exit(1);
  }

  const pending = [rootPackage.id];
  const visited = new Set();
  const violations = new Set();
  while (pending.length > 0) {
    const packageId = pending.pop();
    if (visited.has(packageId)) continue;
    visited.add(packageId);

    const pkg = packages.get(packageId);
    if (packageId !== rootPackage.id) {
      const prohibited = forbidden.has(pkg?.name) || pkg?.name.startsWith("tauri-");
      if (prohibited) violations.add(pkg.name);
    }
    for (const dependency of nodes.get(packageId)?.dependencies ?? []) {
      pending.push(dependency);
    }
  }

  if (violations.size > 0) {
    console.error(
      `Rust boundary check failed: ${packageName} reaches forbidden dependencies: ${[...violations].sort().join(", ")}`,
    );
    process.exit(1);
  }
  console.log(`${packageName} boundaries OK: reaches ${visited.size - 1} dependency packages.`);
}

checkBoundary("devicehub-core", coreForbidden);
checkBoundary("devicehub-runtime", runtimeForbidden);

const hostFilesystemForbidden = ["std::path", "std::fs", "tokio::fs"];
for (const [service, sourcePath] of [
  ["public AFC", "crates/devicehub-runtime/src/storage/public.rs"],
  ["application storage", "crates/devicehub-runtime/src/storage/application.rs"],
  [
    "developer image mount",
    "crates/devicehub-runtime/src/device/developer_image/mount.rs",
  ],
  ["network capture", "crates/devicehub-runtime/src/capture/network.rs"],
  [
    "Bluetooth capture",
    "crates/devicehub-runtime/src/capture/bluetooth.rs",
  ],
  ["sysdiagnose", "crates/devicehub-runtime/src/diagnostics/sysdiagnose.rs"],
  ["log archive", "crates/devicehub-runtime/src/diagnostics/log_archive.rs"],
  ["device backup", "crates/devicehub-runtime/src/diagnostics/device_backup.rs"],
  ["device details", "crates/devicehub-runtime/src/device/details.rs"],
]) {
  const source = readFileSync(sourcePath, "utf8");
  const violations = hostFilesystemForbidden.filter((dependency) =>
    source.includes(dependency),
  );
  if (violations.length > 0) {
    console.error(
      `Rust boundary check failed: runtime ${service} reaches host filesystem APIs: ${violations.join(", ")}`,
    );
    process.exit(1);
  }
  console.log(`devicehub-runtime ${service} host filesystem boundary OK.`);
}
