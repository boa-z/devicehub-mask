import type { Mapping } from "./types";

export type TouchContact = { identity: number; touching: boolean; x: number; y: number };
export const minimumTapDurationMs = 50;
const uiControlSelector = "input, textarea, select, button, [contenteditable='true'], .ant-segmented";

export function remainingTapDuration(startedAt: number, now: number, minimum = minimumTapDurationMs) {
  return Math.max(0, minimum - Math.max(0, now - startedAt));
}

export function singleTapReleaseDelay(
  mappings: readonly Mapping[],
  code: string,
  heldSince: ReadonlyMap<string, number>,
  now: number,
) {
  return mappings.reduce((delay, mapping) => {
    if (mapping.type !== "SingleTap" || mapping.sync || !mapping.bind.includes(code)) return delay;
    const startedAt = Math.max(...mapping.bind.map((key) => heldSince.get(key) ?? now));
    return Math.max(delay, remainingTapDuration(startedAt, now, mapping.duration));
  }, 0);
}

export function mappingBindings(mapping: Mapping): string[] {
  if (mapping.type === "touch") return [mapping.key];
  if (mapping.type === "dpad") return Object.values(mapping.keys);
  const result = "bind" in mapping && Array.isArray(mapping.bind) ? [...mapping.bind] : [];
  const directionBindings = [mapping.type === "DirectionPad" ? mapping.bind : mapping.type === "PadCastSpell" ? mapping.pad_bind : undefined];
  for (const value of directionBindings) if (value?.type === "Button") result.push(...value.up, ...value.down, ...value.left, ...value.right);
  if (mapping.type === "DirectionPad" && mapping.up_boost_key) result.push(...mapping.up_boost_key);
  return result.filter(Boolean);
}

export function isBoundKey(mappings: readonly Mapping[], code: string) { return mappings.some((mapping) => mappingBindings(mapping).includes(code)); }

export type GamepadInputNames = { buttons: string[]; axes: string[] };
export type GamepadInputState = { keys: string[]; axes: Record<string, number> };

const gamepadAxisIndexes: Record<string, number> = {
  LeftStickX: 0,
  LeftStickY: 1,
  RightStickX: 2,
  RightStickY: 3,
  LeftZ: 4,
  RightZ: 5,
};

const gamepadButtonIndexes: Record<string, number> = {
  GamepadSouth: 0,
  GamepadEast: 1,
  GamepadWest: 2,
  GamepadNorth: 3,
  GamepadLeftTrigger: 4,
  GamepadRightTrigger: 5,
  GamepadLeftTrigger2: 6,
  GamepadRightTrigger2: 7,
  GamepadSelect: 8,
  GamepadStart: 9,
  GamepadLeftThumb: 10,
  GamepadRightThumb: 11,
  GamepadDPadUp: 12,
  GamepadDPadDown: 13,
  GamepadDPadLeft: 14,
  GamepadDPadRight: 15,
  GamepadMode: 16,
};
const gamepadButtonNames = new Map(Object.entries(gamepadButtonIndexes).map(([name, index]) => [index, name]));

export const gamepadAxisNames = [
  ...Object.keys(gamepadAxisIndexes),
  ...Array.from({ length: 32 }, (_, index) => `Other-${index}`),
];

function gamepadBindingNames(mapping: Mapping, names: Set<string>) {
  for (const code of mappingBindings(mapping)) if (code.startsWith("Gamepad")) names.add(code);
  const direction = mapping.type === "DirectionPad" ? mapping.bind : mapping.type === "PadCastSpell" ? mapping.pad_bind : undefined;
  if (direction?.type === "JoyStick") {
    names.add(`axis:${direction.x}`);
    names.add(`axis:${direction.y}`);
  }
}

export function gamepadInputNames(mappings: readonly Mapping[]): GamepadInputNames {
  const names = new Set<string>();
  for (const mapping of mappings) gamepadBindingNames(mapping, names);
  return {
    buttons: [...names].filter((name) => !name.startsWith("axis:")).sort(),
    axes: [...names].filter((name) => name.startsWith("axis:")).map((name) => name.slice(5)).sort(),
  };
}

function gamepadIndex(name: string): number | undefined {
  if (gamepadAxisIndexes[name] !== undefined) return gamepadAxisIndexes[name];
  if (/^Other-[0-9]+$/.test(name)) return Number(name.slice(6));
  return undefined;
}

function buttonIndex(name: string): number | undefined {
  if (gamepadButtonIndexes[name] !== undefined) return gamepadButtonIndexes[name];
  if (/^GamepadOther[0-9]+$/.test(name)) return Number(name.slice("GamepadOther".length));
  return undefined;
}

function gamepads(): Gamepad[] {
  if (typeof navigator === "undefined" || typeof navigator.getGamepads !== "function") return [];
  return Array.from(navigator.getGamepads()).filter((gamepad): gamepad is Gamepad => gamepad !== null);
}

export function readGamepadButtonPress(): string | undefined {
  for (const gamepad of gamepads()) {
    for (let index = 0; index < gamepad.buttons.length; index += 1) {
      const button = gamepad.buttons[index];
      if (button?.pressed || button?.value > 0.5) return gamepadButtonNames.get(index) ?? `GamepadOther${index}`;
    }
  }
  return undefined;
}

