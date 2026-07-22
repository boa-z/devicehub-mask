import { mappingPosition, type ButtonBinding, type DirectionBinding, type Mapping } from "./types";

export type TouchContact = { identity: number; touching: boolean; x: number; y: number };
export const minimumTapDurationMs = 50;
const pressed = (held: ReadonlySet<string>, code: string) => code.length > 0 && held.has(code);
const bound = (held: ReadonlySet<string>, keys: ButtonBinding) => keys.length > 0 && keys.every((key) => pressed(held, key));
const clamp = (value: number) => Math.max(0, Math.min(1, value));

export function direction(binding: DirectionBinding, held: ReadonlySet<string>) {
  if (binding.type !== "Button") return { dx: 0, dy: 0 };
  let dx = Number(bound(held, binding.right)) - Number(bound(held, binding.left));
  let dy = Number(bound(held, binding.down)) - Number(bound(held, binding.up));
  if (dx && dy) { dx /= Math.SQRT2; dy /= Math.SQRT2; }
  return { dx, dy };
}

function contact(mapping: Mapping, held: ReadonlySet<string>, frame: { width: number; height: number }, now: number, heldSince: ReadonlyMap<string, number>, offsets: ReadonlyMap<string, { x: number; y: number }>): TouchContact | undefined {
  if (mapping.type === "touch") return { identity: mapping.contactId, touching: pressed(held, mapping.key), x: mapping.x, y: mapping.y };
  if (mapping.type === "dpad") {
    const { dx, dy } = direction({ type: "Button", up: [mapping.keys.up], down: [mapping.keys.down], left: [mapping.keys.left], right: [mapping.keys.right] }, held);
    return { identity: mapping.contactId, touching: dx !== 0 || dy !== 0, x: clamp(mapping.x + dx * mapping.radius), y: clamp(mapping.y + dy * mapping.radius) };
  }
  if (!("pointer_id" in mapping)) return undefined;
  const center = mappingPosition(mapping);
  if (mapping.type === "DirectionPad") {
    const { dx, dy } = direction(mapping.bind, held);
    return { identity: mapping.pointer_id, touching: dx !== 0 || dy !== 0, x: clamp(center.x + dx * mapping.max_offset_x / frame.width), y: clamp(center.y + dy * mapping.max_offset_y / frame.height) };
  }
  if (mapping.type === "PadCastSpell") {
    const active = bound(held, mapping.bind);
    const { dx, dy } = direction(mapping.pad_bind, held);
    return { identity: mapping.pointer_id, touching: active, x: clamp(center.x + dx * mapping.drag_radius / frame.width), y: clamp(center.y + dy * mapping.drag_radius / frame.height) };
  }
  let active = bound(held, mapping.bind);
  const startedAt = mapping.bind.length ? Math.max(...mapping.bind.map((key) => heldSince.get(key) ?? now)) : now;
  const elapsed = Math.max(0, now - startedAt);
  let position = center;
  if (mapping.type === "Observation" || mapping.type === "Fps" || mapping.type === "Fire" || mapping.type === "MouseCastSpell") position = offsets.get(mapping.id) ?? center;
  if (mapping.type === "SingleTap") active = active && elapsed < mapping.duration;
  if (mapping.type === "RepeatTap") active = active && elapsed % Math.max(1, mapping.duration + mapping.interval) < mapping.duration;
  if (mapping.type === "MultipleTap" && mapping.items.length) {
    let cursor = 0;
    active = false;
    for (const item of mapping.items) {
      cursor += item.wait;
      if (elapsed >= cursor && elapsed < cursor + item.duration) { active = bound(held, mapping.bind); position = item.position; break; }
      cursor += item.duration;
    }
  }
  if (mapping.type === "Swipe" && mapping.positions.length) {
    const progress = Math.min(1, elapsed / Math.max(1, mapping.duration));
    const segment = progress * Math.max(1, mapping.positions.length - 1);
    const index = Math.min(mapping.positions.length - 1, Math.floor(segment));
    const next = Math.min(mapping.positions.length - 1, index + 1);
    const amount = segment - index;
    position = { x: mapping.positions[index].x + (mapping.positions[next].x - mapping.positions[index].x) * amount, y: mapping.positions[index].y + (mapping.positions[next].y - mapping.positions[index].y) * amount };
  }
  return { identity: mapping.pointer_id, touching: active, ...position };
}

