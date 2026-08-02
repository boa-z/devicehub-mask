import type { Orientation } from "./types";

export type PointerDebugSource = "direct" | "keymap";

export type PointerDebugContact = {
  identity: number;
  touching: boolean;
  x: number;
  y: number;
  source: PointerDebugSource;
};

export type PointerDebugAction = "down" | "move" | "up";

export type PointerDebugEvent = PointerDebugContact & {
  action: PointerDebugAction;
  at: number;
};

export type PointerDebugPoint = {
  displayX: number;
  displayY: number;
  nativeX: number;
  nativeY: number;
  displayPixelX: number;
  displayPixelY: number;
  nativePixelX: number;
  nativePixelY: number;
};

type FrameSize = { width: number; height: number };

function clampNormalized(value: number) {
  return Number.isFinite(value) ? Math.max(0, Math.min(1, value)) : 0;
}

function pixelCoordinate(value: number, size: number) {
  return Math.round(value * Math.max(0, Math.floor(size) - 1));
}

export function pointerDebugContactKey(contact: Pick<PointerDebugContact, "source" | "identity">) {
  return `${contact.source}:${contact.identity}`;
}

export function displayToNativePoint(
  x: number,
  y: number,
  orientation: Orientation,
  frameSize: FrameSize,
): PointerDebugPoint {
  const displayX = clampNormalized(x);
  const displayY = clampNormalized(y);
  const [nativeX, nativeY] = orientation === "landscape_right"
    ? [displayY, 1 - displayX]
    : orientation === "portrait_upside_down"
      ? [1 - displayX, 1 - displayY]
      : orientation === "landscape_left"
        ? [1 - displayY, displayX]
        : [displayX, displayY];
  const nativeFrameSize = orientation === "landscape_left" || orientation === "landscape_right"
    ? { width: frameSize.height, height: frameSize.width }
    : frameSize;
  return {
    displayX,
    displayY,
    nativeX,
    nativeY,
    displayPixelX: pixelCoordinate(displayX, frameSize.width),
    displayPixelY: pixelCoordinate(displayY, frameSize.height),
    nativePixelX: pixelCoordinate(nativeX, nativeFrameSize.width),
    nativePixelY: pixelCoordinate(nativeY, nativeFrameSize.height),
  };
}

function sameContact(left: PointerDebugContact, right: PointerDebugContact) {
  return left.touching === right.touching && left.x === right.x && left.y === right.y;
}

export function diffPointerDebugContacts(
  previous: ReadonlyMap<string, PointerDebugContact>,
  current: readonly PointerDebugContact[],
  at: number,
): PointerDebugEvent[] {
  const events: PointerDebugEvent[] = [];
  const currentKeys = new Set<string>();
  for (const contact of current) {
    const key = pointerDebugContactKey(contact);
    currentKeys.add(key);
    const before = previous.get(key);
    if (!before) {
      events.push({ ...contact, action: contact.touching ? "down" : "up", at });
    } else if (!sameContact(before, contact)) {
      events.push({ ...contact, action: contact.touching ? "move" : "up", at });
    }
  }
  for (const [key, contact] of previous) {
    if (!currentKeys.has(key) && contact.touching) events.push({ ...contact, touching: false, action: "up", at });
  }
  return events;
}
