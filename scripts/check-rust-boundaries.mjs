import { spawnSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

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

const serverPackage = workspace.packages.find(
  (pkg) => pkg.name === "devicehub-server",
);
if (!serverPackage) {
  console.error(
    "Rust boundary check failed: devicehub-server is not a workspace package",
  );
  process.exit(1);
}
const serverDirectForbidden = new Set(["idevice", "rodio", "tauri", "wry"]);
const serverDirectViolations = serverPackage.dependencies
  .map((dependency) => dependency.name)
  .filter(
    (name) => serverDirectForbidden.has(name) || name.startsWith("tauri-"),
  );
if (serverDirectViolations.length > 0) {
  console.error(
    `Rust boundary check failed: devicehub-server directly depends on host or device implementation crates: ${serverDirectViolations.join(", ")}`,
  );
  process.exit(1);
}
console.log("devicehub-server direct dependency boundary OK.");

function rustSources(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return rustSources(path);
    return entry.isFile() && entry.name.endsWith(".rs") ? [path] : [];
  });
}

function productionSource(source) {
  // Runtime tests may use temporary directories. Production items precede the
  // conventional trailing test module in every runtime source file.
  const testModule = source.search(/#\[cfg\(test\)\]\s*mod tests\s*\{/u);
  return testModule === -1 ? source : source.slice(0, testModule);
}

const runtimeHostResolutionForbidden = [
  "std::env",
  "std::process",
  "tokio::process",
  "Command::new",
  "current_exe",
  "ffmpeg",
];
const runtimeHostResolutionViolations = rustSources(
  "crates/devicehub-runtime/src",
).flatMap((sourcePath) => {
  const source = productionSource(readFileSync(sourcePath, "utf8"));
  return runtimeHostResolutionForbidden
    .filter((token) => source.toLowerCase().includes(token.toLowerCase()))
    .map((token) => `${sourcePath}: ${token}`);
});
if (runtimeHostResolutionViolations.length > 0) {
  console.error(
    `Rust boundary check failed: devicehub-runtime resolves host environment or processes: ${runtimeHostResolutionViolations.join(", ")}`,
  );
  process.exit(1);
}
console.log(
  "devicehub-runtime production code receives host environment and process capabilities through ports.",
);

const serverOwnershipForbidden = [
  "std::env",
  "std::process",
  "tokio::process",
  "TcpListener::bind",
  "CoreRuntime",
  "start_runtime(",
];
const serverOwnershipViolations = rustSources(
  "crates/devicehub-server/src",
).flatMap((sourcePath) => {
  const source = productionSource(readFileSync(sourcePath, "utf8"));
  return serverOwnershipForbidden
    .filter((token) => source.includes(token))
    .map((token) => `${sourcePath}: ${token}`);
});
if (serverOwnershipViolations.length > 0) {
  console.error(
    `Rust boundary check failed: devicehub-server owns host configuration, listeners, or device runtime lifecycle: ${serverOwnershipViolations.join(", ")}`,
  );
  process.exit(1);
}
console.log(
  "devicehub-server receives runtime clients and configuration without owning listeners or device lifecycles.",
);

const serverMcp = readFileSync("crates/devicehub-server/src/mcp.rs", "utf8");
const tauriMcp = readFileSync("src-tauri/src/mcp.rs", "utf8");
if (
  !serverMcp.includes("pub fn router(") ||
  !serverMcp.includes("impl ServerHandler for DeviceHub") ||
  !serverMcp.includes("Implementation::new(") ||
  !serverMcp.includes('"devicehub_mask"') ||
  tauriMcp.includes("impl ServerHandler") ||
  tauriMcp.includes("ToolRouter") ||
  !tauriMcp.includes("devicehub_server::mcp::router(application)")
) {
  console.error(
    "Rust boundary check failed: MCP service ownership drifted back into the Tauri host",
  );
  process.exit(1);
}
console.log(
  "devicehub-server owns MCP tools and routing while Tauri owns only listener policy.",
);

const serverHttp = readFileSync("crates/devicehub-server/src/http.rs", "utf8");
const serverAppsHttp = readFileSync(
  "crates/devicehub-server/src/http/apps.rs",
  "utf8",
);
const serverCrashReportsHttp = readFileSync(
  "crates/devicehub-server/src/http/crash_reports.rs",
  "utf8",
);
const serverPerformanceHttp = readFileSync(
  "crates/devicehub-server/src/http/performance.rs",
  "utf8",
);
const serverDiagnosticsHttp = readFileSync(
  "crates/devicehub-server/src/http/diagnostics.rs",
  "utf8",
);
const serverStorageHttp = readFileSync(
  "crates/devicehub-server/src/http/storage.rs",
  "utf8",
);
if (
  !serverHttp.includes("pub use apps::{AppHttpState, router as apps_router}") ||
  !serverHttp.includes(
    "pub use crash_reports::{CrashReportHttpState, router as crash_reports_router}",
  ) ||
  !serverHttp.includes(
    "pub use storage::{StorageHttpState, router as storage_router}",
  ) ||
  !serverHttp.includes("DiagnosticDestinationPreparer") ||
  !serverAppsHttp.includes("pub fn router<S>(state: AppHttpState)") ||
  !serverCrashReportsHttp.includes(
    "pub fn router<S>(state: CrashReportHttpState)",
  ) ||
  !serverPerformanceHttp.includes(
    "pub fn router<S>(state: PerformanceHttpState)",
  ) ||
  !serverPerformanceHttp.includes("pub struct CaptureDestinationValidator") ||
  !serverDiagnosticsHttp.includes(
    "pub fn router<S>(state: DiagnosticsHttpState)",
  ) ||
  !serverDiagnosticsHttp.includes("pub struct DiagnosticDestinationPreparer") ||
  !serverStorageHttp.includes("pub fn router<S>(state: StorageHttpState)") ||
  !serverStorageHttp.includes("validate_app_bundle_id") ||
  existsSync("src-tauri/src/http_apps.rs") ||
  existsSync("src-tauri/src/http_crash_reports.rs") ||
  existsSync("src-tauri/src/http_performance.rs") ||
  existsSync("src-tauri/src/http_diagnostics.rs") ||
  existsSync("src-tauri/src/http_storage.rs") ||
  existsSync("src-tauri/src/app_documents.rs") ||
  existsSync("src-tauri/src/device_files.rs") ||
  existsSync("src-tauri/src/sysdiagnose.rs") ||
  existsSync("src-tauri/src/log_archive.rs") ||
  existsSync("src-tauri/src/network_capture.rs") ||
  existsSync("src-tauri/src/bluetooth_capture.rs")
) {
  console.error(
    "Rust boundary check failed: reusable HTTP ownership drifted back into the Tauri host",
  );
  process.exit(1);
}
console.log(
  "devicehub-server owns application, crash-report, performance, storage, and diagnostics HTTP adapters while Tauri only composes injected state and filesystem policy.",
);

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
  ["CoreRuntime lifecycle", "crates/devicehub-runtime/src/runtime/owner.rs"],
  ["runtime client", "crates/devicehub-runtime/src/client.rs"],
  ["device control client", "crates/devicehub-runtime/src/client/control.rs"],
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
  !tauriSessionManager.includes("devicehub_runtime::start_runtime(") ||
  !tauriSessionManager.includes("RuntimeHostAdapters {") ||
  tauriSessionManager.includes("SessionManager") ||
  tauriSessionManager.includes(".run(")
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
  tauriSessionManager.includes("DeviceDiscovery") ||
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
  "pub struct RuntimeHostAdapters",
  "struct SessionManager",
  "pub fn start_runtime<",
  "pub(crate) async fn run(",
  "DeviceDiscovery<",
  "DeviceDiscovery::new(",
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
  "DeviceDiscovery",
  "enum Next",
  "SessionRetryPolicy",
  "SessionManager",
  "run_connected_session(",
  "run_session_manager(",
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

const runtimeModule = readFileSync("crates/devicehub-runtime/src/runtime.rs", "utf8");
const runtimeOwner = [
  runtimeModule,
  readFileSync("crates/devicehub-runtime/src/runtime/owner.rs", "utf8"),
  readFileSync("crates/devicehub-runtime/src/runtime/state.rs", "utf8"),
].join("\n");
const tauriRuntimeOwner = readFileSync(
  "src-tauri/src/device_runtime.rs",
  "utf8",
);
const requiredRuntimeOwner = [
  "mod owner;",
  "mod state;",
  "pub use owner::{CoreRuntime, OWNER_THREAD_STACK_BYTES};",
  "pub(crate) use state::CoreRuntimeState;",
  "pub struct CoreRuntime",
  "pub(crate) fn start<State, Build, Task>",
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
  runtimeModule.includes("pub struct CoreRuntime") ||
  runtimeModule.includes("struct CoreRuntimeState") ||
  retainedTauriRuntimeOwner.length > 0 ||
  readFileSync("crates/devicehub-runtime/src/lib.rs", "utf8").includes(
    "CoreRuntimeFuture",
  ) ||
  tauriRuntimeOwner.includes("devicehub_runtime::CoreRuntime::start(") ||
  tauriRuntimeOwner.includes("CoreRuntimeFuture") ||
  !tauriRuntimeOwner.includes("crate::session::start_manager(")
) {
  console.error(
    `Rust boundary check failed: CoreRuntime lifecycle ownership drifted (runtime missing: ${missingRuntimeOwner.join(", ")}; Tauri retained: ${retainedTauriRuntimeOwner.join(", ")})`,
  );
  process.exit(1);
}
console.log("devicehub-runtime CoreRuntime lifecycle ownership boundary OK.");

const runtimeFacade = readFileSync(
  "crates/devicehub-runtime/src/lib.rs",
  "utf8",
);
const runtimeInput = readFileSync(
  "crates/devicehub-runtime/src/input/dispatcher.rs",
  "utf8",
);
const coreInput = readFileSync("crates/devicehub-core/src/input.rs", "utf8");
const runtimeHid = readFileSync(
  "crates/devicehub-runtime/src/input/hid.rs",
  "utf8",
);
const runtimeSupervisor = readFileSync(
  "crates/devicehub-runtime/src/supervisor.rs",
  "utf8",
);
const coreServiceHealth = readFileSync(
  "crates/devicehub-core/src/service_health.rs",
  "utf8",
);
const serviceHealthAdapter = readFileSync(
  "crates/devicehub-server/src/http/performance.rs",
  "utf8",
);
const publicInputFacade = runtimeFacade.match(/pub use input::\{([^}]*)\};/)?.[1] ?? "";
const publicSupervisorFacade =
  runtimeFacade.match(/pub use supervisor::\{([^}]*)\};/)?.[1] ?? "";
