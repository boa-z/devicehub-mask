import fs from "node:fs";
import path from "node:path";

const localeDirectory = path.resolve("src/locales");
const sourceFile = "en-US.json";

function readLocale(fileName) {
  const filePath = path.join(localeDirectory, fileName);
  let value;
  try {
    value = JSON.parse(fs.readFileSync(filePath, "utf8"));
  } catch (error) {
    throw new Error(`Unable to parse ${fileName}: ${error instanceof Error ? error.message : error}`);
  }
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${fileName} must contain a JSON object at the root`);
  }
  return value;
}

function collectLeaves(value, prefix = "", leaves = new Map()) {
  if (typeof value === "string") {
    leaves.set(prefix, value);
    return leaves;
  }
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`Locale value at ${prefix || "<root>"} must be a string or object`);
  }
  for (const [key, child] of Object.entries(value)) {
    const pathName = prefix ? `${prefix}.${key}` : key;
    collectLeaves(child, pathName, leaves);
  }
  return leaves;
}

function placeholders(value) {
  return [...value.matchAll(/{{\s*([^{}]+?)\s*}}/g)]
    .map((match) => match[1].trim())
    .sort();
}

function sameList(left, right) {
  return left.length === right.length && left.every((value, index) => value === right[index]);
}

const files = fs
  .readdirSync(localeDirectory)
  .filter((fileName) => fileName.endsWith(".json"))
  .sort();
if (!files.includes(sourceFile)) {
  throw new Error(`Missing source locale ${sourceFile}`);
}

const sourceLeaves = collectLeaves(readLocale(sourceFile));
const errors = [];

for (const fileName of files) {
  const targetLeaves = collectLeaves(readLocale(fileName));
  const missing = [...sourceLeaves.keys()].filter((key) => !targetLeaves.has(key));
  const extra = [...targetLeaves.keys()].filter((key) => !sourceLeaves.has(key));
  if (missing.length > 0) errors.push(`${fileName}: missing keys: ${missing.join(", ")}`);
  if (extra.length > 0) errors.push(`${fileName}: unknown keys: ${extra.join(", ")}`);

  for (const [key, sourceValue] of sourceLeaves) {
    const targetValue = targetLeaves.get(key);
    if (targetValue === undefined) continue;
    if (!sameList(placeholders(sourceValue), placeholders(targetValue))) {
      errors.push(`${fileName}: interpolation tokens differ at ${key}`);
    }
  }
}

if (errors.length > 0) {
  console.error(errors.join("\n"));
  process.exit(1);
}

console.log(`Locale validation passed: ${files.length} files, ${sourceLeaves.size} strings.`);
