import { readFile, readdir, stat } from "node:fs/promises";
import path from "node:path";

const distDirectory = path.resolve(process.argv[2] ?? "dist");
const manifestPath = path.join(distDirectory, ".vite", "manifest.json");
const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
const entries = Object.entries(manifest);
const entry = entries.find(([, chunk]) => chunk.isEntry);
if (!entry) throw new Error(`No entry chunk found in ${manifestPath}`);

const budgets = {
  initialJavaScript: 1_075_000,
  initialCss: 50_000,
  totalJavaScript: 1_475_000,
  asyncJavaScriptChunk: 70_000,
};

const assetSize = async (relativePath) => (await stat(path.join(distDirectory, relativePath))).size;
const initialChunks = new Set();

function visitStaticChunk(key) {
  if (initialChunks.has(key)) return;
  const chunk = manifest[key];
  if (!chunk) throw new Error(`Manifest references missing chunk ${key}`);
  initialChunks.add(key);
  for (const dependency of chunk.imports ?? []) visitStaticChunk(dependency);
}

visitStaticChunk(entry[0]);

let initialJavaScript = 0;
let initialCss = 0;
const initialCssFiles = new Set();
for (const key of initialChunks) {
  const chunk = manifest[key];
  initialJavaScript += await assetSize(chunk.file);
  for (const cssFile of chunk.css ?? []) initialCssFiles.add(cssFile);
}
for (const cssFile of initialCssFiles) initialCss += await assetSize(cssFile);

const assetFiles = await readdir(path.join(distDirectory, "assets"));
const javascriptFiles = assetFiles.filter((file) => file.endsWith(".js"));
const javascriptSizes = await Promise.all(javascriptFiles.map(async (file) => ({
  file,
  size: await assetSize(path.join("assets", file)),
})));
const initialJavaScriptFiles = new Set([...initialChunks].map((key) => manifest[key].file));
const asyncChunks = javascriptSizes.filter(({ file }) => !initialJavaScriptFiles.has(`assets/${file}`));
const largestAsyncChunk = asyncChunks.reduce(
  (largest, chunk) => chunk.size > largest.size ? chunk : largest,
  { file: "none", size: 0 },
);
const totalJavaScript = javascriptSizes.reduce((total, chunk) => total + chunk.size, 0);

const measurements = [
  ["initial JavaScript", initialJavaScript, budgets.initialJavaScript],
  ["initial CSS", initialCss, budgets.initialCss],
  ["total JavaScript", totalJavaScript, budgets.totalJavaScript],
  [`largest async JavaScript chunk (${largestAsyncChunk.file})`, largestAsyncChunk.size, budgets.asyncJavaScriptChunk],
];

let failed = false;
for (const [label, actual, limit] of measurements) {
  const status = actual <= limit ? "OK" : "OVER";
  console.log(`${status} ${label}: ${actual.toLocaleString("en-US")} / ${limit.toLocaleString("en-US")} bytes`);
  failed ||= actual > limit;
}
if (failed) {
  throw new Error("Frontend performance budget exceeded; split or reduce the affected dependency graph.");
}