const forbiddenExecutionExports = [
  "DeviceInputDispatcher",
  "ServiceReporter",
  "ServiceSupervisor",
  "reconnect_backoff",
  "wait_for_retry",
];
const exposedExecutionInternals = forbiddenExecutionExports.filter(
  (name) =>
    publicInputFacade.includes(name) || publicSupervisorFacade.includes(name),
);
if (
  exposedExecutionInternals.length > 0 ||
  runtimeInput.includes("pub struct DeviceInputDispatcher") ||
  runtimeInput.includes("pub async fn dispatch(") ||
  runtimeSupervisor.includes("pub struct ServiceReporter") ||
  runtimeSupervisor.includes("pub struct ServiceSupervisor") ||
  runtimeSupervisor.includes("pub fn reconnect_backoff(") ||
  runtimeSupervisor.includes("pub async fn wait_for_retry(") ||
  runtimeSupervisor.includes("pub enum ServicePhase") ||
  runtimeSupervisor.includes("pub struct ServiceHealth") ||
  runtimeSupervisor.includes("pub struct ServiceRegistry") ||
  runtimeFacade.includes("ServiceHealth") ||
  runtimeFacade.includes("ServicePhase") ||
  runtimeFacade.includes("ServiceRegistry") ||
  !coreServiceHealth.includes("pub enum ServicePhase") ||
  !coreServiceHealth.includes("pub struct ServiceHealth") ||
  !coreServiceHealth.includes("pub struct ServiceRegistry") ||
  !coreServiceHealth.includes("pub fn record(") ||
  serviceHealthAdapter.includes("devicehub_runtime::ServiceHealth") ||
  serviceHealthAdapter.includes("devicehub_runtime::ServiceRegistry") ||
  !serviceHealthAdapter.includes("ServiceHealth, ServiceRegistry") ||
  !coreInput.includes("pub enum DeviceInputCommand") ||
  !coreInput.includes("pub struct TouchContact") ||
  runtimeInput.includes("pub enum DeviceInputCommand") ||
  runtimeHid.includes("pub struct TouchContact") ||
  runtimeFacade.includes("pub use input::{DeviceInputCommand")
) {
  console.error(
    `Rust boundary check failed: runtime execution internals escaped the public facade: ${exposedExecutionInternals.join(", ")}`,
  );
  process.exit(1);
}
console.log(
  "devicehub-core owns input and service-health contracts while runtime execution internals stay private.",
);

