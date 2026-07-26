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

const hostApiForbidden = ["std::path", "std::fs", "tokio::fs", "std::env"];
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
  ["crash report exports", "crates/devicehub-runtime/src/device/crash_reports.rs"],
  ["provisioning profiles", "crates/devicehub-runtime/src/device/provisioning.rs"],
  ["screen media negotiation", "crates/devicehub-runtime/src/media/negotiation.rs"],
  ["audio RTP", "crates/devicehub-runtime/src/media/audio_rtp.rs"],
  ["video RTP", "crates/devicehub-runtime/src/media/video_rtp.rs"],
  ["RTCP", "crates/devicehub-runtime/src/media/rtcp.rs"],
  ["media session", "crates/devicehub-runtime/src/media/orchestrator.rs"],
  ["session service ports", "crates/devicehub-runtime/src/session/services.rs"],
  ["session commands", "crates/devicehub-runtime/src/session/commands.rs"],
  ["session command router", "crates/devicehub-runtime/src/session/router.rs"],
  ["session input loop", "crates/devicehub-runtime/src/session/input.rs"],
  ["connected session runner", "crates/devicehub-runtime/src/session/runner.rs"],
  ["outer session manager", "crates/devicehub-runtime/src/session/manager.rs"],
  ["CoreRuntime lifecycle", "crates/devicehub-runtime/src/runtime.rs"],
  ["device trust", "crates/devicehub-runtime/src/session/trust.rs"],
  [
    "Wi-Fi discovery",
    "crates/devicehub-runtime/src/transport/wifi_discovery.rs",
  ],
  ["device discovery", "crates/devicehub-runtime/src/transport/discovery.rs"],
  [
    "session diagnostic sinks",
    "crates/devicehub-runtime/src/session/diagnostics.rs",
  ],
  ["HID protocol", "crates/devicehub-runtime/src/input/hid.rs"],
  ["clipboard session", "crates/devicehub-runtime/src/clipboard/session.rs"],
]) {
  const source = readFileSync(sourcePath, "utf8");
  const violations = hostApiForbidden.filter((dependency) =>
    source.includes(dependency),
  );
  if (violations.length > 0) {
    console.error(
      `Rust boundary check failed: runtime ${service} reaches host APIs: ${violations.join(", ")}`,
    );
    process.exit(1);
  }
  console.log(`devicehub-runtime ${service} host API boundary OK.`);
}

const sessionServices = readFileSync(
  "crates/devicehub-runtime/src/session/services.rs",
  "utf8",
);
const supervisorEscapes = [
  "pub fn reporter(",
  "pub fn shutdown_receiver(",
  "pub fn spawn_host_task(",
];
const exposedSupervisorApis = supervisorEscapes.filter((signature) =>
  sessionServices.includes(signature),
);
if (exposedSupervisorApis.length > 0) {
  console.error(
    `Rust boundary check failed: runtime session exposes supervisor escape hatches: ${exposedSupervisorApis.join(", ")}`,
  );
  process.exit(1);
}
console.log("devicehub-runtime session supervisor ownership boundary OK.");

const runtimeSessionRunner = readFileSync(
  "crates/devicehub-runtime/src/session/runner.rs",
  "utf8",
);
const requiredRuntimeOwnership = [
  "RuntimeSessionServices::start",
  "MediaSessionRuntime::new",
  "session_services.shutdown().await",
  "display.stop_media_stream().await",
];
const missingRuntimeOwnership = requiredRuntimeOwnership.filter(
  (signature) => !runtimeSessionRunner.includes(signature),
);
if (missingRuntimeOwnership.length > 0) {
  console.error(
    `Rust boundary check failed: connected session runner lost lifecycle ownership: ${missingRuntimeOwnership.join(", ")}`,
  );
  process.exit(1);
}
console.log("devicehub-runtime connected session lifecycle ownership boundary OK.");

const tauriSession = readFileSync("src-tauri/src/session.rs", "utf8");
const tauriSessionManager = readFileSync(
  "src-tauri/src/session/manager.rs",
  "utf8",
);
const forbiddenTauriSessionOwnership = [
  "RuntimeSessionServices::start",
  "MediaSessionRuntime::new",
  "start_screen_media_stream(",
  "connect_device_input(",
];
const retainedTauriOwnership = forbiddenTauriSessionOwnership.filter(
  (signature) =>
    tauriSession.includes(signature) || tauriSessionManager.includes(signature),
);
if (
  retainedTauriOwnership.length > 0 ||
  !tauriSessionManager.includes("run_session_manager(")
) {
  console.error(
    `Rust boundary check failed: Tauri retained connected session orchestration: ${retainedTauriOwnership.join(", ")}`,
  );
  process.exit(1);
}
console.log("Tauri connected session host-adapter boundary OK.");

