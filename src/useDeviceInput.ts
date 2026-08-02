import { useCallback, useEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import {
  gamepadInputNames,
  readGamepadInput,
  isBoundKey,
  isUiControl,
  keyboardUsage,
  pointerButtonCode,
  remainingTapDuration,
  scrollBindingCode,
  singleTapReleaseDelay,
  type TouchContact,
} from "./control";
import { logFrontend } from "./diagnostics";
import type { HardwareButtonName, Mapping, Profile } from "./types";
import type { KeymapStatus } from "./useDeviceVideoStream";

export type ControlMode = "mapping" | "keyboard";

type PointerDelta = {
  mapping_id: string;
  delta_x: number;
  delta_y: number;
  cursor_x?: number;
  cursor_y?: number;
};

export type DeviceInputCommand =
  | { type: "multi_touch"; contacts: TouchContact[] }
  | { type: "button_down" | "button_up"; name: HardwareButtonName }
  | { type: "keyboard_down" | "keyboard_up"; usage: number }
  | { type: "keymap_configure"; profile: Profile; frame: FrameSize; allow_scripts: boolean }
  | { type: "keymap_input"; keys: string[]; pointer_deltas: PointerDelta[]; gamepad_axes: Record<string, number> }
  | { type: "keymap_direct_touches"; contacts: TouchContact[] }
  | { type: "keymap_debug"; enabled: boolean }
  | { type: "keymap_stop" };

export function directTouchCommand(controlMode: ControlMode, contacts: TouchContact[]): DeviceInputCommand {
  return controlMode === "mapping"
    ? { type: "keymap_direct_touches", contacts }
    : { type: "multi_touch", contacts };
}

type FrameSize = { width: number; height: number };

type Options = {
  connected: boolean;
  command: (payload: DeviceInputCommand) => void;
  profile: Profile;
  keymapStatus: KeymapStatus;
  frameSize: FrameSize;
  pointerDebugEnabled: boolean;
  mappingEditing: boolean;
  controlMode: ControlMode;
  onControlModeChange: (mode: ControlMode) => void;
  onContactLimit: () => void;
};

export type DeviceInputCollections = {
  held: Set<string>;
  heldSince: Map<string, number>;
  mappingOffsets: Map<string, { x: number; y: number }>;
  heldHardware: Map<string, HardwareButtonName>;
  forwardedKeyboard: Map<string, number>;
  directTouches: Map<number, TouchContact>;
  directTouchStartedAt: Map<number, number>;
  directTouchReleaseTimers: Map<number, number>;
  mappedReleaseTimers: Map<string, number>;
  mappedContactIds: Map<string, number>;
  heldPointerBindings: Map<number, string>;
};

export function clearDeviceInputCollections(
  collections: DeviceInputCollections,
  cancelReleaseTimer: (timer: number) => void,
) {
  for (const timer of collections.directTouchReleaseTimers.values()) cancelReleaseTimer(timer);
  for (const timer of collections.mappedReleaseTimers.values()) cancelReleaseTimer(timer);
  for (const collection of Object.values(collections)) collection.clear();
}

function pointFromElement(element: HTMLElement, clientX: number, clientY: number) {
  const bounds = element.getBoundingClientRect();
  return {
    x: Math.max(0, Math.min(1, (clientX - bounds.left) / bounds.width)),
    y: Math.max(0, Math.min(1, (clientY - bounds.top) / bounds.height)),
  };
}

function pointFromPointer(event: ReactPointerEvent<HTMLDivElement>) {
  return pointFromElement(event.currentTarget, event.clientX, event.clientY);
}

const pointerMappingTypes = new Set<Mapping["type"]>([
  "MouseCastSpell",
  "Observation",
  "Fps",
  "Fire",
]);

function profileUsesKey(profile: Profile, code: string) {
  return isBoundKey(profile.mappings, code)
    || Object.values(profile.hardwareBindings).includes(code);
}

function pointerMappings(profile: Profile, ids: ReadonlySet<string>) {
  return profile.mappings.filter((mapping) => ids.has(mapping.id) && acceptsPointerDelta(mapping));
}

export function acceptsPointerDelta(mapping: Mapping) {
  return pointerMappingTypes.has(mapping.type)
    && !(mapping.type === "MouseCastSpell" && (mapping.cast_no_direction || mapping.release_mode === "OnPress"))
    && !(mapping.type === "Fire" && mapping.preserve_fps_control);
}

export function pointerInputMappings(profile: Profile, code: string, held: ReadonlySet<string>) {
  return profile.mappings.filter((mapping) => {
    const binding = "bind" in mapping && Array.isArray(mapping.bind) ? mapping.bind : null;
    return binding !== null
      && binding.includes(code)
      && binding.every((key) => held.has(key))
      && acceptsPointerDelta(mapping);
  });
}

export function rawInputTriggered(profile: Profile, held: ReadonlySet<string>) {
  return profile.mappings.some((mapping) => (
    mapping.type === "RawInput"
      && mapping.bind.length > 0
      && mapping.bind.every((key) => held.has(key))
  ));
}

export function useDeviceInput(options: Options) {
  const {
    connected,
    controlMode,
    command,
    frameSize,
    keymapStatus,
    mappingEditing,
    onControlModeChange,
    pointerDebugEnabled,
    profile,
  } = options;
  const [directTouches, setDirectTouches] = useState<TouchContact[]>([]);
  const optionsRef = useRef(options);
  const heldRef = useRef(new Set<string>());
  const gamepadKeysRef = useRef(new Set<string>());
  const gamepadAxesRef = useRef<Record<string, number>>({});
  const heldSinceRef = useRef(new Map<string, number>());
  const forwardedKeyboardRef = useRef(new Map<string, number>());
  const directTouchesRef = useRef(new Map<number, TouchContact>());
  const directTouchStartedAtRef = useRef(new Map<number, number>());
  const directTouchReleaseTimersRef = useRef(new Map<number, number>());
  const mappedReleaseTimersRef = useRef(new Map<string, number>());
  const heldPointerBindingsRef = useRef(new Map<number, string>());
  const pointerLockTargetRef = useRef<HTMLElement | null>(null);
  const pointerCursorRef = useRef({ x: 0.5, y: 0.5 });
  const activeMappingIdsRef = useRef(new Set(options.keymapStatus.active_mapping_ids));
  optionsRef.current = options;
  activeMappingIdsRef.current = new Set(options.keymapStatus.active_mapping_ids);
  const gamepadNames = useMemo(() => gamepadInputNames(profile.mappings), [profile.mappings]);

  const collections = useCallback((): DeviceInputCollections => ({
    held: heldRef.current,
    heldSince: heldSinceRef.current,
    mappingOffsets: new Map(),
    heldHardware: new Map(),
    forwardedKeyboard: forwardedKeyboardRef.current,
    directTouches: directTouchesRef.current,
    directTouchStartedAt: directTouchStartedAtRef.current,
    directTouchReleaseTimers: directTouchReleaseTimersRef.current,
    mappedReleaseTimers: mappedReleaseTimersRef.current,
    mappedContactIds: new Map(),
    heldPointerBindings: heldPointerBindingsRef.current,
  }), []);

  const sendKeyState = useCallback((pointer_deltas: PointerDelta[] = []) => {
    const current = optionsRef.current;
    if (!current.connected || current.controlMode !== "mapping") return;
    current.command({
      type: "keymap_input",
      keys: [...heldRef.current, ...gamepadKeysRef.current],
      pointer_deltas,
      gamepad_axes: gamepadAxesRef.current,
    });
  }, []);

  const sendDirectTouches = useCallback(() => {
    const current = optionsRef.current;
    if (!current.connected) return;
    current.command(directTouchCommand(current.controlMode, [...directTouchesRef.current.values()]));
  }, []);

  const exitPointerLock = useCallback(() => {
    const target = pointerLockTargetRef.current;
    pointerLockTargetRef.current = null;
    if (target && document.pointerLockElement === target) document.exitPointerLock();
  }, []);

  const releaseAllControls = useCallback(() => {
    const current = optionsRef.current;
    const forwarded = [...forwardedKeyboardRef.current.values()];
    clearDeviceInputCollections(collections(), (timer) => window.clearTimeout(timer));
    gamepadKeysRef.current.clear();
    gamepadAxesRef.current = {};
    pointerCursorRef.current = { x: 0.5, y: 0.5 };
    setDirectTouches([]);
    exitPointerLock();
    if (!current.connected) return;
    for (const usage of forwarded) current.command({ type: "keyboard_up", usage });
    if (current.controlMode === "mapping") {
      current.command({ type: "keymap_input", keys: [], pointer_deltas: [], gamepad_axes: {} });
      current.command({ type: "keymap_direct_touches", contacts: [] });
    } else {
      current.command({ type: "multi_touch", contacts: [] });
    }
  }, [collections, exitPointerLock]);

  const switchToKeyboardMode = useCallback(() => {
    const current = optionsRef.current;
    releaseAllControls();
    current.onControlModeChange("keyboard");
  }, [releaseAllControls]);

  useEffect(() => {
    if (!connected) return;
    if (controlMode === "mapping") {
      command({
        type: "keymap_configure",
        profile,
        frame: frameSize,
        allow_scripts: true,
      });
      sendKeyState();
      sendDirectTouches();
    } else {
      command({ type: "keymap_stop" });
    }
  }, [command, connected, controlMode, frameSize, profile, sendDirectTouches, sendKeyState]);

  useEffect(() => {
    if (!connected) return;
    command({ type: "keymap_debug", enabled: pointerDebugEnabled });
  }, [command, connected, pointerDebugEnabled]);

  useEffect(() => {
    if (!connected || controlMode !== "mapping" || mappingEditing
      || (gamepadNames.buttons.length === 0 && gamepadNames.axes.length === 0)) {
      gamepadKeysRef.current.clear();
      gamepadAxesRef.current = {};
      return;
    }
    let animationFrame = 0;
    let previous = "";
    let switchedToKeyboard = false;
    const poll = () => {
      if (switchedToKeyboard) return;
      const state = readGamepadInput(gamepadNames);
      const held = new Set([...heldRef.current, ...state.keys]);
      const current = optionsRef.current;
      if (rawInputTriggered(current.profile, held)) {
        switchedToKeyboard = true;
        switchToKeyboardMode();
        return;
      }
      const signature = JSON.stringify(state);
      if (signature !== previous) {
        previous = signature;
        gamepadKeysRef.current = new Set(state.keys);
        gamepadAxesRef.current = state.axes;
        sendKeyState();
      }
      animationFrame = window.requestAnimationFrame(poll);
    };
    poll();
    return () => window.cancelAnimationFrame(animationFrame);
  }, [connected, controlMode, gamepadNames, mappingEditing, sendKeyState, switchToKeyboardMode]);

  useEffect(() => {
    const mode = keymapStatus.control_mode;
    if (mode && mode !== controlMode) {
      releaseAllControls();
      onControlModeChange(mode);
    }
    if (keymapStatus.error) {
      logFrontend("warn", "keymap", "runtime", keymapStatus.error);
    }
  }, [controlMode, keymapStatus, onControlModeChange, releaseAllControls]);

  useEffect(() => {
    if (!options.connected) {
      clearDeviceInputCollections(collections(), (timer) => window.clearTimeout(timer));
      gamepadKeysRef.current.clear();
      gamepadAxesRef.current = {};
      setDirectTouches([]);
      exitPointerLock();
    }
  }, [collections, exitPointerLock, options.connected]);

  useEffect(() => {
    const changed = () => {
      const target = pointerLockTargetRef.current;
      if (!target || document.pointerLockElement === target) return;
      pointerLockTargetRef.current = null;
      releaseAllControls();
    };
    document.addEventListener("pointerlockchange", changed);
    return () => document.removeEventListener("pointerlockchange", changed);
  }, [releaseAllControls]);

  useEffect(() => {
    const activePointers = pointerMappings(options.profile, activeMappingIdsRef.current);
    if (activePointers.length === 0) exitPointerLock();
  }, [exitPointerLock, options.keymapStatus.active_mapping_ids, options.profile]);

  const capturePointer = useCallback((target: HTMLElement) => {
    pointerLockTargetRef.current = target;
    const request = target.requestPointerLock?.();
    if (request && "catch" in request) void request.catch(() => {
      if (pointerLockTargetRef.current === target) pointerLockTargetRef.current = null;
    });
  }, []);

  const finishMappedRelease = useCallback((code: string) => {
    mappedReleaseTimersRef.current.delete(code);
    if (!heldRef.current.delete(code)) return;
    heldSinceRef.current.delete(code);
    sendKeyState();
  }, [sendKeyState]);

  const releaseMappedKey = useCallback((code: string) => {
    const pending = mappedReleaseTimersRef.current.get(code);
    if (pending !== undefined) window.clearTimeout(pending);
    const current = optionsRef.current;
    const delay = singleTapReleaseDelay(
      current.profile.mappings,
      code,
      heldSinceRef.current,
      performance.now(),
    );
    if (delay > 0) {
      mappedReleaseTimersRef.current.set(
        code,
        window.setTimeout(() => finishMappedRelease(code), delay),
      );
    } else {
      finishMappedRelease(code);
    }
  }, [finishMappedRelease]);

  useEffect(() => {
    const move = (event: PointerEvent) => {
      const current = optionsRef.current;
      if (!current.connected
        || current.mappingEditing
        || current.controlMode !== "mapping"
        || (!event.movementX && !event.movementY)
        || document.pointerLockElement === null) return;
      const target = pointerLockTargetRef.current;
      if (target) {
        const bounds = target.getBoundingClientRect();
        if (bounds.width > 0 && bounds.height > 0) {
          pointerCursorRef.current = {
            x: Math.max(0, Math.min(1, pointerCursorRef.current.x + event.movementX / bounds.width)),
            y: Math.max(0, Math.min(1, pointerCursorRef.current.y + event.movementY / bounds.height)),
          };
        }
      }
      const deltas = pointerMappings(current.profile, activeMappingIdsRef.current).map((mapping) => ({
        mapping_id: mapping.id,
        delta_x: event.movementX,
        delta_y: event.movementY,
        cursor_x: pointerCursorRef.current.x,
        cursor_y: pointerCursorRef.current.y,
      }));
      if (deltas.length) sendKeyState(deltas);
    };
    window.addEventListener("pointermove", move);
    return () => window.removeEventListener("pointermove", move);
  }, [sendKeyState]);

  useEffect(() => {
    const down = (event: KeyboardEvent) => {
      const current = optionsRef.current;
      if (event.ctrlKey && event.shiftKey && event.code === "KeyK") {
        event.preventDefault();
        releaseAllControls();
        current.onControlModeChange(current.controlMode === "mapping" ? "keyboard" : "mapping");
        return;
      }
      if (!current.connected) return;
      if (current.controlMode === "keyboard") {
        if (event.repeat || isUiControl(event.target)) return;
        const usage = keyboardUsage(event.code);
        if (usage === undefined) return;
        event.preventDefault();
        forwardedKeyboardRef.current.set(event.code, usage);
        current.command({ type: "keyboard_down", usage });
        return;
      }
      if (current.mappingEditing || event.repeat || isUiControl(event.target)) return;
      if (!profileUsesKey(current.profile, event.code)) return;
      event.preventDefault();
      const held = new Set([...heldRef.current, ...gamepadKeysRef.current]);
      held.add(event.code);
      if (rawInputTriggered(current.profile, held)) {
        switchToKeyboardMode();
        const usage = keyboardUsage(event.code);
        if (usage !== undefined) {
          forwardedKeyboardRef.current.set(event.code, usage);
          current.command({ type: "keyboard_down", usage });
        }
        return;
      }
      const pending = mappedReleaseTimersRef.current.get(event.code);
      if (pending !== undefined) {
        window.clearTimeout(pending);
        mappedReleaseTimersRef.current.delete(event.code);
      }
      heldRef.current.add(event.code);
      heldSinceRef.current.set(event.code, performance.now());
      if (pointerInputMappings(current.profile, event.code, held).length > 0
        && event.target instanceof HTMLElement) capturePointer(event.target);
      sendKeyState();
    };

    const up = (event: KeyboardEvent) => {
      const current = optionsRef.current;
      const usage = forwardedKeyboardRef.current.get(event.code);
      if (usage !== undefined) {
        event.preventDefault();
        forwardedKeyboardRef.current.delete(event.code);
        current.command({ type: "keyboard_up", usage });
        return;
      }
      if (!heldRef.current.has(event.code)) return;
      event.preventDefault();
      releaseMappedKey(event.code);
    };

    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    window.addEventListener("blur", releaseAllControls);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
      window.removeEventListener("blur", releaseAllControls);
    };
  }, [capturePointer, releaseAllControls, releaseMappedKey, sendKeyState, switchToKeyboardMode]);

  useEffect(() => {
    const wheel = (event: WheelEvent) => {
      const current = optionsRef.current;
      if (!current.connected || current.mappingEditing || current.controlMode !== "mapping") return;
      const code = scrollBindingCode(event.deltaY);
      if (!code || !profileUsesKey(current.profile, code)) return;
      event.preventDefault();
      const held = new Set([...heldRef.current, ...gamepadKeysRef.current]);
      held.add(code);
      if (rawInputTriggered(current.profile, held)) {
        switchToKeyboardMode();
        return;
      }
      const pending = mappedReleaseTimersRef.current.get(code);
      if (pending !== undefined) window.clearTimeout(pending);
      heldRef.current.delete(code);
      heldSinceRef.current.delete(code);
      sendKeyState();
      const startedAt = performance.now();
      heldRef.current.add(code);
      heldSinceRef.current.set(code, startedAt);
      sendKeyState();
      mappedReleaseTimersRef.current.set(
        code,
        window.setTimeout(() => finishMappedRelease(code), remainingTapDuration(startedAt, performance.now())),
      );
    };
    window.addEventListener("wheel", wheel, { passive: false });
    return () => window.removeEventListener("wheel", wheel);
  }, [finishMappedRelease, sendKeyState, switchToKeyboardMode]);

  const handlePointerDown = useCallback((event: ReactPointerEvent<HTMLDivElement>) => {
    const current = optionsRef.current;
    if (!current.connected || current.mappingEditing) return;
    const pointerCode = pointerButtonCode(event.button);
    if (current.controlMode === "mapping" && pointerCode && profileUsesKey(current.profile, pointerCode)) {
      event.preventDefault();
      const held = new Set([...heldRef.current, ...gamepadKeysRef.current]);
      held.add(pointerCode);
      if (rawInputTriggered(current.profile, held)) {
        switchToKeyboardMode();
        return;
      }
      event.currentTarget.focus();
      event.currentTarget.setPointerCapture(event.pointerId);
      const pending = mappedReleaseTimersRef.current.get(pointerCode);
      if (pending !== undefined) {
        window.clearTimeout(pending);
        mappedReleaseTimersRef.current.delete(pointerCode);
      }
      heldPointerBindingsRef.current.set(event.pointerId, pointerCode);
      heldRef.current.add(pointerCode);
      heldSinceRef.current.set(pointerCode, performance.now());
      const inputMappings = pointerInputMappings(current.profile, pointerCode, held);
      if (inputMappings.length > 0 && event.currentTarget instanceof HTMLElement) {
        pointerCursorRef.current = pointFromPointer(event);
        capturePointer(event.currentTarget);
      }
      const pointer_deltas = inputMappings
        .filter((mapping) => mapping.type === "MouseCastSpell")
        .map((mapping) => ({
          mapping_id: mapping.id,
          delta_x: 0,
          delta_y: 0,
          cursor_x: pointerCursorRef.current.x,
          cursor_y: pointerCursorRef.current.y,
        }));
      sendKeyState(pointer_deltas);
      return;
    }
    if (event.button !== 0 || directTouchesRef.current.has(event.pointerId)) return;
    const used = new Set([
      ...(current.controlMode === "mapping" ? current.keymapStatus.active_contact_ids ?? [] : []),
      ...[...directTouchesRef.current.values()].map((contact) => contact.identity),
    ]);
    const identity = [0, 1, 2, 3, 4].find((candidate) => !used.has(candidate));
    if (identity === undefined) {
      current.onContactLimit();
      return;
    }
    event.preventDefault();
    event.currentTarget.focus();
    event.currentTarget.setPointerCapture(event.pointerId);
    const contact = { identity, touching: true, ...pointFromPointer(event) };
    directTouchesRef.current.set(event.pointerId, contact);
    directTouchStartedAtRef.current.set(event.pointerId, performance.now());
    setDirectTouches([...directTouchesRef.current.values()]);
    sendDirectTouches();
  }, [capturePointer, sendDirectTouches, sendKeyState, switchToKeyboardMode]);

  const handlePointerMove = useCallback((event: ReactPointerEvent<HTMLDivElement>) => {
    const contact = directTouchesRef.current.get(event.pointerId);
    if (!contact) return;
    event.preventDefault();
    directTouchesRef.current.set(event.pointerId, { ...contact, ...pointFromPointer(event) });
    setDirectTouches([...directTouchesRef.current.values()]);
    sendDirectTouches();
  }, [sendDirectTouches]);

  const handlePointerUp = useCallback((event: ReactPointerEvent<HTMLDivElement>) => {
    const pointerCode = heldPointerBindingsRef.current.get(event.pointerId);
    if (pointerCode) {
      event.preventDefault();
      heldPointerBindingsRef.current.delete(event.pointerId);
      releaseMappedKey(pointerCode);
      return;
    }
    const contact = directTouchesRef.current.get(event.pointerId);
    if (!contact || directTouchReleaseTimersRef.current.has(event.pointerId)) return;
    event.preventDefault();
    const pointerId = event.pointerId;
    const finalContact = { ...contact, ...pointFromPointer(event) };
    directTouchesRef.current.set(pointerId, finalContact);
    const finish = () => {
      if (!directTouchesRef.current.has(pointerId)) return;
      directTouchReleaseTimersRef.current.delete(pointerId);
      directTouchStartedAtRef.current.delete(pointerId);
      directTouchesRef.current.delete(pointerId);
      setDirectTouches([...directTouchesRef.current.values()]);
      sendDirectTouches();
    };
    const delay = remainingTapDuration(
      directTouchStartedAtRef.current.get(pointerId) ?? performance.now(),
      performance.now(),
    );
    if (delay > 0) {
      directTouchReleaseTimersRef.current.set(pointerId, window.setTimeout(finish, delay));
    } else {
      finish();
    }
  }, [releaseMappedKey, sendDirectTouches]);

  return {
    activeMappingIds: new Set(options.keymapStatus.active_mapping_ids),
    directTouches,
    releaseAllControls,
    handlePointerDown,
    handlePointerMove,
    handlePointerUp,
  };
}
