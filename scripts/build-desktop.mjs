import { spawnSync } from "node:child_process";
import { join } from "node:path";

const args = process.argv.slice(2);
const target = valueAfter(args, "--target") ?? hostTarget();
const node = process.execPath;

run(node, ["scripts/prepare-netmuxd.mjs", "--target", target]);
run(node, ["scripts/prepare-ffmpeg.mjs", "--target", target]);
run(node, [join("node_modules", "@tauri-apps", "cli", "tauri.js"), "build", ...args]);

function run(command, commandArgs) {
  const result = spawnSync(command, commandArgs, {
    cwd: process.cwd(),
    env: process.env,
    stdio: "inherit",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
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
