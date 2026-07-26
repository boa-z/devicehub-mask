import { spawnSync } from "node:child_process";

const usage = "Usage: npm run verify:device -- --udid <UDID>";
const args = process.argv.slice(2);
let udid;

for (let index = 0; index < args.length; index += 1) {
  if (args[index] !== "--udid" || udid !== undefined || !args[index + 1]) {
    console.error(usage);
    process.exit(2);
  }
  udid = args[index + 1].trim();
  index += 1;
}

if (!udid) {
  console.error(usage);
  process.exit(2);
}

const ideviceId = process.platform === "win32" ? "idevice_id.exe" : "idevice_id";
const discovery = spawnSync(ideviceId, ["-l"], {
  cwd: process.cwd(),
  encoding: "utf8",
  stdio: ["ignore", "pipe", "pipe"],
});

if (discovery.error) {
  console.error(`Unable to run ${ideviceId}: ${discovery.error.message}`);
  console.error("Install libimobiledevice and make idevice_id available on PATH.");
  process.exit(1);
}
if (discovery.status !== 0) {
  process.stderr.write(discovery.stderr);
  console.error(`${ideviceId} failed with exit code ${discovery.status ?? "unknown"}.`);
  process.exit(discovery.status ?? 1);
}

const usbUdids = discovery.stdout
  .split(/\r?\n/u)
  .map((value) => value.trim())
  .filter(Boolean);

if (usbUdids.length !== 1) {
  console.error(`Expected exactly one USB device, found ${usbUdids.length}.`);
  console.error("Disconnect other USB devices and retry with the intended device connected.");
  process.exit(1);
}
if (usbUdids[0] !== udid) {
  console.error(`Connected USB device ${usbUdids[0]} does not match requested UDID ${udid}.`);
  process.exit(1);
}

const tests = [
  "session::heartbeat::tests::acknowledges_heartbeat_from_hardware",
  "device::details::tests::reads_developer_mode_status_from_hardware",
  "device::screenshot::tests::captures_native_screenshot_from_hardware",
  "device::provisioning::tests::lists_profiles_over_rsd_from_hardware",
  "device::logs::tests::reads_syslog_from_hardware",
  "storage::public::tests::lists_public_afc_root_from_hardware",
  "applications::icons::tests::reads_app_icon_sources_from_hardware",
  "performance::tests::inspects_sysmontap_process_schema_from_hardware",
];
const cargo = process.platform === "win32" ? "cargo.exe" : "cargo";
const env = { ...process.env, DEVICEHUB_TEST_UDID: udid };

console.log(`Verified one USB device with UDID ${udid}.`);
console.log("Running read-only hardware checks. The desktop application will not be launched.");

for (const test of tests) {
  console.log(`\n==> ${test}`);
  const result = spawnSync(
    cargo,
    [
      "test",
      "--manifest-path",
      "Cargo.toml",
      "-p",
      "devicehub-runtime",
      "--lib",
      "--locked",
      test,
      "--",
      "--ignored",
      "--exact",
      "--nocapture",
      "--test-threads=1",
    ],
    { cwd: process.cwd(), env, stdio: "inherit" },
  );
  if (result.error) {
    console.error(`Unable to start ${test}: ${result.error.message}`);
    process.exit(1);
  }
  if (result.status !== 0) {
    console.error(`${test} failed with exit code ${result.status ?? "unknown"}.`);
    process.exit(result.status ?? 1);
  }
}

console.log("\nRead-only physical-device verification passed.");
