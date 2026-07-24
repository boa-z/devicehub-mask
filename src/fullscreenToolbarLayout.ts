export const fullscreenToolbarDocks = [
  "top-left",
  "top-center",
  "top-right",
  "left-center",
  "right-center",
  "bottom-left",
  "bottom-center",
  "bottom-right",
] as const;

export type FullscreenToolbarDock = (typeof fullscreenToolbarDocks)[number];

export type ToolbarSize = { width: number; height: number };
export type ToolbarPoint = { x: number; y: number };
export type FullscreenToolbarKind = "hardware" | "function";
export type FullscreenToolbarDocks = { hardware: FullscreenToolbarDock; function: FullscreenToolbarDock };

const dockSet = new Set<string>(fullscreenToolbarDocks);

export function isFullscreenToolbarDock(value: unknown): value is FullscreenToolbarDock {
  return typeof value === "string" && dockSet.has(value);
}

export function fullscreenToolbarDockPosition(
  dock: FullscreenToolbarDock,
  container: ToolbarSize,
  toolbar: ToolbarSize,
  margin = 8,
): ToolbarPoint {
  const left = margin;
  const centerX = (container.width - toolbar.width) / 2;
  const right = container.width - toolbar.width - margin;
  const top = margin;
  const centerY = (container.height - toolbar.height) / 2;
  const bottom = container.height - toolbar.height - margin;
  const [vertical, horizontal] = dock.split("-") as ["top" | "left" | "right" | "bottom", "left" | "center" | "right"];

  if (vertical === "left") return { x: left, y: Math.max(margin, centerY) };
  if (vertical === "right") return { x: Math.max(margin, right), y: Math.max(margin, centerY) };
  const x = horizontal === "left" ? left : horizontal === "right" ? right : centerX;
  return {
    x: Math.max(margin, x),
    y: vertical === "top" ? top : Math.max(margin, bottom),
  };
}

export function nearestFullscreenToolbarDock(
  point: ToolbarPoint,
  container: ToolbarSize,
  toolbar: ToolbarSize,
  excluded: ReadonlySet<FullscreenToolbarDock> = new Set(),
): FullscreenToolbarDock {
  let nearest: FullscreenToolbarDock = "top-center";
  let nearestDistance = Number.POSITIVE_INFINITY;
  for (const dock of fullscreenToolbarDocks) {
    if (excluded.has(dock)) continue;
    const position = fullscreenToolbarDockPosition(dock, container, toolbar);
    const center = { x: position.x + toolbar.width / 2, y: position.y + toolbar.height / 2 };
    const distance = (point.x - center.x) ** 2 + (point.y - center.y) ** 2;
    if (distance < nearestDistance) {
      nearest = dock;
      nearestDistance = distance;
    }
  }
  return nearest;
}

export function clampToolbarPosition(
  point: ToolbarPoint,
  container: ToolbarSize,
  toolbar: ToolbarSize,
  margin = 8,
): ToolbarPoint {
  return {
    x: Math.min(Math.max(margin, point.x), Math.max(margin, container.width - toolbar.width - margin)),
    y: Math.min(Math.max(margin, point.y), Math.max(margin, container.height - toolbar.height - margin)),
  };
}

export function resolveFullscreenToolbarDrop(
  kind: FullscreenToolbarKind,
  point: ToolbarPoint,
  docks: FullscreenToolbarDocks,
  container: ToolbarSize,
  hardwareSize: ToolbarSize,
  functionSize: ToolbarSize,
): FullscreenToolbarDocks {
  if (kind === "function") {
    return {
      hardware: docks.hardware,
      function: nearestFullscreenToolbarDock(point, container, functionSize, new Set([docks.hardware])),
    };
  }

  const hardware = nearestFullscreenToolbarDock(point, container, hardwareSize);
  if (hardware !== docks.function) return { hardware, function: docks.function };
  const functionPosition = fullscreenToolbarDockPosition(docks.function, container, functionSize);
  const functionCenter = {
    x: functionPosition.x + functionSize.width / 2,
    y: functionPosition.y + functionSize.height / 2,
  };
  return {
    hardware,
    function: nearestFullscreenToolbarDock(functionCenter, container, functionSize, new Set([hardware])),
  };
}