const runtimeCaptureFacade = readFileSync(
  "crates/devicehub-runtime/src/capture.rs",
  "utf8",
);
const coreCapture = readFileSync(
  "crates/devicehub-core/src/capture.rs",
  "utf8",
);
const runtimeDiagnosticsFacade = readFileSync(
  "crates/devicehub-runtime/src/diagnostics.rs",
  "utf8",
);
const coreDiagnostics = readFileSync(
  "crates/devicehub-core/src/diagnostics.rs",
  "utf8",
);
const runtimeCrashReports = readFileSync(
  "crates/devicehub-runtime/src/device/crash_reports.rs",
  "utf8",
);
const captureAdapter = readFileSync(
  "crates/devicehub-server/src/http/performance.rs",
  "utf8",
);
const diagnosticAdapter = readFileSync(
  "crates/devicehub-server/src/http/diagnostics.rs",
  "utf8",
);
if (
  runtimeCaptureFacade.includes("pub use devicehub_core") ||
  runtimeDiagnosticsFacade.includes("pub use devicehub_core") ||
  runtimeCaptureFacade.includes("NetworkCaptureSlot") ||
  runtimeCaptureFacade.includes("BluetoothCaptureSlot") ||
  runtimeDiagnosticsFacade.includes("DeviceBackupSlot") ||
  runtimeDiagnosticsFacade.includes("SysdiagnoseSlot") ||
  runtimeDiagnosticsFacade.includes("LogArchiveSlot") ||
  runtimeCrashReports.includes("pub use devicehub_core::validate_crash_report_path") ||
  runtimeFacade.includes("validate_crash_report_path,") ||
  !coreCapture.includes("pub struct NetworkCaptureSlot") ||
  !coreCapture.includes("pub struct BluetoothCaptureSlot") ||
  !coreDiagnostics.includes("pub struct DeviceBackupSlot") ||
  !coreDiagnostics.includes("pub struct SysdiagnoseSlot") ||
  !coreDiagnostics.includes("pub struct LogArchiveSlot") ||
  !captureAdapter.includes("NetworkCaptureSlot") ||
  !captureAdapter.includes("BluetoothCaptureSlot") ||
  !captureAdapter.includes("use devicehub_core::{") ||
  !diagnosticAdapter.includes("use devicehub_core::{") ||
  !diagnosticAdapter.includes("DeviceBackupSlot") ||
  !diagnosticAdapter.includes("SysdiagnoseSlot") ||
  !diagnosticAdapter.includes("LogArchiveSlot") ||
  captureAdapter.includes("devicehub_runtime::NetworkCaptureSlot") ||
  captureAdapter.includes("devicehub_runtime::BluetoothCaptureSlot") ||
  diagnosticAdapter.includes("devicehub_runtime::DeviceBackupSlot") ||
  diagnosticAdapter.includes("devicehub_runtime::SysdiagnoseSlot") ||
  diagnosticAdapter.includes("devicehub_runtime::LogArchiveSlot")
) {
  console.error(
    "Rust boundary check failed: capture or diagnostic domain values are re-exported through runtime",
  );
  process.exit(1);
}
console.log("devicehub-core owns capture and diagnostic domain values directly.");

const coreDeveloperImage = readFileSync(
  "crates/devicehub-core/src/developer_image.rs",
  "utf8",
);
const runtimeDeveloperImage = readFileSync(
  "crates/devicehub-runtime/src/device/developer_image.rs",
  "utf8",
);
const runtimeDeveloperImageMount = readFileSync(
  "crates/devicehub-runtime/src/device/developer_image/mount.rs",
  "utf8",
);
const tauriDeveloperImage = readFileSync(
  "src-tauri/src/developer_image.rs",
  "utf8",
);
const requiredCoreDeveloperImage = [
  "pub enum DeveloperImageMountState",
  "pub struct DeveloperImageMountStatus",
  "pub struct DeveloperImageMountSlot",
  "pub fn developer_image_type_for_version",
];
const missingCoreDeveloperImage = requiredCoreDeveloperImage.filter(
  (definition) => !coreDeveloperImage.includes(definition),
);
if (
  missingCoreDeveloperImage.length > 0 ||
  runtimeDeveloperImage.includes("pub fn developer_image_type_for_version") ||
  runtimeDeveloperImageMount.includes("pub enum DeveloperImageMountState") ||
  runtimeDeveloperImageMount.includes("pub struct DeveloperImageMountStatus") ||
  runtimeDeveloperImageMount.includes("pub struct DeveloperImageMountSlot") ||
  runtimeFacade.includes("DeveloperImageMountState") ||
  runtimeFacade.includes("DeveloperImageMountStatus") ||
  runtimeFacade.includes("DeveloperImageMountSlot") ||
  runtimeFacade.includes("developer_image_type_for_version") ||
  tauriDeveloperImage.includes("devicehub_runtime::DeveloperImageMountState") ||
  tauriDeveloperImage.includes("devicehub_runtime::{DeveloperImageMountSlot") ||
  !tauriDeveloperImage.includes("devicehub_core::{DeveloperImageMountSlot")
) {
  console.error(
    `Rust boundary check failed: Developer Image domain state is not owned directly by core: ${missingCoreDeveloperImage.join(", ")}`,
  );
  process.exit(1);
}
console.log("devicehub-core owns Developer Image state and version policy.");

