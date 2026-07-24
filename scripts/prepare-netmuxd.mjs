import { createHash } from "node:crypto";
import { chmod, cp, mkdir, mkdtemp, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";
import { spawnSync } from "node:child_process";

const version = "v0.4.3";
const releaseBase = `https://github.com/jkcoxson/netmuxd/releases/download/${version}`;
const licenseUrl = `https://raw.githubusercontent.com/jkcoxson/netmuxd/${version}/LICENSE`;
const licenseSha256 = "596c3c0e8bea14135aa214d23b81d39d0417ec435cf76f3d8f6849dda50db307";
const assets = {
  "aarch64-apple-darwin": ["netmuxd-aarch64-apple-darwin.tar.gz", "b3a42aaaf6eef497ed1db24d71187f5a064a5670e9139335c521beb74ef7c0bd"],
  "x86_64-apple-darwin": ["netmuxd-x86_64-apple-darwin.tar.gz", "30e5d2fd91c1c0c3327894f496e9f8b78cc420b4b44b88d8eeeff2de9b07720e"],
  "aarch64-pc-windows-msvc": ["netmuxd-aarch64-pc-windows-msvc.zip", "42df659a204bb2fa3396b4387c4d3ac1e2019390d23aa29131360a4056b22761"],
  "x86_64-pc-windows-msvc": ["netmuxd-x86_64-pc-windows-msvc.zip", "852936985ff213a6233e523a574a780c56a8debcf49b9053e422b8a1a1bfa657"],
  "aarch64-unknown-linux-gnu": ["netmuxd-aarch64-unknown-linux-gnu.tar.gz", "a89d587b094184b41431bc343da6dfa890391ccc7d4b3d6bce02b524f3191ae6"],
  "x86_64-unknown-linux-gnu": ["netmuxd-x86_64-unknown-linux-gnu.tar.gz", "85b6598284fc639f2a282584461d05e2090b79bdf3ec949d2a5e5d3dc655dde4"],
};

const requested = valueAfter("--target") ?? hostTarget();
const resourceDir = join(process.cwd(), "src-tauri", "resources");
const output = join(resourceDir, requested.includes("windows") ? "netmuxd.exe" : "netmuxd");
const incompatibleOutput = join(resourceDir, requested.includes("windows") ? "netmuxd" : "netmuxd.exe");
const work = await mkdtemp(join(tmpdir(), "devicehub-netmuxd-"));

try {
  await mkdir(resourceDir, { recursive: true });
  await rm(incompatibleOutput, { force: true });
  if (requested === "universal-apple-darwin") {
    const arm = await fetchBinary("aarch64-apple-darwin", join(work, "arm64"));
    const intel = await fetchBinary("x86_64-apple-darwin", join(work, "x86_64"));
    run("lipo", ["-create", arm, intel, "-output", output]);
  } else {
    const binary = await fetchBinary(requested, join(work, requested));
    await cp(binary, output);
  }
  if (!requested.includes("windows")) await chmod(output, 0o755);

  const license = await download(licenseUrl, licenseSha256);
  await writeFile(join(resourceDir, "netmuxd-LICENSE.txt"), license);
  console.log(`Prepared netmuxd ${version} for ${requested}: ${output}`);
} finally {
  await rm(work, { recursive: true, force: true });
}

async function fetchBinary(target, directory) {
  const asset = assets[target];
  if (!asset) throw new Error(`Unsupported netmuxd target: ${target}`);
  await mkdir(directory, { recursive: true });
  const archive = await download(`${releaseBase}/${asset[0]}`, asset[1]);
  const archivePath = join(directory, asset[0]);
  await writeFile(archivePath, archive);
  run("tar", ["-xf", archivePath, "-C", directory]);
  const binaryName = target.includes("windows") ? "netmuxd.exe" : "netmuxd";
  const binary = await findFile(directory, binaryName);
  if (!binary) throw new Error(`${asset[0]} did not contain ${binaryName}`);
  return binary;
}

async function download(url, expected) {
  const response = await fetch(url, { redirect: "follow" });
  if (!response.ok) throw new Error(`Download failed (${response.status}): ${url}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  const actual = createHash("sha256").update(bytes).digest("hex");
  if (actual !== expected) throw new Error(`Checksum mismatch for ${basename(url)}: ${actual}`);
  return bytes;
}

async function findFile(directory, name) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isFile() && entry.name === name) return path;
    if (entry.isDirectory()) {
      const nested = await findFile(path, name);
      if (nested) return nested;
    }
  }
  return undefined;
}

function run(command, args) {
  const result = spawnSync(command, args, { stdio: "inherit" });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command} exited with ${result.status}`);
}

function valueAfter(flag) {
  const index = process.argv.indexOf(flag);
  return index < 0 ? undefined : process.argv[index + 1];
}

function hostTarget() {
  const arch = process.arch === "arm64" ? "aarch64" : process.arch === "x64" ? "x86_64" : process.arch;
  if (process.platform === "darwin") return `${arch}-apple-darwin`;
  if (process.platform === "win32") return `${arch}-pc-windows-msvc`;
  if (process.platform === "linux") return `${arch}-unknown-linux-gnu`;
  throw new Error(`Unsupported host: ${process.platform}/${process.arch}`);
}