const tauriSessionRoot = readFileSync("src-tauri/src/session.rs", "utf8");
if (
  tauriSessionRoot.includes("mod trust;") ||
  tauriSessionManager.includes("LockdownClient") ||
  tauriSessionManager.includes("save_pair_record") ||
  tauriSessionManager.includes("delete_pair_record")
) {
  console.error(
    "Rust boundary check failed: Tauri retained device trust protocol ownership",
  );
  process.exit(1);
}
console.log("devicehub-runtime device trust ownership boundary OK.");

if (
  tauriSessionRoot.includes("mod discovery;") ||
  tauriSessionManager.includes("struct DeviceDiscovery") ||
  tauriSessionManager.includes("get_devices().await")
) {
  console.error(
    "Rust boundary check failed: Tauri retained device discovery protocol ownership",
  );
  process.exit(1);
}
console.log("devicehub-runtime device discovery ownership boundary OK.");

const runtimeSessionManager = readFileSync(
  "crates/devicehub-runtime/src/session/manager.rs",
  "utf8",
);
const requiredManagerOwnership = [
  "DeviceDiscovery<",
  "SessionRetryPolicy::default()",
  "run_connected_session(",
  "pair_device(",
  "forget_device(",
  "ACTIVE_RESCAN",
  "SWITCH_GRACE",
];
const missingManagerOwnership = requiredManagerOwnership.filter(
  (signature) => !runtimeSessionManager.includes(signature),
);
const forbiddenTauriManagerOwnership = [
  "enum Next",
  "SessionRetryPolicy",
  "run_connected_session(",
  "pair_device(",
  "forget_device(",
  "active_rescan",
  "tokio::select!",
];
const retainedTauriManagerOwnership = forbiddenTauriManagerOwnership.filter(
  (signature) => tauriSessionManager.includes(signature),
);
if (
  missingManagerOwnership.length > 0 ||
  retainedTauriManagerOwnership.length > 0
) {
  console.error(
    `Rust boundary check failed: outer session manager ownership drifted (runtime missing: ${missingManagerOwnership.join(", ")}; Tauri retained: ${retainedTauriManagerOwnership.join(", ")})`,
  );
  process.exit(1);
}
console.log("devicehub-runtime outer session manager ownership boundary OK.");

const runtimeOwner = readFileSync(
  "crates/devicehub-runtime/src/runtime.rs",
  "utf8",
);
const tauriRuntimeOwner = readFileSync(
  "src-tauri/src/device_runtime.rs",
  "utf8",
);
const requiredRuntimeOwner = [
  "pub struct CoreRuntime",
  "std::thread::Builder::new()",
  ".stack_size(OWNER_THREAD_STACK_BYTES)",
  "tokio::task::LocalSet::new()",
  "SessionControlCommand::Quit",
  "thread.join()",
];
const missingRuntimeOwner = requiredRuntimeOwner.filter(
  (signature) => !runtimeOwner.includes(signature),
);
const forbiddenTauriRuntimeOwner = [
  "std::thread::Builder",
  "JoinHandle",
  "stack_size(",
  "tokio::task::LocalSet",
  "tokio::runtime::Builder",
];
const retainedTauriRuntimeOwner = forbiddenTauriRuntimeOwner.filter(
  (signature) => tauriRuntimeOwner.includes(signature),
);
if (
  missingRuntimeOwner.length > 0 ||
  retainedTauriRuntimeOwner.length > 0 ||
  !tauriRuntimeOwner.includes("devicehub_runtime::CoreRuntime::start(")
) {
  console.error(
    `Rust boundary check failed: CoreRuntime lifecycle ownership drifted (runtime missing: ${missingRuntimeOwner.join(", ")}; Tauri retained: ${retainedTauriRuntimeOwner.join(", ")})`,
  );
  process.exit(1);
}
console.log("devicehub-runtime CoreRuntime lifecycle ownership boundary OK.");