const coreDeviceLogs = readFileSync(
  "crates/devicehub-core/src/device_logs.rs",
  "utf8",
);
const runtimeDeviceLogs = readFileSync(
  "crates/devicehub-runtime/src/device/logs.rs",
  "utf8",
);
const tauriDeviceLogAdapters = [
  readFileSync("crates/devicehub-server/src/http/performance.rs", "utf8"),
  readFileSync("crates/devicehub-server/src/mcp.rs", "utf8"),
].join("\n");
const deviceLogDomainDefinitions = [
  "pub struct DeviceLogBatch",
  "pub struct DeviceLogEntry",
  "pub enum DeviceLogLevel",
  "pub struct DeviceLogMetadata",
  "pub struct DeviceLogSlot",
  "pub enum DeviceLogSource",
  "pub const MAX_DEVICE_LOG_BATCH_ENTRIES",
];
const missingCoreDeviceLogs = deviceLogDomainDefinitions.filter(
  (definition) => !coreDeviceLogs.includes(definition),
);
if (
  missingCoreDeviceLogs.length > 0 ||
  /pub (struct|enum) DeviceLog(Batch|Entry|Level|Source)/u.test(
    runtimeDeviceLogs,
  ) ||
  runtimeFacade.includes("DeviceLogBatch") ||
  runtimeFacade.includes("DeviceLogEntry") ||
  runtimeFacade.includes("DeviceLogLevel") ||
  runtimeFacade.includes("DeviceLogMetadata") ||
  runtimeFacade.includes("DeviceLogSlot") ||
  runtimeFacade.includes("DeviceLogSource") ||
  runtimeFacade.includes("MAX_DEVICE_LOG_BATCH_ENTRIES") ||
  tauriDeviceLogAdapters.includes("devicehub_runtime::DeviceLogBatch") ||
  tauriDeviceLogAdapters.includes("devicehub_runtime::DeviceLogEntry") ||
  tauriDeviceLogAdapters.includes("devicehub_runtime::DeviceLogLevel") ||
  tauriDeviceLogAdapters.includes("devicehub_runtime::DeviceLogSlot") ||
  !tauriDeviceLogAdapters.includes("devicehub_core::DeviceLogBatch") ||
  !tauriDeviceLogAdapters.includes("use devicehub_core::{")
) {
  console.error(
    `Rust boundary check failed: device log domain values are not owned directly by core: ${missingCoreDeviceLogs.join(", ")}`,
  );
  process.exit(1);
}
console.log("devicehub-core owns device log domain values directly.");

const coreDeviceConditions = readFileSync(
  "crates/devicehub-core/src/device_conditions.rs",
  "utf8",
);
const runtimeDeviceConditions = readFileSync(
  "crates/devicehub-runtime/src/device/conditions.rs",
  "utf8",
);
if (
  !coreDeviceConditions.includes("pub struct DeviceConditionSlot") ||
  runtimeDeviceConditions.includes("pub struct DeviceConditionSlot") ||
  runtimeFacade.includes("DeviceConditionSlot") ||
  tauriDeviceLogAdapters.includes("devicehub_runtime::DeviceConditionSlot")
) {
  console.error(
    "Rust boundary check failed: device condition observations are not owned directly by core",
  );
  process.exit(1);
}
console.log("devicehub-core owns device condition observations directly.");

const corePerformance = readFileSync(
  "crates/devicehub-core/src/performance.rs",
  "utf8",
);
const runtimePerformanceSlot = readFileSync(
  "crates/devicehub-runtime/src/performance/slot.rs",
  "utf8",
);
const tauriPerformanceAdapters = [
  readFileSync("crates/devicehub-server/src/http/performance.rs", "utf8"),
  readFileSync("crates/devicehub-server/src/mcp.rs", "utf8"),
].join("\n");
const requiredCorePerformance = [
  "pub enum PerformanceObservation",
  "pub struct PerformanceSlot",
  "pub fn observe(",
  "pub fn energy_targets(",
  "pub fn publish_app_activity(",
];
const missingCorePerformance = requiredCorePerformance.filter(
  (definition) => !corePerformance.includes(definition),
);
if (
  missingCorePerformance.length > 0 ||
  runtimePerformanceSlot.includes("pub struct PerformanceSlot") ||
  runtimeFacade.includes("PerformanceSlot") ||
  tauriPerformanceAdapters.includes("devicehub_runtime::PerformanceSlot") ||
  tauriPerformanceAdapters.includes("devicehub_runtime::{PerformanceDemand, PerformanceSlot}") ||
  !tauriPerformanceAdapters.includes("PerformanceSlot, PerformanceSnapshot")
) {
  console.error(
    `Rust boundary check failed: performance observations are not owned directly by core: ${missingCorePerformance.join(", ")}`,
  );
  process.exit(1);
}
console.log("devicehub-core owns normalized performance observation policy.");