export function buildTouchFrame(mappings: Mapping[], held: ReadonlySet<string>, frame = { width: 1296, height: 2816 }, now = performance.now(), heldSince: ReadonlyMap<string, number> = new Map(), offsets: ReadonlyMap<string, { x: number; y: number }> = new Map()): TouchContact[] {
  const contacts = mappings.map((mapping) => contact(mapping, held, frame, now, heldSince, offsets)).filter((value): value is TouchContact => Boolean(value));
  const active = contacts.filter((value) => value.touching);
  const inactive = contacts.filter((value) => !value.touching);
  const unique = new Map<number, TouchContact>();
  for (const value of [...active, ...inactive]) if (!unique.has(value.identity) && unique.size < 5) unique.set(value.identity, value);
  return [...unique.values()];
}

export function touchFramesEqual(left: readonly TouchContact[] | null, right: readonly TouchContact[]) {
  return left !== null
    && left.length === right.length
    && left.every((contact, index) => {
      const other = right[index];
      return contact.identity === other.identity
        && contact.touching === other.touching
        && contact.x === other.x
        && contact.y === other.y;
    });
}

export function mergeTouchContacts(
  mapped: readonly TouchContact[],
  direct: readonly TouchContact[],
  released: readonly TouchContact[] = [],
): TouchContact[] {
  const current = [...mapped, ...direct];
  const ordered = [
    ...current.filter((contact) => contact.touching),
    ...released,
    ...current.filter((contact) => !contact.touching),
  ];
  return ordered
    .filter((contact, index, all) => all.findIndex((candidate) => candidate.identity === contact.identity) === index)
    .slice(0, 5);
}

export function remainingTapDuration(startedAt: number, now: number, minimum = minimumTapDurationMs) {
  return Math.max(0, minimum - Math.max(0, now - startedAt));
}

export function mappingBindings(mapping: Mapping): string[] {
  if (mapping.type === "touch") return [mapping.key];
  if (mapping.type === "dpad") return Object.values(mapping.keys);
  const result = "bind" in mapping && Array.isArray(mapping.bind) ? [...mapping.bind] : [];
  const directionBindings = [mapping.type === "DirectionPad" ? mapping.bind : mapping.type === "PadCastSpell" ? mapping.pad_bind : undefined];
  for (const value of directionBindings) if (value?.type === "Button") result.push(...value.up, ...value.down, ...value.left, ...value.right);
  return result.filter(Boolean);
}

export function isBoundKey(mappings: Mapping[], code: string) { return mappings.some((mapping) => mappingBindings(mapping).includes(code)); }

const fixedKeyboardUsages: Record<string, number> = {
  Enter: 0x28, Escape: 0x29, Backspace: 0x2a, Tab: 0x2b, Space: 0x2c, Minus: 0x2d, Equal: 0x2e, BracketLeft: 0x2f, BracketRight: 0x30, Backslash: 0x31, Semicolon: 0x33, Quote: 0x34, Backquote: 0x35, Comma: 0x36, Period: 0x37, Slash: 0x38, CapsLock: 0x39, PrintScreen: 0x46, ScrollLock: 0x47, Pause: 0x48, Insert: 0x49, Home: 0x4a, PageUp: 0x4b, Delete: 0x4c, End: 0x4d, PageDown: 0x4e, ArrowRight: 0x4f, ArrowLeft: 0x50, ArrowDown: 0x51, ArrowUp: 0x52, NumLock: 0x53, NumpadDivide: 0x54, NumpadMultiply: 0x55, NumpadSubtract: 0x56, NumpadAdd: 0x57, NumpadEnter: 0x58, Numpad1: 0x59, Numpad2: 0x5a, Numpad3: 0x5b, Numpad4: 0x5c, Numpad5: 0x5d, Numpad6: 0x5e, Numpad7: 0x5f, Numpad8: 0x60, Numpad9: 0x61, Numpad0: 0x62, NumpadDecimal: 0x63, IntlBackslash: 0x64, ContextMenu: 0x65, NumpadEqual: 0x67, NumpadComma: 0x85, IntlRo: 0x87, IntlYen: 0x89, ControlLeft: 0xe0, ShiftLeft: 0xe1, AltLeft: 0xe2, MetaLeft: 0xe3, ControlRight: 0xe4, ShiftRight: 0xe5, AltRight: 0xe6, MetaRight: 0xe7,
};
export function keyboardUsage(code: string): number | undefined { if (/^Key[A-Z]$/.test(code)) return 0x04 + code.charCodeAt(3) - 65; if (/^Digit[1-9]$/.test(code)) return 0x1e + Number(code[5]) - 1; if (code === "Digit0") return 0x27; if (/^F(?:[1-9]|1[0-9]|2[0-4])$/.test(code)) return 0x3a + Number(code.slice(1)) - 1; return fixedKeyboardUsages[code]; }
