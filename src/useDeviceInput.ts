import { useCallback, useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import {
  buildMappingRuntimeFrame,
  buildTouchFrame,
  isBoundKey,
  isUiControl,
  keyboardUsage,
  mappingBindings,
  mergeTouchContacts,
  pointerButtonCode,
  remainingTapDuration,
  singleTapReleaseDelay,
  transitionTouchContacts,
  touchFramesEqual,
  type TouchContact,
} from "./control";
import type { AdvancedMapping, AdvancedMappingRuntime } from "./advancedMappingRuntime";
import { createDemandScheduler, type DemandScheduler } from "./inputScheduler";
import { hardwareButtons, type HardwareButtonName, type Mapping, type Profile } from "./types";

export type ControlMode = "mapping" | "keyboard";

export type DeviceInputCommand =
  | { type: "multi_touch"; contacts: TouchContact[] }
  | { type: "button_down" | "button_up"; name: HardwareButtonName }
  | { type: "keyboard_down" | "keyboard_up"; usage: number };

type FrameSize = { width: number; height: number };

type Options = {
  connected: boolean;
  command: (payload: DeviceInputCommand) => void;
  mappings: Mapping[];
  hardwareBindings: Profile["hardwareBindings"];
  frameSize: FrameSize;
  mappingEditing: boolean;
  controlMode: ControlMode;
  onControlModeChange: (mode: ControlMode) => void;
  onContactLimit: () => void;
};

type RuntimeOptions = Options;

const advancedMappingTypes = new Set<Mapping["type"]>(["MouseCastSpell", "PadCastSpell", "CancelCast", "Observation", "Fps", "Fire"]);
const isAdvancedMapping = (mapping: Mapping): mapping is AdvancedMapping => advancedMappingTypes.has(mapping.type);

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
  collections.directTouchReleaseTimers.clear();
  collections.mappedReleaseTimers.clear();
  collections.mappedContactIds.clear();
  collections.directTouchStartedAt.clear();
  collections.directTouches.clear();
  collections.heldPointerBindings.clear();
  collections.held.clear();
  collections.heldSince.clear();
  collections.mappingOffsets.clear();
  collections.heldHardware.clear();
  collections.forwardedKeyboard.clear();
}

function pointFromPointer(event: ReactPointerEvent<HTMLDivElement>) {
  const bounds = event.currentTarget.getBoundingClientRect();
  return {
    x: Math.max(0, Math.min(1, (event.clientX - bounds.left) / bounds.width)),
    y: Math.max(0, Math.min(1, (event.clientY - bounds.top) / bounds.height)),
  };
}

function setsEqual(left: ReadonlySet<string>, right: ReadonlySet<string>) {
  return left.size === right.size && [...left].every((value) => right.has(value));
}