const coreWda = readFileSync(
  "crates/devicehub-core/src/applications/wda.rs",
  "utf8",
);
const runtimeWdaAutomation = readFileSync(
  "crates/devicehub-runtime/src/applications/wda_automation.rs",
  "utf8",
);
const runtimeWdaRunner = readFileSync(
  "crates/devicehub-runtime/src/applications/wda_runner.rs",
  "utf8",
);
const tauriWdaAdapters = [
  readFileSync("crates/devicehub-server/src/mcp.rs", "utf8"),
  readFileSync("src-tauri/src/web.rs", "utf8"),
].join("\n");
const requiredCoreWda = [
  "pub const WDA_MAX_ELEMENTS",
  "pub struct WdaStatus",
  "pub struct WdaDeviceState",
  "pub struct WdaElementDetails",
  "pub struct WdaElementWaitResult",
  "pub struct WdaRunnerStatus",
  "pub enum WdaElementWaitState",
  "pub enum WdaRunnerPhase",
  "pub fn validate_wda_selector",
  "pub fn validate_wda_text",
  "pub fn validate_wda_runner_bundle_id",
];
const missingCoreWda = requiredCoreWda.filter(
  (definition) => !coreWda.includes(definition),
);
const forbiddenRuntimeWdaExports = [
  "WdaBoundedText",
  "WdaDeviceState",
  "WdaElementDetails",
  "WdaElementWaitResult",
  "WdaElementWaitState",
  "WdaOrientation",
  "WdaRect",
  "WdaRunnerPhase",
  "WdaRunnerStatus",
  "WdaSize",
  "WdaStatus",
  "WdaUiTree",
  "WdaUnlockResult",
  "validate_wda_selector",
  "validate_wda_runner_bundle_id",
];
const exposedRuntimeWda = forbiddenRuntimeWdaExports.filter((name) =>
  runtimeFacade.includes(name),
);
if (
  missingCoreWda.length > 0 ||
  exposedRuntimeWda.length > 0 ||
  /pub (struct|enum) Wda(?!AutomationCommand|RunnerCommand)/u.test(
    `${runtimeWdaAutomation}\n${runtimeWdaRunner}`,
  ) ||
  /devicehub_runtime::Wda(?!AutomationCommand|RunnerCommand)/u.test(
    tauriWdaAdapters,
  ) ||
  tauriWdaAdapters.includes("devicehub_runtime::validate_runner_bundle_id") ||
  tauriWdaAdapters.includes("devicehub_runtime::validate_selector") ||
  !runtimeWdaAutomation.includes("use devicehub_core::{") ||
  !runtimeWdaRunner.includes("use devicehub_core::{") ||
  !tauriWdaAdapters.includes("devicehub_core::WdaRunnerStatus") ||
  !tauriWdaAdapters.includes("devicehub_core::validate_wda_selector")
) {
  console.error(
    `Rust boundary check failed: WDA domain values or policy escaped core ownership (missing: ${missingCoreWda.join(", ")}; runtime exports: ${exposedRuntimeWda.join(", ")})`,
  );
  process.exit(1);
}
console.log(
  "devicehub-core owns WDA domain values and policy while runtime owns execution commands.",
);

const publicTransportFacade = runtimeFacade.match(
  /pub use transport::\{([\s\S]*?)\};/,
)?.[1] ?? "";
const forbiddenTransportExports = [
  "CoreTunnelConfig",
  "DeviceDiscovery",
  "SessionEndpoint",
  "UsbmuxdEndpoint",
  "WifiEndpoint",
  "WifiDiscovery",
  "connect_core_tunnel",
  "connect_provider",
  "remove_remote_pairing_credentials",
  "select_preferred_usbmuxd_device",
  "wifi_provider",
];
const exposedTransportInternals = forbiddenTransportExports.filter(
  (name) => publicTransportFacade.includes(name),
);
const runtimeTransport = readFileSync(
  "crates/devicehub-runtime/src/transport.rs",
  "utf8",
);
const runtimeDiscovery = readFileSync(
  "crates/devicehub-runtime/src/transport/discovery.rs",
  "utf8",
);
const tauriSidecar = readFileSync("src-tauri/src/netmuxd.rs", "utf8");
if (
  exposedTransportInternals.length > 0 ||
  runtimeDiscovery.includes("Future<Output = Option<UsbmuxdAddr>>") ||
  tauriSidecar.includes("idevice::usbmuxd::UsbmuxdAddr") ||
  runtimeTransport.includes("pub enum SessionEndpoint") ||
  runtimeTransport.includes("pub struct UsbmuxdEndpoint") ||
  runtimeTransport.includes("pub struct WifiEndpoint") ||
  runtimeDiscovery.includes("pub struct DeviceDiscovery")
) {
  console.error(
    `Rust boundary check failed: runtime transport internals escaped the host port facade: ${exposedTransportInternals.join(", ")}`,
  );
  process.exit(1);
}
console.log("devicehub-runtime transport protocol types stay private.");

const tauriPairingStore = readFileSync("src-tauri/src/wifi_devices.rs", "utf8");
const requiredPairingPort = [
  "pub trait PairingStore",
  "fn load_lockdown_pairings(&self)",
  "fn save_lockdown_pairing(&self, udid: &str, bytes: &[u8])",
  "fn remove_lockdown_pairing(&self, udid: &str)",
  "fn load_remote_pairing(&self, udid: &str)",
  "fn save_remote_pairing(&self, udid: &str, bytes: &[u8])",
  "fn remove_remote_pairing(&self, udid: &str)",
];
const missingPairingPort = requiredPairingPort.filter(
  (signature) => !runtimeTransport.includes(signature),
);
if (
  missingPairingPort.length > 0 ||
  runtimeTransport.includes("use std::path") ||
  runtimeTransport.includes("pairing_dir") ||
  runtimeTransport.includes("RpPairingFile::read_from_file") ||
  runtimeTransport.includes(".write_to_file(") ||
  !runtimeTransport.includes("pub(crate) struct CoreTunnelConfig") ||
  !tauriPairingStore.includes("impl PairingStore for HostPairingStore") ||
  tauriPairingStore.includes("impl RemotePairingStore") ||
  tauriPairingStore.includes("impl WifiPairingStore")
) {
  console.error(
    `Rust boundary check failed: device pairing storage is not unified behind the host port (missing port: ${missingPairingPort.join(", ")})`,
  );
  process.exit(1);
}
console.log("devicehub-runtime pairing storage is unified and host-injected.");

