import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdtempSync, mkdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const args = process.argv.slice(2);
const target = valueAfter(args, "--target") ?? hostTarget();
const node = process.execPath;
const preserveHostFfmpeg = target !== hostTarget() && target !== "universal-apple-darwin";
const ffmpegBackup = preserveHostFfmpeg ? backupFfmpegResources() : undefined;

try {
  run(node, ["scripts/prepare-netmuxd.mjs", "--target", target]);
  run(node, [
    "scripts/prepare-ffmpeg.mjs",
    "--target",
    target,
    ...(preserveHostFfmpeg ? ["--allow-cross-resource"] : []),
  ]);
  run(node, [join("node_modules", "@tauri-apps", "cli", "tauri.js"), "build", ...args]);
} finally {
  if (ffmpegBackup) restoreFfmpegResources(ffmpegBackup);
}

function run(command, commandArgs) {
  const result = spawnSync(command, commandArgs, {
    cwd: process.cwd(),
    env: process.env,
    stdio: "inherit",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command} exited with ${result.status ?? 1}`);
}

function backupFfmpegResources() {
  const backupRoot = mkdtempSync(join(tmpdir(), "devicehub-ffmpeg-backup-"));
  const resourceRoot = join(process.cwd(), "src-tauri", "resources");
  const present = [];
  for (const name of ffmpegResourceNames()) {
    const source = join(resourceRoot, name);
    if (!existsSync(source)) continue;
    copyFileSync(source, join(backupRoot, name));
    present.push(name);
  }
  return { backupRoot, present, resourceRoot };
}

function restoreFfmpegResources({ backupRoot, present, resourceRoot }) {
  mkdirSync(resourceRoot, { recursive: true });
  for (const name of ffmpegResourceNames()) {
    const destination = join(resourceRoot, name);
    if (present.includes(name)) copyFileSync(join(backupRoot, name), destination);
    else rmSync(destination, { force: true });
  }
  rmSync(backupRoot, { recursive: true, force: true });
}

function ffmpegResourceNames() {
  return ["ffmpeg", "ffmpeg.exe", "ffmpeg-LICENSE.txt", "ffmpeg-target.json"];
}

function valueAfter(values, flag) {
  const index = values.indexOf(flag);
  if (index < 0) return undefined;
  const value = values[index + 1];
  if (!value || value.startsWith("-")) throw new Error(`${flag} requires a target triple`);
  return value;
}

function hostTarget() {
  const arch = process.arch === "arm64" ? "aarch64" : process.arch === "x64" ? "x86_64" : process.arch;
  if (process.platform === "darwin") return `${arch}-apple-darwin`;
  if (process.platform === "win32") return `${arch}-pc-windows-msvc`;
  if (process.platform === "linux") return `${arch}-unknown-linux-gnu`;
  throw new Error(`Unsupported host: ${process.platform}/${process.arch}`);
}