export function useDeviceInput(options: Options) {
  const [activeMappingIds, setActiveMappingIds] = useState<Set<string>>(new Set());
  const [directTouches, setDirectTouches] = useState<TouchContact[]>([]);
  const optionsRef = useRef<RuntimeOptions>(options);
  const heldRef = useRef(new Set<string>());
  const heldSinceRef = useRef(new Map<string, number>());
  const mappingOffsetsRef = useRef(new Map<string, { x: number; y: number }>());
  const heldHardwareRef = useRef(new Map<string, HardwareButtonName>());
  const forwardedKeyboardRef = useRef(new Map<string, number>());
  const directTouchesRef = useRef(new Map<number, TouchContact>());
  const directTouchStartedAtRef = useRef(new Map<number, number>());
  const directTouchReleaseTimersRef = useRef(new Map<number, number>());
  const mappedReleaseTimersRef = useRef(new Map<string, number>());
  const mappedContactIdsRef = useRef(new Map<string, number>());
  const heldPointerBindingsRef = useRef(new Map<number, string>());
  const activeMappingIdsRef = useRef(new Set<string>());
  const advancedRuntimeRef = useRef<AdvancedMappingRuntime | null>(null);
  const pointerLockTargetRef = useRef<HTMLElement | null>(null);
  const lastActiveTouchFrameRef = useRef<TouchContact[] | null>(null);
  const schedulerRef = useRef<DemandScheduler | null>(null);
  const sendFrameRef = useRef<(held?: ReadonlySet<string>, released?: TouchContact[]) => void>(() => undefined);
  optionsRef.current = options;
  advancedRuntimeRef.current?.configure(options.mappings, options.frameSize);

  const inputCollections = useCallback((): DeviceInputCollections => ({
    held: heldRef.current,
    heldSince: heldSinceRef.current,
    mappingOffsets: mappingOffsetsRef.current,
    heldHardware: heldHardwareRef.current,
    forwardedKeyboard: forwardedKeyboardRef.current,
    directTouches: directTouchesRef.current,
    directTouchStartedAt: directTouchStartedAtRef.current,
    directTouchReleaseTimers: directTouchReleaseTimersRef.current,
    mappedReleaseTimers: mappedReleaseTimersRef.current,
    mappedContactIds: mappedContactIdsRef.current,
    heldPointerBindings: heldPointerBindingsRef.current,
  }), []);

  const publishActiveMappings = useCallback((next: Set<string>) => {
    if (setsEqual(activeMappingIdsRef.current, next)) return;
    activeMappingIdsRef.current = next;
    setActiveMappingIds(next);
  }, []);

  const capturePointer = useCallback((target: HTMLElement) => {
    pointerLockTargetRef.current = target;
    const request = target.requestPointerLock?.();
    if (request && "catch" in request) void request.catch(() => {
      if (pointerLockTargetRef.current === target) pointerLockTargetRef.current = null;
    });
  }, []);

  const releasePointerCaptureIfIdle = useCallback(() => {
    const runtime = advancedRuntimeRef.current;
    if (runtime?.needsPointerCapture() || runtime?.hasTimedWork()) return;
    const target = pointerLockTargetRef.current;
    pointerLockTargetRef.current = null;
    if (target && document.pointerLockElement === target) document.exitPointerLock();
  }, []);

  const sendFrame = useCallback((nextHeld = heldRef.current as ReadonlySet<string>, released: TouchContact[] = []) => {
    const current = optionsRef.current;
    const now = performance.now();
    const directContacts = [...directTouchesRef.current.values()];
    const directIdentities = new Set([...directContacts, ...released].map((contact) => contact.identity));
    const advancedFrame = advancedRuntimeRef.current?.frame(nextHeld, now) ?? {
      contacts: [], activeMappingIds: new Set<string>(), blockDirectionPad: false,
    };
    const advancedContacts = advancedFrame.contacts.filter((contact) => !directIdentities.has(contact.identity));
    const advancedIdentities = new Set(advancedContacts.map((contact) => contact.identity));
    const regularMappings = current.mappings.filter((mapping) => !isAdvancedMapping(mapping)
      && !(advancedFrame.blockDirectionPad && mapping.type === "DirectionPad"));
    const mappedFrame = buildMappingRuntimeFrame(
      regularMappings,
      nextHeld,
      current.frameSize,
      now,
      heldSinceRef.current,
      mappingOffsetsRef.current,
      {
        reservedIdentities: new Set([...directIdentities, ...advancedIdentities]),
        assignedIdentities: mappedContactIdsRef.current,
      },
    );
    const activeContacts = mergeTouchContacts(
      [...advancedContacts, ...mappedFrame.contacts],
      directContacts,
    );
    publishActiveMappings(new Set([...advancedFrame.activeMappingIds, ...mappedFrame.activeMappingIds]));
    if (nextHeld.size > 0 || advancedRuntimeRef.current?.hasTimedWork()) schedulerRef.current?.start();
    else schedulerRef.current?.stop();
    releasePointerCaptureIfIdle();
    if (!current.connected) return;
    const previous = lastActiveTouchFrameRef.current ?? [];
    if (released.length === 0 && touchFramesEqual(lastActiveTouchFrameRef.current, activeContacts)) return;
    const contacts = transitionTouchContacts(previous, activeContacts, released);
    current.command({ type: "multi_touch", contacts });
    lastActiveTouchFrameRef.current = activeContacts;
  }, [publishActiveMappings, releasePointerCaptureIfIdle]);
  sendFrameRef.current = sendFrame;

  const clearLocalState = useCallback((publish: boolean) => {
    clearDeviceInputCollections(inputCollections(), (timer) => window.clearTimeout(timer));
    schedulerRef.current?.stop();
    advancedRuntimeRef.current?.reset();
    const pointerLockTarget = pointerLockTargetRef.current;
    pointerLockTargetRef.current = null;
    if (pointerLockTarget && document.pointerLockElement === pointerLockTarget) document.exitPointerLock();
    lastActiveTouchFrameRef.current = null;
    activeMappingIdsRef.current = new Set();
    if (publish) {
      setDirectTouches([]);
      setActiveMappingIds(new Set());
    }
  }, [inputCollections]);

  const releaseAllControls = useCallback(() => {
    const current = optionsRef.current;
    const released = [...directTouchesRef.current.values()].map((contact) => ({ ...contact, touching: false }));
    const heldHardware = [...heldHardwareRef.current.values()];
    const forwardedKeyboard = [...forwardedKeyboardRef.current.values()];
    clearDeviceInputCollections(inputCollections(), (timer) => window.clearTimeout(timer));
    schedulerRef.current?.stop();
    advancedRuntimeRef.current?.reset();
    const pointerLockTarget = pointerLockTargetRef.current;
    pointerLockTargetRef.current = null;
    if (pointerLockTarget && document.pointerLockElement === pointerLockTarget) document.exitPointerLock();
    for (const name of heldHardware) current.command({ type: "button_up", name });
    for (const usage of forwardedKeyboard) current.command({ type: "keyboard_up", usage });
    setDirectTouches([]);
    sendFrameRef.current(heldRef.current, released);
  }, [inputCollections]);

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
    const scheduler = createDemandScheduler(
      (tick, intervalMs) => window.setInterval(tick, intervalMs),
      (handle) => window.clearInterval(handle),
      () => sendFrameRef.current(),
    );
    schedulerRef.current = scheduler;
    return () => {
      scheduler.dispose();
      schedulerRef.current = null;
      clearLocalState(false);
    };
  }, [clearLocalState]);

  const hasAdvancedMappings = options.mappings.some(isAdvancedMapping);
  useEffect(() => {
    if (!hasAdvancedMappings || advancedRuntimeRef.current) return;
    let cancelled = false;
    void import("./advancedMappingRuntime").then(({ AdvancedMappingRuntime: Runtime }) => {
      if (cancelled || advancedRuntimeRef.current) return;
      const runtime = new Runtime();
      const current = optionsRef.current;
      runtime.configure(current.mappings, current.frameSize);
      const replayHeld = new Set<string>();
      const heldInOrder = [...heldRef.current].sort((left, right) =>
        (heldSinceRef.current.get(left) ?? 0) - (heldSinceRef.current.get(right) ?? 0));
      for (const code of heldInOrder) {
        replayHeld.add(code);
        runtime.keyDown(code, replayHeld, performance.now());
      }
      advancedRuntimeRef.current = runtime;
      if (runtime.needsPointerCapture() && document.activeElement instanceof HTMLElement) {
        capturePointer(document.activeElement);
      }
      sendFrameRef.current();
    });
    return () => { cancelled = true; };
  }, [capturePointer, hasAdvancedMappings]);

  const wasConnectedRef = useRef(options.connected);
  useEffect(() => {
    if (wasConnectedRef.current && !options.connected) clearLocalState(true);
    wasConnectedRef.current = options.connected;
  }, [clearLocalState, options.connected]);

  const finishMappedRelease = useCallback((code: string) => {
    mappedReleaseTimersRef.current.delete(code);
    if (!heldRef.current.delete(code)) return;
    heldSinceRef.current.delete(code);
    for (const mapping of optionsRef.current.mappings) {
      if (mappingBindings(mapping).includes(code)) mappingOffsetsRef.current.delete(mapping.id);
    }
    if (heldRef.current.size === 0) schedulerRef.current?.stop();
    sendFrameRef.current();
  }, []);

  const releaseMappedKey = useCallback((code: string) => {
    advancedRuntimeRef.current?.keyUp(code);
    const pending = mappedReleaseTimersRef.current.get(code);
    if (pending !== undefined) window.clearTimeout(pending);
    const mappings = optionsRef.current.mappings;
    const isAdvancedBinding = mappings.some((mapping) => isAdvancedMapping(mapping) && mappingBindings(mapping).includes(code));
    const delay = isAdvancedBinding ? 0 : singleTapReleaseDelay(
      mappings,
      code,
      heldSinceRef.current,
      performance.now(),
    );
    if (delay > 0) {
      mappedReleaseTimersRef.current.set(code, window.setTimeout(() => finishMappedRelease(code), delay));
    } else {
      finishMappedRelease(code);
    }
    releasePointerCaptureIfIdle();
  }, [finishMappedRelease, releasePointerCaptureIfIdle]);

  useEffect(() => {
    const move = (event: PointerEvent) => {
      const current = optionsRef.current;
      if (!current.connected || current.mappingEditing || current.controlMode !== "mapping" || (!event.movementX && !event.movementY)) return;
      if (!advancedRuntimeRef.current?.needsPointerCapture()) return;
      advancedRuntimeRef.current.pointerDelta(event.movementX, event.movementY, performance.now());
      sendFrameRef.current();
    };
    window.addEventListener("pointermove", move);
    return () => window.removeEventListener("pointermove", move);
  }, []);

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
      const hardware = hardwareButtons.find((button) => current.hardwareBindings[button.name] === event.code);
      if (hardware) {
        event.preventDefault();
        heldHardwareRef.current.set(event.code, hardware.name);
        current.command({ type: "button_down", name: hardware.name });
        return;
      }
      if (!isBoundKey(current.mappings, event.code)) return;
      event.preventDefault();
      const triggered = current.mappings.filter((mapping) => mappingBindings(mapping).includes(event.code));
      if (triggered.some((mapping) => mapping.type === "RawInput")) {
        releaseAllControls();
        current.onControlModeChange("keyboard");
        return;
      }
      const pendingRelease = mappedReleaseTimersRef.current.get(event.code);
      if (pendingRelease !== undefined) {
        window.clearTimeout(pendingRelease);
        mappedReleaseTimersRef.current.delete(event.code);
      }
      const now = performance.now();
      const nextHeld = new Set(heldRef.current).add(event.code);
      const advanced = advancedRuntimeRef.current?.keyDown(event.code, nextHeld, now)
        ?? { handled: false, capturePointer: false };
      heldRef.current.add(event.code);
      heldSinceRef.current.set(event.code, now);
      if (advanced.capturePointer && event.target instanceof HTMLElement) capturePointer(event.target);
      schedulerRef.current?.start();
      sendFrameRef.current();
      releasePointerCaptureIfIdle();
    };

    const up = (event: KeyboardEvent) => {
      const current = optionsRef.current;
      const forwardedUsage = forwardedKeyboardRef.current.get(event.code);
      if (forwardedUsage !== undefined) {
        event.preventDefault();
        forwardedKeyboardRef.current.delete(event.code);
        current.command({ type: "keyboard_up", usage: forwardedUsage });
        return;
      }
      const hardware = heldHardwareRef.current.get(event.code);
      if (hardware) {
        event.preventDefault();
        heldHardwareRef.current.delete(event.code);
        current.command({ type: "button_up", name: hardware });
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
  }, [capturePointer, releaseAllControls, releaseMappedKey, releasePointerCaptureIfIdle]);

  const handlePointerDown = useCallback((event: ReactPointerEvent<HTMLDivElement>) => {
    const current = optionsRef.current;
    if (!current.connected || current.mappingEditing) return;
    const pointerCode = pointerButtonCode(event.button);
    if (current.controlMode === "mapping" && pointerCode && isBoundKey(current.mappings, pointerCode)) {
      event.preventDefault();
      event.currentTarget.focus();
      event.currentTarget.setPointerCapture(event.pointerId);
      for (const mapping of current.mappings) {
        if (mappingBindings(mapping).includes(pointerCode)) mappingOffsetsRef.current.delete(mapping.id);
      }
      const pendingRelease = mappedReleaseTimersRef.current.get(pointerCode);
      if (pendingRelease !== undefined) {
        window.clearTimeout(pendingRelease);
        mappedReleaseTimersRef.current.delete(pointerCode);
      }
      heldPointerBindingsRef.current.set(event.pointerId, pointerCode);
      const now = performance.now();
      const nextHeld = new Set(heldRef.current).add(pointerCode);
      const advanced = advancedRuntimeRef.current?.keyDown(pointerCode, nextHeld, now)
        ?? { handled: false, capturePointer: false };
      heldRef.current.add(pointerCode);
      heldSinceRef.current.set(pointerCode, now);
      if (advanced.capturePointer) capturePointer(event.currentTarget);
      schedulerRef.current?.start();
      sendFrameRef.current();
      releasePointerCaptureIfIdle();
      return;
    }
    if (event.button !== 0 || directTouchesRef.current.has(event.pointerId)) return;
    const now = performance.now();
    const advancedContacts = advancedRuntimeRef.current?.frame(heldRef.current, now).contacts ?? [];
    const regularMappings = current.mappings.filter((mapping) => !isAdvancedMapping(mapping));
    const used = new Set([
      ...(lastActiveTouchFrameRef.current ?? [...advancedContacts, ...buildTouchFrame(regularMappings, heldRef.current, current.frameSize)]).map((contact) => contact.identity),
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
    sendFrameRef.current();
  }, [capturePointer, releasePointerCaptureIfIdle]);

  const handlePointerMove = useCallback((event: ReactPointerEvent<HTMLDivElement>) => {
    const contact = directTouchesRef.current.get(event.pointerId);
    if (!contact) return;
    event.preventDefault();
    const moved = { ...contact, ...pointFromPointer(event) };
    directTouchesRef.current.set(event.pointerId, moved);
    setDirectTouches([...directTouchesRef.current.values()]);
    sendFrameRef.current();
  }, []);

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
      const latest = directTouchesRef.current.get(pointerId);
      if (!latest || latest.identity !== finalContact.identity) return;
      directTouchReleaseTimersRef.current.delete(pointerId);
      directTouchStartedAtRef.current.delete(pointerId);
      directTouchesRef.current.delete(pointerId);
      setDirectTouches([...directTouchesRef.current.values()]);
      sendFrameRef.current(heldRef.current, [{ ...finalContact, touching: false }]);
    };
    const delay = remainingTapDuration(
      directTouchStartedAtRef.current.get(pointerId) ?? performance.now(),
      performance.now(),
    );
    if (delay > 0) directTouchReleaseTimersRef.current.set(pointerId, window.setTimeout(finish, delay));
    else finish();
  }, [releaseMappedKey]);

  return {
    activeMappingIds,
    directTouches,
    releaseAllControls,
    handlePointerDown,
    handlePointerMove,
    handlePointerUp,
  };
}
