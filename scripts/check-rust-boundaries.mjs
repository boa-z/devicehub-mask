import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";

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
  !tauriSessionManager.includes("SessionManager::new(") ||
  !tauriSessionManager.includes(".run(")
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
  "pub struct SessionManager",
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

const runtimeFacade = readFileSync(
  "crates/devicehub-runtime/src/lib.rs",
  "utf8",
);
const publicTransportFacade = runtimeFacade.match(
  /pub use transport::\{([\s\S]*?)\};/,
)?.[1] ?? "";
const forbiddenTransportExports = [
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
const requiredRemotePairingPort = [
  "pub trait RemotePairingStore",
  "fn load_remote_pairing(&self, udid: &str)",
  "fn save_remote_pairing(&self, udid: &str, bytes: &[u8])",
  "fn remove_remote_pairing(&self, udid: &str)",
];
const missingRemotePairingPort = requiredRemotePairingPort.filter(
  (signature) => !runtimeTransport.includes(signature),
);
if (
  missingRemotePairingPort.length > 0 ||
  runtimeTransport.includes("use std::path") ||
  runtimeTransport.includes("pairing_dir") ||
  runtimeTransport.includes("RpPairingFile::read_from_file") ||
  runtimeTransport.includes(".write_to_file(") ||
  !tauriPairingStore.includes("impl RemotePairingStore for HostPairingStore")
) {
  console.error(
    `Rust boundary check failed: CoreDevice remote pairing storage is not host-injected (missing port: ${missingRemotePairingPort.join(", ")})`,
  );
  process.exit(1);
}
console.log("devicehub-runtime remote pairing storage is host-injected.");

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
  "pub struct CoreRuntimeState<HostPath>",
  "pub fn client(",
  "pub(crate) fn manager_views(&self)",
  "RuntimeServiceViews {",
  "RuntimeHostServiceViews {",
  "SessionManagerViews {",
];
const missingRuntimeState = requiredRuntimeState.filter(
  (signature) => !runtimeOwner.includes(signature),
);
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
  !tauriRuntimeOwner.includes("CoreRuntimeState::<PathBuf>::default()") ||
  !tauriRuntimeOwner.includes("state.client(control.clone())") ||
  !tauriSessionComposition.includes("SessionManager::new(") ||
  !tauriSessionComposition.includes(".run(") ||
  !tauriSessionComposition.includes("        state,") ||
  tauriSessionComposition.includes("state.manager_views()")
) {
  console.error(
    `Rust boundary check failed: shared runtime state or session API ownership drifted (runtime missing: ${missingRuntimeState.join(", ")}; Tauri duplicated: ${duplicatedTauriState.join(", ")}; exposed session internals: ${exposedSessionInternals.join(", ")})`,
  );
  process.exit(1);
}
console.log("devicehub-runtime owns the single shared runtime state graph.");

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
