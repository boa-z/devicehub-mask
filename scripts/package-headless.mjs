import { createHash } from "node:crypto";
import {
  chmodSync,
  cpSync,
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { basename, join, resolve, sep } from "node:path";
import { spawnSync } from "node:child_process";

const args = process.argv.slice(2);
const target = valueAfter(args, "--target") ?? hostTarget();
const version = requiredValue(args, "--version");
const buildNumber = requiredValue(args, "--build-number");
const skipFrontendBuild = args.includes("--skip-frontend-build");
const skipSidecars = args.includes("--skip-sidecars");
const suppliedBinary = valueAfter(args, "--binary");
const descriptor = targetDescriptor(target);
const packageName = `devicehub-mask-headless_${version}+${buildNumber}_${descriptor.label}`;
const workRoot = resolve("release-artifacts");
const outputRoot = resolve(valueAfter(args, "--output-dir") ?? "release-artifacts");
const stagingRoot = join(workRoot, "staging");
const packageRoot = join(stagingRoot, packageName);
const archive = join(outputRoot, `${packageName}.${descriptor.archiveExtension}`);

assertContained(workRoot, stagingRoot);
assertContained(outputRoot, archive);
rmSync(stagingRoot, { recursive: true, force: true });
mkdirSync(packageRoot, { recursive: true });
mkdirSync(outputRoot, { recursive: true });

if (!skipFrontendBuild) runNpm(["run", "build"]);
if (!skipSidecars) {
  run(process.execPath, ["scripts/prepare-netmuxd.mjs", "--target", target]);
  run(process.execPath, ["scripts/prepare-ffmpeg.mjs", "--target", target]);
}

const binary = suppliedBinary
  ? resolve(suppliedBinary)
  : buildHeadless(target, descriptor);
requireFile(binary, "headless executable");
requireFile("dist/index.html", "built browser UI");

const executableName = descriptor.windows ? "devicehub-headless.exe" : "devicehub-headless";
cpSync(binary, join(packageRoot, executableName));
if (!descriptor.windows) chmodSync(join(packageRoot, executableName), 0o755);
copyFrontend(join(packageRoot, "dist"));
copyResource(descriptor.windows ? "ffmpeg.exe" : "ffmpeg", packageRoot, true);
copyResource(descriptor.windows ? "netmuxd.exe" : "netmuxd", packageRoot, true);
copyResource("ffmpeg-LICENSE.txt", packageRoot, false);
copyResource("netmuxd-LICENSE.txt", packageRoot, false);
copyResource("THIRD_PARTY_NOTICES.txt", packageRoot, false);
cpSync("docs/en/headless.md", join(packageRoot, "README.md"));
cpSync("docs/zh-CN/headless.md", join(packageRoot, "README.zh-CN.md"));

rmSync(archive, { force: true });
if (descriptor.windows) {
  run(
    "powershell.exe",
    [
      "-NoProfile",
      "-NonInteractive",
      "-Command",
      "Compress-Archive -LiteralPath $env:DEVICEHUB_HEADLESS_STAGE -DestinationPath $env:DEVICEHUB_HEADLESS_ARCHIVE -CompressionLevel Optimal",
    ],
    {
      DEVICEHUB_HEADLESS_STAGE: packageRoot,
      DEVICEHUB_HEADLESS_ARCHIVE: archive,
    },
  );
} else {
  run("tar", ["-czf", archive, "-C", stagingRoot, packageName]);
}

requireFile(archive, "headless archive");
const digest = createHash("sha256").update(readFileSync(archive)).digest("hex");
writeFileSync(`${archive}.sha256`, `${digest}  ${basename(archive)}\n`, "ascii");
rmSync(stagingRoot, { recursive: true, force: true });
console.log(`Headless package: ${archive}`);
console.log(`SHA-256: ${digest}`);

function buildHeadless(selectedTarget, selectedDescriptor) {
  if (selectedDescriptor.universalMac) {
    const armTarget = "aarch64-apple-darwin";
    const intelTarget = "x86_64-apple-darwin";
    run("cargo", ["build", "-p", "devicehub-headless", "--release", "--locked", "--target", armTarget]);
    run("cargo", ["build", "-p", "devicehub-headless", "--release", "--locked", "--target", intelTarget]);
    const output = join(stagingRoot, "devicehub-headless-universal");
    run("lipo", [
      "-create",
      rustBinary(armTarget, false),
      rustBinary(intelTarget, false),
      "-output",
      output,
    ]);
    const architectures = capture("lipo", ["-archs", output]).trim().split(/\s+/u).sort();
    if (architectures.join(" ") !== "arm64 x86_64") {
      throw new Error(`Universal headless executable has unexpected architectures: ${architectures.join(" ")}`);
    }
    return output;
  }
  if (selectedTarget === hostTarget()) {
    run("cargo", ["build", "-p", "devicehub-headless", "--release", "--locked"]);
    return resolve(
      "src-tauri",
      "target",
      "release",
      selectedDescriptor.windows ? "devicehub-headless.exe" : "devicehub-headless",
    );
  }
  run("cargo", ["build", "-p", "devicehub-headless", "--release", "--locked", "--target", selectedTarget]);
  return rustBinary(selectedTarget, selectedDescriptor.windows);
}

function rustBinary(selectedTarget, windows) {
  return resolve(
    "src-tauri",
    "target",
    selectedTarget,
    "release",
    windows ? "devicehub-headless.exe" : "devicehub-headless",
  );
}

function copyResource(name, destination, executable) {
  const source = join("src-tauri", "resources", name);
  requireFile(source, `bundled resource ${name}`);
  const output = join(destination, name);
  cpSync(source, output);
  if (executable && !name.endsWith(".exe")) chmodSync(output, 0o755);
}

function copyFrontend(destination) {
  mkdirSync(destination, { recursive: true });
  cpSync("dist/index.html", join(destination, "index.html"));
  requireFile("dist/assets", "built browser assets");
  cpSync("dist/assets", join(destination, "assets"), { recursive: true });
  if (existsSync("dist/.vite")) {
    cpSync("dist/.vite", join(destination, ".vite"), { recursive: true });
  }
}

function requireFile(path, label) {
  if (!existsSync(path)) throw new Error(`${label} is missing: ${path}`);
}

function runNpm(commandArgs) {
  const npmCli = process.env.npm_execpath;
  if (npmCli) run(process.execPath, [npmCli, ...commandArgs]);
  else run(process.platform === "win32" ? "npm.cmd" : "npm", commandArgs);
}

function run(command, commandArgs, extraEnv = {}) {
  const result = spawnSync(command, commandArgs, {
    cwd: process.cwd(),
    env: { ...process.env, ...extraEnv },
    stdio: "inherit",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}

function capture(command, commandArgs) {
  const result = spawnSync(command, commandArgs, {
    cwd: process.cwd(),
    env: process.env,
    encoding: "utf8",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    process.stderr.write(result.stderr ?? "");
    process.exit(result.status ?? 1);
  }
  return result.stdout;
}

function targetDescriptor(selectedTarget) {
  if (selectedTarget === "universal-apple-darwin") {
    return { label: "macos-universal", archiveExtension: "tar.gz", universalMac: true, windows: false };
  }
  if (selectedTarget === "aarch64-apple-darwin") {
    return { label: "macos-arm64", archiveExtension: "tar.gz", universalMac: false, windows: false };
  }
  if (selectedTarget === "x86_64-apple-darwin") {
    return { label: "macos-x64", archiveExtension: "tar.gz", universalMac: false, windows: false };
  }
  if (selectedTarget === "x86_64-pc-windows-msvc") {
    return { label: "windows-x64", archiveExtension: "zip", universalMac: false, windows: true };
  }
  if (selectedTarget === "x86_64-unknown-linux-gnu") {
    return { label: "linux-x64", archiveExtension: "tar.gz", universalMac: false, windows: false };
  }
  if (selectedTarget === "aarch64-unknown-linux-gnu") {
    return { label: "linux-arm64", archiveExtension: "tar.gz", universalMac: false, windows: false };
  }
  throw new Error(`Unsupported headless package target: ${selectedTarget}`);
}

function assertContained(parent, child) {
  const root = resolve(parent);
  const candidate = resolve(child);
  if (candidate !== root && !candidate.startsWith(`${root}${sep}`)) {
    throw new Error(`Refusing to write outside ${root}: ${candidate}`);
  }
}

function requiredValue(values, flag) {
  const value = valueAfter(values, flag);
  if (!value) throw new Error(`${flag} is required`);
  return value;
}

function valueAfter(values, flag) {
  const index = values.indexOf(flag);
  if (index < 0) return undefined;
  const value = values[index + 1];
  if (!value || value.startsWith("--")) throw new Error(`${flag} requires a value`);
  return value;
}

function hostTarget() {
  const arch = process.arch === "arm64" ? "aarch64" : process.arch === "x64" ? "x86_64" : process.arch;
  if (process.platform === "darwin") return `${arch}-apple-darwin`;
  if (process.platform === "win32") return `${arch}-pc-windows-msvc`;
  if (process.platform === "linux") return `${arch}-unknown-linux-gnu`;
  throw new Error(`Unsupported host: ${process.platform}/${process.arch}`);
}
