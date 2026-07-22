import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, extname, join, relative, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const englishDir = join(root, "docs", "en");
const chineseDir = join(root, "docs", "zh-CN");

function markdownFiles(directory) {
  return readdirSync(directory)
    .filter((file) => extname(file) === ".md")
    .sort();
}

const englishPages = markdownFiles(englishDir);
const chinesePages = markdownFiles(chineseDir);
if (JSON.stringify(englishPages) !== JSON.stringify(chinesePages)) {
  throw new Error(
    `Documentation page mismatch:\nEnglish: ${englishPages.join(", ")}\nChinese: ${chinesePages.join(", ")}`,
  );
}

const files = [
  join(root, "README.md"),
  join(root, "README.zh-CN.md"),
  ...englishPages.map((file) => join(englishDir, file)),
  ...chinesePages.map((file) => join(chineseDir, file)),
];
const missing = [];
const linkPattern = /\[[^\]]*\]\(([^)]+)\)/g;

for (const file of files) {
  const content = readFileSync(file, "utf8");
  for (const match of content.matchAll(linkPattern)) {
    const target = match[1].trim();
    if (!target || target.startsWith("#") || /^[a-z][a-z\d+.-]*:/i.test(target)) continue;
    const path = decodeURIComponent(target.split("#", 1)[0]);
    if (!existsSync(resolve(dirname(file), path))) {
      missing.push(`${relative(root, file)} -> ${target}`);
    }
  }
}

if (missing.length > 0) {
  throw new Error(`Broken documentation links:\n${missing.join("\n")}`);
}

console.log(`Documentation OK: ${files.length} files, ${englishPages.length} paired pages.`);