const tauriManifest = readFileSync("src-tauri/Cargo.toml", "utf8");
const tauriBackup = readFileSync("src-tauri/src/device_backup.rs", "utf8");
const runtimeBackup = readFileSync(
  "crates/devicehub-runtime/src/diagnostics/device_backup.rs",
  "utf8",
);
const publicBackupPort = runtimeBackup.match(
  /pub trait DeviceBackupDestination[\s\S]*?\n\}/,
)?.[0] ?? "";
const forbiddenBackupPortTypes = [
  "MobileBackup2Client",
  "BackupDelegate",
  "IdeviceError",
];
const leakedBackupTypes = forbiddenBackupPortTypes.filter((name) =>
  publicBackupPort.includes(name),
);
const retainedTauriBackupProtocol = forbiddenBackupPortTypes.filter((name) =>
  tauriBackup.includes(name),
);
if (
  /(^|\n)idevice\s*=/.test(tauriManifest) ||
  leakedBackupTypes.length > 0 ||
  retainedTauriBackupProtocol.length > 0 ||
  !runtimeBackup.includes("impl BackupDelegate for ConfinedBackupDelegate") ||
  !runtimeBackup.includes(".backup_from_path(") ||
  !tauriBackup.includes("impl devicehub_runtime::DeviceBackupDestination")
) {
  console.error(
    `Rust boundary check failed: MobileBackup2 ownership escaped runtime (public port: ${leakedBackupTypes.join(", ")}; Tauri: ${retainedTauriBackupProtocol.join(", ")})`,
  );
  process.exit(1);
}
console.log("devicehub-runtime owns MobileBackup2 behind a host destination port.");

const tauriSessionComposition = readFileSync(
  "src-tauri/src/session/manager.rs",
  "utf8",
);
const tauriFacadeRoot = readFileSync("src-tauri/src/lib.rs", "utf8");
const runtimeSessionFacade = runtimeFacade.match(
  /pub use session::\{([\s\S]*?)\n\};/,
)?.[1] ?? "";
const forbiddenSessionExports = [
  "ConnectedSessionHost",
  "ConnectedSessionMedia",
  "ConnectedSessionViews",
  "DeviceManagementBootstrap",
  "DeviceManagementSession",
  "DeviceServicePorts",
  "DeviceSessionRouter",
  "LocationServicePort",
  "OrientationWatcher",
  "PairingCredentialStore",
  "RuntimeHostServiceViews",
  "RuntimeServiceViews",
  "SessionFailureAction",
  "SessionManagerHost",
  "SessionManagerViews",
  "SessionRetry",
  "SessionRetryPolicy",
  "connect_device_input",
  "run_device_command_loop",
  "run_management_command_loop",
  "run_session_manager",
  "supervise_heartbeat",
];
const exposedSessionInternals = forbiddenSessionExports.filter((name) =>
  runtimeSessionFacade.includes(name),
);
const requiredRuntimeState = [
  "pub(crate) struct RuntimeManagerState",
  "pub(crate) struct DeviceSessionState<HostPath>",
  "pub(crate) struct CoreRuntimeState<HostPath>",
  "pub(crate) manager: RuntimeManagerState",
  "pub(crate) device: DeviceSessionState<HostPath>",
  "pub(crate) fn client(",
  "pub(crate) fn manager_views(&self)",
  "RuntimeServiceViews {",
  "RuntimeHostServiceViews {",
  "SessionManagerViews {",
];
const missingRuntimeState = requiredRuntimeState.filter(
  (signature) => !runtimeOwner.includes(signature),
);
const runtimeStateFacade = runtimeOwner.match(
  /pub\(crate\) struct CoreRuntimeState<HostPath> \{([\s\S]*?)\n\}/u,
)?.[1] ?? "";
const forbiddenFlatRuntimeState = [
  "pub(crate) devices:",
  "pub(crate) active:",
  "pub(crate) status:",
  "pub(crate) commands:",
].filter((signature) => runtimeStateFacade.includes(signature));
const forbiddenTauriStateConstruction = [
  "StatusSlot::default()",
  "BrowserVideoSlot::default()",
  "PerformanceSlot::default()",
  "RuntimeServiceViews {",
  "RuntimeHostServiceViews {",
  "SessionManagerViews {",
  "DeviceControlService::new(",
  "RuntimeClient {",
];
const duplicatedTauriState = forbiddenTauriStateConstruction.filter(
  (signature) =>
    tauriRuntimeOwner.includes(signature) ||
    tauriSessionComposition.includes(signature),
);
if (
  missingRuntimeState.length > 0 ||
  forbiddenFlatRuntimeState.length > 0 ||
  duplicatedTauriState.length > 0 ||
  exposedSessionInternals.length > 0 ||
  existsSync("src-tauri/src/application.rs") ||
  existsSync("src-tauri/src/domain.rs") ||
  existsSync("src-tauri/src/protocol.rs") ||
  existsSync("src-tauri/src/device_runtime/commands.rs") ||
  existsSync("src-tauri/src/device_runtime/state.rs") ||
  tauriFacadeRoot.includes("mod application;") ||
  tauriFacadeRoot.includes("mod domain;") ||
  tauriFacadeRoot.includes("mod protocol;") ||
  runtimeFacade.includes("CoreRuntimeState") ||
  tauriRuntimeOwner.includes("CoreRuntimeState") ||
  tauriRuntimeOwner.includes("RuntimeServices") ||
  tauriRuntimeOwner.includes("from_state(") ||
  !runtimeSessionManager.includes("CoreRuntimeState::<Files::Path>::default()") ||
  !runtimeSessionManager.includes("let client = state.client(control);") ||
  !runtimeSessionManager.includes("CoreRuntime::start(") ||
  !tauriSessionComposition.includes("devicehub_runtime::start_runtime(") ||
  !tauriSessionComposition.includes("RuntimeHostAdapters {") ||
  !tauriSessionComposition.includes("pairing_store,") ||
  !tauriSessionComposition.includes("system_usbmuxd: transport.system_usbmuxd") ||
  !runtimeSessionManager.includes(
    "CoreTunnelConfig::new(pairing_store.clone(), system_usbmuxd)",
  ) ||
  !runtimeSessionManager.includes("pairing_store.map(Arc::new)") ||
  tauriSessionComposition.includes("CoreTunnelConfig") ||
  tauriSessionComposition.includes("SessionManager") ||
  tauriSessionComposition.includes(".run(") ||
  tauriSessionComposition.includes("state.manager_views()")
) {
  console.error(
    `Rust boundary check failed: shared runtime state or session API ownership drifted (runtime missing: ${missingRuntimeState.join(", ")}; flat runtime state: ${forbiddenFlatRuntimeState.join(", ")}; Tauri duplicated: ${duplicatedTauriState.join(", ")}; exposed session internals: ${exposedSessionInternals.join(", ")})`,
  );
  process.exit(1);
}
console.log("devicehub-runtime owns the single shared runtime state graph.");

