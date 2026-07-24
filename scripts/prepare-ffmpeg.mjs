import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { access, chmod, cp, mkdir, mkdtemp, readdir, rm, writeFile } from "node:fs/promises";
import { cpus, tmpdir } from "node:os";
import { basename, join } from "node:path";

const ffmpegVersion = "8.1.2";
const sourceUrl = `https://ffmpeg.org/releases/ffmpeg-${ffmpegVersion}.tar.xz`;
const sourceSha256 = "464beb5e7bf0c311e68b45ae2f04e9cc2af88851abb4082231742a74d97b524c";
const btbTag = "autobuild-2026-07-23-14-16";
const btbBase = `https://github.com/BtbN/FFmpeg-Builds/releases/download/${btbTag}`;
const btbVersion = "n8.1.2-30-g45f1910444";
const assets = {
  "x86_64-pc-windows-msvc": [`ffmpeg-${btbVersion}-win64-lgpl-8.1.zip`, "0c6dc759eb1c70804ca04e11d83c583a6b03ae0630474f862e97400c715c6376"],
  "aarch64-pc-windows-msvc": [`ffmpeg-${btbVersion}-winarm64-lgpl-8.1.zip`, "679ab26bcaec11516c76b2805335208923826ec8ca410f51b4fd17928d30a8df"],
  "x86_64-unknown-linux-gnu": [`ffmpeg-${btbVersion}-linux64-lgpl-8.1.tar.xz`, "759149752aab56335d3234f82e118c6f7d441e5b635467ff8bff307729b02e6b"],
  "aarch64-unknown-linux-gnu": [`ffmpeg-${btbVersion}-linuxarm64-lgpl-8.1.tar.xz`, "c9c7653babc8ae7191100fc570703fb6a4e7fe11a1696da6e94fbf3b71a5a4a6"],
};
const licenseFiles = [
  ["LICENSE.md", "2e1d16c72fd74e12063776371da757322f8b77589386532f4fd8634bde7de1af"],
  ["COPYING.LGPLv2.1", "246041b6ecf9bc32d718a62c57877c78b5eb397b6467e74ed7ae2626ab189c30"],
];

const requested = valueAfter("--target") ?? hostTarget();
const resourceDir = join(process.cwd(), "src-tauri", "resources");
const output = join(resourceDir, requested.includes("windows") ? "ffmpeg.exe" : "ffmpeg");
const licenseOutput = join(resourceDir, "ffmpeg-LICENSE.txt");
if (process.argv.includes("--verify-only")) {
  if (!canRunTarget(requested)) throw new Error(`Cannot execute FFmpeg target ${requested} on this host`);
  verifyBinary(output, requested);
  console.log(`Verified bundled FFmpeg capabilities: ${output}`);
  process.exit(0);
}
if (!process.argv.includes("--force") && canRunTarget(requested) && await filesExist(output, licenseOutput)) {
  try {
    verifyBinary(output, requested);
    console.log(`Reusing verified bundled FFmpeg: ${output}`);
    process.exit(0);
  } catch (error) {
    console.warn(`Existing bundled FFmpeg is unsuitable and will be replaced: ${error.message}`);
  }
}
const work = await mkdtemp(join(tmpdir(), "devicehub-ffmpeg-"));

try {
  await mkdir(resourceDir, { recursive: true });
  if (requested.endsWith("apple-darwin")) {
    await prepareDarwin(requested, output);
  } else {
    const binary = await fetchPackagedBinary(requested);
    await cp(binary, output);
  }
  if (!requested.includes("windows")) await chmod(output, 0o755);
  if (canRunTarget(requested)) verifyBinary(output, requested);
  await writeLicense();
  console.log(`Prepared FFmpeg ${ffmpegVersion} for ${requested}: ${output}`);
} finally {
  await rm(work, { recursive: true, force: true });
}

async function fetchPackagedBinary(target) {
  const asset = assets[target];
  if (!asset) throw new Error(`Unsupported packaged FFmpeg target: ${target}`);
  const directory = join(work, target);
  await mkdir(directory, { recursive: true });
  const archive = await download(`${btbBase}/${asset[0]}`, asset[1]);
  const archivePath = join(directory, asset[0]);
  await writeFile(archivePath, archive);
  run("tar", ["-xf", archivePath, "-C", directory]);
  const binary = await findFile(directory, target.includes("windows") ? "ffmpeg.exe" : "ffmpeg");
  if (!binary) throw new Error(`${asset[0]} did not contain ffmpeg`);
  return binary;
}

