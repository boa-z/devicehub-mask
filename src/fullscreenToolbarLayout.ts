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
type ToolbarRect = ToolbarPoint & ToolbarSize;
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
  occupied: readonly ToolbarRect[] = [],
): FullscreenToolbarDock {
  let nearest: FullscreenToolbarDock = "top-center";
  let nearestOverlap = Number.POSITIVE_INFINITY;
  let nearestDistance = Number.POSITIVE_INFINITY;
  for (const dock of fullscreenToolbarDocks) {
    if (excluded.has(dock)) continue;
    const position = fullscreenToolbarDockPosition(dock, container, toolbar);
    const center = { x: position.x + toolbar.width / 2, y: position.y + toolbar.height / 2 };
    const rect = { ...position, ...toolbar };
    const overlap = occupied.reduce((total, other) => total + toolbarOverlapArea(rect, other), 0);
    const distance = (point.x - center.x) ** 2 + (point.y - center.y) ** 2;
    if (overlap < nearestOverlap || (overlap === nearestOverlap && distance < nearestDistance)) {
      nearest = dock;
      nearestOverlap = overlap;
      nearestDistance = distance;
    }
  }
  return nearest;
}

function toolbarRect(
  dock: FullscreenToolbarDock,
  container: ToolbarSize,
  toolbar: ToolbarSize,
): ToolbarRect {
  return { ...fullscreenToolbarDockPosition(dock, container, toolbar), ...toolbar };
}

function toolbarOverlapArea(first: ToolbarRect, second: ToolbarRect): number {
  const width = Math.max(0, Math.min(first.x + first.width, second.x + second.width) - Math.max(first.x, second.x));
  const height = Math.max(0, Math.min(first.y + first.height, second.y + second.height) - Math.max(first.y, second.y));
  return width * height;
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
    const hardwareRect = toolbarRect(docks.hardware, container, hardwareSize);
    return {
      hardware: docks.hardware,
      function: nearestFullscreenToolbarDock(
        point,
        container,
        functionSize,
        new Set([docks.hardware]),
        [hardwareRect],
      ),
    };
  }

  const hardware = nearestFullscreenToolbarDock(point, container, hardwareSize);
  const hardwareRect = toolbarRect(hardware, container, hardwareSize);
  const functionRect = toolbarRect(docks.function, container, functionSize);
  if (toolbarOverlapArea(hardwareRect, functionRect) === 0) return { hardware, function: docks.function };
  const functionPosition = fullscreenToolbarDockPosition(docks.function, container, functionSize);
  const functionCenter = {
    x: functionPosition.x + functionSize.width / 2,
    y: functionPosition.y + functionSize.height / 2,
  };
  return {
    hardware,
    function: nearestFullscreenToolbarDock(
      functionCenter,
      container,
      functionSize,
      new Set([hardware]),
      [hardwareRect],
    ),
  };
}