const runtimeClient = readFileSync(
  "crates/devicehub-runtime/src/client.rs",
  "utf8",
);
const runtimeClientFacade = runtimeClient.match(
  /pub struct RuntimeClient<HostPath> \{([\s\S]*?)\n\}/u,
)?.[1] ?? "";
const requiredRuntimeClientState = [
  "pub struct RuntimeManagerClient",
  "pub struct DeviceSessionClient<HostPath>",
  "pub manager: RuntimeManagerClient",
  "pub device: DeviceSessionClient<HostPath>",
  "pub browser_frames: BrowserVideoSlot",
  "pub video_counters: VideoCounters",
  "pub clipboard: ClipboardSlot",
  "pub network_capture: NetworkCaptureSlot",
  "pub bluetooth_capture: BluetoothCaptureSlot",
  "pub device_backup: DeviceBackupSlot",
  "pub sysdiagnose: SysdiagnoseSlot",
  "pub log_archive: LogArchiveSlot",
  "pub developer_image: DeveloperImageMountSlot",
  "pub app_operation: AppOperationSlot",
  "pub app_documents: AppDocumentActivitySlot",
  "pub device_files: DeviceFileActivitySlot",
  "pub service_registry: ServiceRegistry",
  "pub commands: SessionCommandSlot<HostPath>",
];
const missingRuntimeClientState = requiredRuntimeClientState.filter(
  (signature) => !runtimeClient.includes(signature),
);
const forbiddenFlatRuntimeClientState = [
  "pub devices:",
  "pub active:",
  "pub control:",
  "pub status:",
  "pub commands:",
].filter((signature) => runtimeClientFacade.includes(signature));
if (
  missingRuntimeClientState.length > 0 ||
  forbiddenFlatRuntimeClientState.length > 0
) {
  console.error(
    `Rust boundary check failed: RuntimeClient manager/session ownership drifted (missing: ${missingRuntimeClientState.join(", ")}; flat: ${forbiddenFlatRuntimeClientState.join(", ")})`,
  );
  process.exit(1);
}
console.log("devicehub-runtime RuntimeClient separates manager and device-session ownership.");

const publicMediaFacade = runtimeFacade.match(
  /pub use media::\{([\s\S]*?)\n\};/,
)?.[1] ?? "";
const forbiddenMediaExports = [
  "AccessUnitAssembler",
  "HEVC_QUEUE_MAX_BYTES",
  "HevcAccessUnit",
  "HevcQueue",
  "HevcQueuePush",
  "HevcQueueSnapshot",
  "MediaSessionConfig",
  "MediaSessionRuntime",
  "RtcpShared",
  "RtpVideoClock",
  "RunningStats",
  "ScreenMediaStream",
  "VideoRtpOptions",
  "forward_keyframe_requests",
  "hevc_dimensions",
  "publish_hevc_queue",
  "receive_audio_rtp",
  "receive_rtcp",
  "receive_video_rtp",
  "send_rtcp",
  "stall_watchdog",
  "start_screen_media_stream",
];
const exposedMediaInternals = forbiddenMediaExports.filter((name) =>
  publicMediaFacade.includes(name),
);
const publicClipboardFacade = runtimeFacade.match(
  /pub use clipboard::\{([\s\S]*?)\};/,
)?.[1] ?? "";
const forbiddenClipboardExports = [
  "ClipboardBridge",
  "DeviceClipboardSession",
  "HostClipboardFactory",
  "connect_device_clipboard",
];
const exposedClipboardInternals = forbiddenClipboardExports.filter((name) =>
  publicClipboardFacade.includes(name),
);
if (exposedMediaInternals.length > 0 || exposedClipboardInternals.length > 0) {
  console.error(
    `Rust boundary check failed: connected-session implementation escaped the runtime facade (media: ${exposedMediaInternals.join(", ")}; clipboard: ${exposedClipboardInternals.join(", ")})`,
  );
  process.exit(1);
}
console.log("devicehub-runtime media and clipboard session internals stay private.");

