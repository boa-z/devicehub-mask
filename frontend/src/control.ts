import type { Mapping } from "./types";

export type TouchContact = {
  identity: number;
  touching: boolean;
  x: number;
  y: number;
};

function pressed(held: ReadonlySet<string>, code: string) {
  return code.length > 0 && held.has(code);
}

export function buildTouchFrame(mappings: Mapping[], held: ReadonlySet<string>): TouchContact[] {
  return mappings.slice(0, 5).map((mapping) => {
    if (mapping.type === "touch") {
      return {
        identity: mapping.contactId,
        touching: pressed(held, mapping.key),
        x: mapping.x,
        y: mapping.y,
      };
    }

    let dx = Number(pressed(held, mapping.keys.right)) - Number(pressed(held, mapping.keys.left));
    let dy = Number(pressed(held, mapping.keys.down)) - Number(pressed(held, mapping.keys.up));
    const touching = dx !== 0 || dy !== 0;
    if (dx !== 0 && dy !== 0) {
      dx /= Math.SQRT2;
      dy /= Math.SQRT2;
    }
    return {
      identity: mapping.contactId,
      touching,
      x: Math.max(0, Math.min(1, mapping.x + dx * mapping.radius)),
      y: Math.max(0, Math.min(1, mapping.y + dy * mapping.radius)),
    };
  });
}

export function isBoundKey(mappings: Mapping[], code: string) {
  return mappings.some((mapping) =>
    mapping.type === "touch"
      ? mapping.key === code
      : Object.values(mapping.keys).includes(code),
  );
}

const fixedKeyboardUsages: Record<string, number> = {
  Enter: 0x28, Escape: 0x29, Backspace: 0x2a, Tab: 0x2b, Space: 0x2c,
  Minus: 0x2d, Equal: 0x2e, BracketLeft: 0x2f, BracketRight: 0x30,
  Backslash: 0x31, Semicolon: 0x33, Quote: 0x34, Backquote: 0x35,
  Comma: 0x36, Period: 0x37, Slash: 0x38, CapsLock: 0x39,
  PrintScreen: 0x46, ScrollLock: 0x47, Pause: 0x48, Insert: 0x49,
  Home: 0x4a, PageUp: 0x4b, Delete: 0x4c, End: 0x4d, PageDown: 0x4e,
  ArrowRight: 0x4f, ArrowLeft: 0x50, ArrowDown: 0x51, ArrowUp: 0x52,
  NumLock: 0x53, NumpadDivide: 0x54, NumpadMultiply: 0x55,
  NumpadSubtract: 0x56, NumpadAdd: 0x57, NumpadEnter: 0x58,
  Numpad1: 0x59, Numpad2: 0x5a, Numpad3: 0x5b, Numpad4: 0x5c,
  Numpad5: 0x5d, Numpad6: 0x5e, Numpad7: 0x5f, Numpad8: 0x60,
  Numpad9: 0x61, Numpad0: 0x62, NumpadDecimal: 0x63,
  IntlBackslash: 0x64, ContextMenu: 0x65, NumpadEqual: 0x67,
  NumpadComma: 0x85, IntlRo: 0x87, IntlYen: 0x89,
  ControlLeft: 0xe0, ShiftLeft: 0xe1, AltLeft: 0xe2, MetaLeft: 0xe3,
  ControlRight: 0xe4, ShiftRight: 0xe5, AltRight: 0xe6, MetaRight: 0xe7,
};

export function keyboardUsage(code: string): number | undefined {
  if (/^Key[A-Z]$/.test(code)) return 0x04 + code.charCodeAt(3) - 65;
  if (/^Digit[1-9]$/.test(code)) return 0x1e + Number(code[5]) - 1;
  if (code === "Digit0") return 0x27;
  if (/^F(?:[1-9]|1[0-9]|2[0-4])$/.test(code)) return 0x3a + Number(code.slice(1)) - 1;
  return fixedKeyboardUsages[code];
}
