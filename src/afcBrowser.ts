export type AfcSortField = "name" | "size" | "modified";
export type AfcSortDirection = "ascending" | "descending";
export type AfcAppScope = "documents" | "container";

type AfcEntry = {
  name: string;
  kind: "file" | "directory" | "other";
  size_bytes: number;
  modified: string;
};

const encoder = new TextEncoder();

export function normalizeAfcPath(path: string): string | null {
  if (encoder.encode(path).byteLength > 1_024 || path.includes("\0") || path.includes("\\")) {
    return null;
  }
  const components = path.split("/").filter(Boolean);
  if (components.some((part) => part === "." || part === ".." || encoder.encode(part).byteLength > 255)) {
    return null;
  }
  return components.length ? `/${components.join("/")}` : "/";
}

function kindOrder(kind: AfcEntry["kind"]): number {
  if (kind === "directory") return 0;
  if (kind === "file") return 1;
  return 2;
}

export function sortAfcEntries<T extends AfcEntry>(
  entries: readonly T[],
  field: AfcSortField,
  direction: AfcSortDirection,
  locale: string,
): T[] {
  const collator = new Intl.Collator(locale, { numeric: true, sensitivity: "base" });
  const multiplier = direction === "ascending" ? 1 : -1;
  return [...entries].sort((left, right) => {
    const kind = kindOrder(left.kind) - kindOrder(right.kind);
    if (kind !== 0) return kind;
    let primary = 0;
    if (field === "name") {
      primary = collator.compare(left.name, right.name);
    } else if (field === "size") {
      primary = left.size_bytes - right.size_bytes;
    } else {
      const leftTime = Date.parse(left.modified);
      const rightTime = Date.parse(right.modified);
      primary = (Number.isNaN(leftTime) ? 0 : leftTime) - (Number.isNaN(rightTime) ? 0 : rightTime);
    }
    return primary === 0
      ? collator.compare(left.name, right.name)
      : primary * multiplier;
  });
}

type AfcApp = {
  bundle_id: string;
  name: string;
  documents_available: boolean;
  is_developer_app: boolean;
};

export function availableAfcApps<T extends AfcApp>(apps: readonly T[], scope: AfcAppScope, locale: string): T[] {
  const collator = new Intl.Collator(locale, { sensitivity: "base", numeric: true });
  return apps
    .filter((app) => scope === "documents" ? app.documents_available : app.is_developer_app)
    .sort((left, right) => collator.compare(left.name, right.name) || left.bundle_id.localeCompare(right.bundle_id));
}