const runtimeAudioPort = readFileSync(
  "crates/devicehub-runtime/src/audio.rs",
  "utf8",
);
const tauriAudioAdapter = readFileSync("src-tauri/src/decode.rs", "utf8");
const requiredAudioSourcePort = [
  "pub struct DeviceAudioSource",
  "pub async fn drain(&self)",
  "pub async fn forward_rtp_to_local_port(&self, port: u16)",
  "pub async fn drain_for(&self, delay: Duration)",
  "fn run(&self, source: DeviceAudioSource)",
];
const missingAudioSourcePort = requiredAudioSourcePort.filter(
  (signature) => !runtimeAudioPort.includes(signature),
);
if (
  missingAudioSourcePort.length > 0 ||
  runtimeAudioPort.includes("fn run(&self, udp: UdpSocketHandle)") ||
  runtimeAudioPort.includes("pub udp: UdpSocketHandle") ||
  tauriAudioAdapter.includes("idevice::tcp::handle::UdpSocketHandle") ||
  tauriAudioAdapter.includes("receive_audio_rtp")
) {
  console.error(
    `Rust boundary check failed: audio host port exposes Apple transport ownership (missing source API: ${missingAudioSourcePort.join(", ")})`,
  );
  process.exit(1);
}
console.log("devicehub-runtime owns audio transport behind a host-neutral source port.");

const publicApplicationFacade = runtimeFacade.match(
  /pub use applications::\{([\s\S]*?)\n\};/,
)?.[1] ?? "";
const publicDeviceFacade = runtimeFacade.match(
  /pub use device::\{([\s\S]*?)\n\};/,
)?.[1] ?? "";
const publicPerformanceFacade = runtimeFacade.match(
  /pub use performance::\{([\s\S]*?)\};/,
)?.[1] ?? "";
const forbiddenProtocolExecutors = [
  "DevicePowerController",
  "ScreenCaptureTransport",
  "delete_crash_report",
  "download_crash_report",
  "execute_developer_mode",
  "is_developer_image_mounted",
  "is_developer_image_mounted_for_device",
  "list_crash_reports",
  "read_activation_state",
  "read_crash_report",
  "read_developer_mode_status",
  "read_device_battery",
  "read_device_details",
  "read_device_developer_mode_status",
  "read_device_product_version",
  "rename_device",
  "serve_app_console",
  "serve_app_icons",
  "serve_app_lifecycle",
  "serve_companion_devices",
  "serve_home_screen",
  "serve_running_processes",
  "serve_screen_capture",
  "serve_wda_automation",
  "serve_wda_runner",
  "supervise_device_conditions",
  "supervise_device_events",
  "supervise_device_logs",
  "supervise_location",
  "supervise_performance_app_activity",
  "supervise_performance_energy",
  "supervise_performance_graphics",
  "supervise_performance_network",
  "supervise_performance_system",
];
const publicProtocolFacade = [
  publicApplicationFacade,
  publicDeviceFacade,
  publicPerformanceFacade,
].join("\n");
const exposedProtocolExecutors = forbiddenProtocolExecutors.filter((name) =>
  publicProtocolFacade.includes(name),
);
if (exposedProtocolExecutors.length > 0) {
  console.error(
    `Rust boundary check failed: raw device protocol executors escaped the runtime facade: ${exposedProtocolExecutors.join(", ")}`,
  );
  process.exit(1);
}
console.log("devicehub-runtime raw device protocol executors stay private.");

const coreStorage = readFileSync(
  "crates/devicehub-core/src/storage.rs",
  "utf8",
);
const runtimeStorageFacade = runtimeFacade.match(
  /pub use storage::\{([\s\S]*?)\n\};/,
)?.[1] ?? "";
const requiredCoreStoragePolicy = [
  "pub struct DeviceFileEntry",
  "pub struct DeviceFileActivitySlot",
  "pub struct AppDocumentEntry",
  "pub struct AppDocumentActivitySlot",
  "pub fn normalize_device_file_path(",
  "pub fn normalize_app_document_path(",
  "pub fn validate_app_bundle_id(",
];
const missingCoreStoragePolicy = requiredCoreStoragePolicy.filter(
  (signature) => !coreStorage.includes(signature),
);
const forbiddenRuntimeStorageDomainExports = [
  "AppDocumentActivityKind",
  "AppDocumentActivitySlot",
  "AppDocumentActivityState",
  "AppDocumentActivityView",
  "AppDocumentEntry",
  "AppDocumentKind",
  "AppDocumentList",
  "AppDocumentTransfer",
  "AppStorageScope",
  "DeviceFileActivityKind",
  "DeviceFileActivitySlot",
  "DeviceFileActivityState",
  "DeviceFileActivityView",
  "DeviceFileEntry",
  "DeviceFileKind",
  "DeviceFileList",
  "DeviceFileTransfer",
];
const leakedRuntimeStorageDomain = forbiddenRuntimeStorageDomainExports.filter(
  (name) => runtimeStorageFacade.includes(name),
);
const serverStorageAdapter = readFileSync(
  "crates/devicehub-server/src/http/storage.rs",
  "utf8",
);
if (
  missingCoreStoragePolicy.length > 0 ||
  leakedRuntimeStorageDomain.length > 0 ||
  !serverStorageAdapter.includes("use devicehub_core::{") ||
  !serverStorageAdapter.includes("AppDocumentActivitySlot") ||
  !serverStorageAdapter.includes("DeviceFileActivitySlot") ||
  serverStorageAdapter.includes("devicehub_runtime::AppDocumentActivity") ||
  serverStorageAdapter.includes("devicehub_runtime::DeviceFileActivity")
) {
  console.error(
    `Rust boundary check failed: storage domain ownership drifted (core missing: ${missingCoreStoragePolicy.join(", ")}; runtime leaked: ${leakedRuntimeStorageDomain.join(", ")})`,
  );
  process.exit(1);
}
console.log(
  "devicehub-core owns AFC and application-storage domain models and policy.",
);