async function prepareDarwin(target, destination) {
  if (process.platform !== "darwin") {
    throw new Error(`FFmpeg target ${target} must be built on macOS`);
  }
  const targets = target === "universal-apple-darwin"
    ? ["x86_64-apple-darwin", "aarch64-apple-darwin"]
    : [target];
  const source = await download(sourceUrl, sourceSha256);
  const binaries = [];
  for (const darwinTarget of targets) {
    const directory = join(work, darwinTarget);
    await mkdir(directory, { recursive: true });
    const archivePath = join(directory, `ffmpeg-${ffmpegVersion}.tar.xz`);
    await writeFile(archivePath, source);
    run("tar", ["-xf", archivePath, "-C", directory]);
    const sourceDirectory = join(directory, `ffmpeg-${ffmpegVersion}`);
    const architecture = darwinTarget.startsWith("aarch64") ? "arm64" : "x86_64";
    const sdk = runOutput("xcrun", ["--sdk", "macosx", "--show-sdk-path"]);
    const configureArgs = [
      "--disable-autodetect",
      "--disable-everything",
      "--disable-debug",
      "--disable-doc",
      "--disable-ffplay",
      "--disable-ffprobe",
      "--disable-gpl",
      "--disable-nonfree",
      "--enable-static",
      "--disable-shared",
      "--enable-ffmpeg",
      "--enable-decoder=hevc,aac",
      "--enable-encoder=pam,rawvideo,pcm_s16le",
      "--enable-demuxer=hevc,sdp,rtp",
      "--enable-muxer=image2pipe,yuv4mpegpipe,pcm_s16le",
      "--enable-parser=hevc,aac",
      "--enable-protocol=pipe,udp,rtp",
      "--enable-filter=scale,aresample",
      "--enable-swscale",
      "--enable-swresample",
      "--enable-hwaccel=hevc_videotoolbox",
      "--enable-videotoolbox",
      "--enable-audiotoolbox",
      "--enable-cross-compile",
      "--target-os=darwin",
      `--arch=${architecture}`,
      "--cc=clang",
      `--sysroot=${sdk}`,
      `--extra-cflags=-arch ${architecture}`,
      `--extra-ldflags=-arch ${architecture}`,
    ];
    if (architecture === "x86_64" && !commandAvailable("nasm", ["-v"])) {
      console.warn("NASM is unavailable; building the x86_64 FFmpeg slice without x86 assembly optimizations");
      configureArgs.push("--disable-x86asm");
    }
    run("./configure", configureArgs, { cwd: sourceDirectory });
    run("make", [`-j${Math.max(1, cpus().length)}`, "ffmpeg"], { cwd: sourceDirectory });
    binaries.push(join(sourceDirectory, "ffmpeg"));
  }
  if (binaries.length === 2) {
    run("lipo", ["-create", ...binaries, "-output", destination]);
  } else {
    await cp(binaries[0], destination);
  }
}

async function writeLicense() {
  const sections = [];
  for (const [name, checksum] of licenseFiles) {
    const content = await download(`https://raw.githubusercontent.com/FFmpeg/FFmpeg/n${ffmpegVersion}/${name}`, checksum);
    sections.push(`===== ${name} =====\n\n${content.toString("utf8").trim()}\n`);
  }
  await writeFile(licenseOutput, sections.join("\n"));
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

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { stdio: "inherit", ...options });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command} exited with ${result.status}`);
}

function runOutput(command, args) {
  const result = spawnSync(command, args, { encoding: "utf8" });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command} exited with ${result.status}: ${result.stderr}`);
  return result.stdout.trim();
}

function verifyBinary(binary, target) {
  const checks = [
    [["-hide_banner", "-decoders"], [" hevc ", " aac "]],
    [["-hide_banner", "-encoders"], [" pam ", " rawvideo ", " pcm_s16le "]],
    [["-hide_banner", "-demuxers"], [" hevc ", " rtp ", " sdp "]],
    [["-hide_banner", "-muxers"], [" image2pipe ", " yuv4mpegpipe ", " s16le "]],
    [["-hide_banner", "-protocols"], ["pipe", "rtp", "udp"]],
    [["-hide_banner", "-filters"], [" scale ", " aresample "]],
  ];
  for (const [args, required] of checks) {
    const output = runOutput(binary, args);
    for (const capability of required) {
      if (!output.includes(capability)) {
        throw new Error(`Prepared FFmpeg is missing required capability: ${capability.trim()}`);
      }
    }
  }
  const version = runOutput(binary, ["-hide_banner", "-version"]);
  if (version.includes("--enable-gpl") || version.includes("--enable-nonfree")) {
    throw new Error("Prepared FFmpeg unexpectedly enables GPL or non-free components");
  }
  if (target === "universal-apple-darwin") {
    const architectures = runOutput("lipo", ["-archs", binary]).split(/\s+/).sort();
    if (architectures.join(" ") !== "arm64 x86_64") {
      throw new Error(`Prepared universal FFmpeg has unexpected architectures: ${architectures.join(" ")}`);
    }
  } else if (target.endsWith("apple-darwin")) {
    const expected = target.startsWith("aarch64") ? "arm64" : "x86_64";
    const architectures = runOutput("lipo", ["-archs", binary]).split(/\s+/);
    if (!architectures.includes(expected)) {
      throw new Error(`Prepared FFmpeg does not contain the required ${expected} architecture`);
    }
  }
}

async function filesExist(...paths) {
  try {
    await Promise.all(paths.map((path) => access(path)));
    return true;
  } catch {
    return false;
  }
}

function canRunTarget(target) {
  return target === hostTarget() || (process.platform === "darwin" && target === "universal-apple-darwin");
}

function commandAvailable(command, args) {
  const result = spawnSync(command, args, { stdio: "ignore" });
  return !result.error && result.status === 0;
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
