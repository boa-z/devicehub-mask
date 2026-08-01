import { useCallback, useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import {
  isBoundKey,
  isUiControl,
  keyboardUsage,
  mappingBindings,
  pointerButtonCode,
  remainingTapDuration,
  singleTapReleaseDelay,
  type TouchContact,
} from "./control";
import { logFrontend } from "./diagnostics";
import type { HardwareButtonName, Mapping, Profile } from "./types";
import type { KeymapStatus } from "./useDeviceVideoStream";

export type ControlMode = "mapping" | "keyboard";

type PointerDelta = { mapping_id: string; delta_x: number; delta_y: number };

export type DeviceInputCommand =
  | { type: "multi_touch"; contacts: TouchContact[] }
  | { type: "button_down" | "button_up"; name: HardwareButtonName }
  | { type: "keyboard_down" | "keyboard_up"; usage: number }
  | { type: "keymap_configure"; profile: Profile; frame: FrameSize; allow_scripts: boolean }
  | { type: "keymap_input"; keys: string[]; pointer_deltas: PointerDelta[] }
  | { type: "keymap_direct_touches"; contacts: TouchContact[] }
  | { type: "keymap_stop" };

type FrameSize = { width: number; height: number };

type Options = {
  connected: boolean;
  command: (payload: DeviceInputCommand) => void;
  profile: Profile;
  keymapStatus: KeymapStatus;
  frameSize: FrameSize;
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

function pointFromPointer(event: ReactPointerEvent<HTMLDivElement>) {
  const bounds = event.currentTarget.getBoundingClientRect();
  return {
    x: Math.max(0, Math.min(1, (event.clientX - bounds.left) / bounds.width)),
    y: Math.max(0, Math.min(1, (event.clientY - bounds.top) / bounds.height)),
  };
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
  return profile.mappings.filter((mapping) => ids.has(mapping.id) && pointerMappingTypes.has(mapping.type));
}

export function useDeviceInput(options: Options) {
  const {
    connected,
    controlMode,
    command,
    frameSize,
    keymapStatus,
    onControlModeChange,
    profile,
  } = options;
  const [directTouches, setDirectTouches] = useState<TouchContact[]>([]);
  const optionsRef = useRef(options);
  const heldRef = useRef(new Set<string>());
  const heldSinceRef = useRef(new Map<string, number>());
  const forwardedKeyboardRef = useRef(new Map<string, number>());
  const directTouchesRef = useRef(new Map<number, TouchContact>());
  const directTouchStartedAtRef = useRef(new Map<number, number>());
  const directTouchReleaseTimersRef = useRef(new Map<number, number>());
  const mappedReleaseTimersRef = useRef(new Map<string, number>());
  const heldPointerBindingsRef = useRef(new Map<number, string>());
  const pointerLockTargetRef = useRef<HTMLElement | null>(null);
  const activeMappingIdsRef = useRef(new Set(options.keymapStatus.active_mapping_ids));
  optionsRef.current = options;
  activeMappingIdsRef.current = new Set(options.keymapStatus.active_mapping_ids);

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
      keys: [...heldRef.current],
      pointer_deltas,
    });
  }, []);

  const sendDirectTouches = useCallback(() => {
    const current = optionsRef.current;
    if (!current.connected || current.controlMode !== "mapping") return;
    current.command({
      type: "keymap_direct_touches",
      contacts: [...directTouchesRef.current.values()],
    });
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
    setDirectTouches([]);
    exitPointerLock();
    if (!current.connected) return;
    for (const usage of forwarded) current.command({ type: "keyboard_up", usage });
    current.command({ type: "keymap_input", keys: [], pointer_deltas: [] });
    current.command({ type: "keymap_direct_touches", contacts: [] });
  }, [collections, exitPointerLock]);

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
      const deltas = pointerMappings(current.profile, activeMappingIdsRef.current).map((mapping) => ({
        mapping_id: mapping.id,
        delta_x: event.movementX,
        delta_y: event.movementY,
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
      const triggered = current.profile.mappings.filter((mapping) => mappingBindings(mapping).includes(event.code));
      if (triggered.some((mapping) => mapping.type === "RawInput")) {
        releaseAllControls();
        current.onControlModeChange("keyboard");
        return;
      }
      const pending = mappedReleaseTimersRef.current.get(event.code);
      if (pending !== undefined) {
        window.clearTimeout(pending);
        mappedReleaseTimersRef.current.delete(event.code);
      }
      heldRef.current.add(event.code);
      heldSinceRef.current.set(event.code, performance.now());
      if (triggered.some((mapping) => pointerMappingTypes.has(mapping.type))
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
  }, [capturePointer, releaseAllControls, releaseMappedKey, sendKeyState]);

  const handlePointerDown = useCallback((event: ReactPointerEvent<HTMLDivElement>) => {
    const current = optionsRef.current;
    if (!current.connected || current.mappingEditing) return;
    const pointerCode = pointerButtonCode(event.button);
    if (current.controlMode === "mapping" && pointerCode && profileUsesKey(current.profile, pointerCode)) {
      event.preventDefault();
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
      const triggered = current.profile.mappings.filter((mapping) => mappingBindings(mapping).includes(pointerCode));
      if (triggered.some((mapping) => pointerMappingTypes.has(mapping.type))) capturePointer(event.currentTarget);
      sendKeyState();
      return;
    }
    if (event.button !== 0 || directTouchesRef.current.has(event.pointerId)) return;
    const used = new Set([
      ...(current.keymapStatus.active_contact_ids ?? []),
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
  }, [capturePointer, sendDirectTouches, sendKeyState]);

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