export function readGamepadInput(names: GamepadInputNames): GamepadInputState {
  const devices = gamepads();
  const keys = names.buttons.filter((name) => {
    const index = buttonIndex(name);
    return index !== undefined && devices.some((gamepad) => {
      const button = gamepad.buttons[index];
      return button !== undefined && (button.pressed || button.value > 0.5);
    });
  });
  const axes = Object.fromEntries(names.axes.map((name) => {
    const index = gamepadIndex(name);
    const value = index === undefined
      ? 0
      : devices.reduce((current, gamepad) => {
        const candidate = gamepad.axes[index];
        return typeof candidate === "number" && Number.isFinite(candidate) && Math.abs(candidate) > Math.abs(current) ? candidate : current;
      }, 0);
    return [name, Math.max(-1, Math.min(1, value))];
  }));
  return { keys, axes };
}

export function isUiControl(target: EventTarget | null): boolean {
  if (target === null || typeof target !== "object" || !("closest" in target)) return false;
  const closest = target.closest;
  return typeof closest === "function" && closest.call(target, uiControlSelector) !== null;
}

export function pointerButtonCode(button: number): string | undefined {
  if (button === 0) return "MouseLeft";
  if (button === 1) return "MouseMiddle";
  if (button === 2) return "MouseRight";
  if (button === 3) return "MouseBack";
  if (button === 4) return "MouseForward";
  if (Number.isInteger(button) && button >= 5) return `MouseOther${button}`;
  return undefined;
}

export function scrollBindingCode(deltaY: number): "ScrollDown" | "ScrollUp" | undefined {
  if (!Number.isFinite(deltaY) || deltaY === 0) return undefined;
  return deltaY > 0 ? "ScrollDown" : "ScrollUp";
}

const fixedKeyboardUsages: Record<string, number> = {
  Enter: 0x28, Escape: 0x29, Backspace: 0x2a, Tab: 0x2b, Space: 0x2c, Minus: 0x2d, Equal: 0x2e, BracketLeft: 0x2f, BracketRight: 0x30, Backslash: 0x31, Semicolon: 0x33, Quote: 0x34, Backquote: 0x35, Comma: 0x36, Period: 0x37, Slash: 0x38, CapsLock: 0x39, PrintScreen: 0x46, ScrollLock: 0x47, Pause: 0x48, Insert: 0x49, Home: 0x4a, PageUp: 0x4b, Delete: 0x4c, End: 0x4d, PageDown: 0x4e, ArrowRight: 0x4f, ArrowLeft: 0x50, ArrowDown: 0x51, ArrowUp: 0x52, NumLock: 0x53, NumpadDivide: 0x54, NumpadMultiply: 0x55, NumpadSubtract: 0x56, NumpadAdd: 0x57, NumpadEnter: 0x58, Numpad1: 0x59, Numpad2: 0x5a, Numpad3: 0x5b, Numpad4: 0x5c, Numpad5: 0x5d, Numpad6: 0x5e, Numpad7: 0x5f, Numpad8: 0x60, Numpad9: 0x61, Numpad0: 0x62, NumpadDecimal: 0x63, IntlBackslash: 0x64, ContextMenu: 0x65, NumpadEqual: 0x67, NumpadComma: 0x85, IntlRo: 0x87, IntlYen: 0x89, ControlLeft: 0xe0, ShiftLeft: 0xe1, AltLeft: 0xe2, MetaLeft: 0xe3, ControlRight: 0xe4, ShiftRight: 0xe5, AltRight: 0xe6, MetaRight: 0xe7,
};
export function keyboardUsage(code: string): number | undefined {
  if (/^Key[A-Z]$/.test(code)) return 0x04 + code.charCodeAt(3) - 65;
  if (/^Digit[1-9]$/.test(code)) return 0x1e + Number(code[5]) - 1;
  if (code === "Digit0") return 0x27;
  if (/^F(?:[1-9]|1[0-2])$/.test(code)) return 0x3a + Number(code.slice(1)) - 1;
  if (/^F(?:1[3-9]|2[0-4])$/.test(code)) return 0x68 + Number(code.slice(1)) - 13;
  return fixedKeyboardUsages[code];
}

const keyboardCodesByUsage = new Map(
  Object.entries(fixedKeyboardUsages).map(([code, usage]) => [usage, code]),
);

export function keyboardCodeForUsage(usage: number): string | undefined {
  if (!Number.isInteger(usage)) return undefined;
  if (usage >= 0x04 && usage <= 0x1d) return `Key${String.fromCharCode(65 + usage - 0x04)}`;
  if (usage >= 0x1e && usage <= 0x26) return `Digit${usage - 0x1e + 1}`;
  if (usage === 0x27) return "Digit0";
  if (usage >= 0x3a && usage <= 0x45) return `F${usage - 0x3a + 1}`;
  if (usage >= 0x68 && usage <= 0x73) return `F${usage - 0x68 + 13}`;
  return keyboardCodesByUsage.get(usage);
}
