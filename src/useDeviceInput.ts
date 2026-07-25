import { useCallback, useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import {
  buildMappingRuntimeFrame,
  buildTouchFrame,
  isBoundKey,
  isUiControl,
  keyboardUsage,
  mappingBindings,
  mergeTouchContacts,
  remainingTapDuration,
  touchFramesEqual,
  type TouchContact,
} from "./control";
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

export type DeviceInputCollections = {
  held: Set<string>;
  heldSince: Map<string, number>;
  mappingOffsets: Map<string, { x: number; y: number }>;
  heldHardware: Map<string, HardwareButtonName>;
  forwardedKeyboard: Map<string, number>;
  directTouches: Map<number, TouchContact>;
  directTouchStartedAt: Map<number, number>;
  directTouchReleaseTimers: Map<number, number>;
};

export function clearDeviceInputCollections(
  collections: DeviceInputCollections,
  cancelReleaseTimer: (timer: number) => void,
) {
  for (const timer of collections.directTouchReleaseTimers.values()) cancelReleaseTimer(timer);
  collections.directTouchReleaseTimers.clear();
  collections.directTouchStartedAt.clear();
  collections.directTouches.clear();
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
  const activeMappingIdsRef = useRef(new Set<string>());
  const lastSentTouchFrameRef = useRef<TouchContact[] | null>(null);
  const schedulerRef = useRef<DemandScheduler | null>(null);
  const sendFrameRef = useRef<(held?: ReadonlySet<string>, released?: TouchContact[]) => void>(() => undefined);
  optionsRef.current = options;

  const inputCollections = useCallback((): DeviceInputCollections => ({
    held: heldRef.current,
    heldSince: heldSinceRef.current,
    mappingOffsets: mappingOffsetsRef.current,
    heldHardware: heldHardwareRef.current,
    forwardedKeyboard: forwardedKeyboardRef.current,
    directTouches: directTouchesRef.current,
    directTouchStartedAt: directTouchStartedAtRef.current,
    directTouchReleaseTimers: directTouchReleaseTimersRef.current,
  }), []);

  const publishActiveMappings = useCallback((next: Set<string>) => {
    if (setsEqual(activeMappingIdsRef.current, next)) return;
    activeMappingIdsRef.current = next;
    setActiveMappingIds(next);
  }, []);

  const sendFrame = useCallback((nextHeld = heldRef.current as ReadonlySet<string>, released: TouchContact[] = []) => {
    const current = optionsRef.current;
    const mappedFrame = buildMappingRuntimeFrame(
      current.mappings,
      nextHeld,
      current.frameSize,
      performance.now(),
      heldSinceRef.current,
      mappingOffsetsRef.current,
    );
    const contacts = mergeTouchContacts(
      mappedFrame.contacts,
      [...directTouchesRef.current.values()],
      released,
    );
    publishActiveMappings(mappedFrame.activeMappingIds);
    if (!current.connected || touchFramesEqual(lastSentTouchFrameRef.current, contacts)) return;
    current.command({ type: "multi_touch", contacts });
    lastSentTouchFrameRef.current = contacts;
  }, [publishActiveMappings]);
  sendFrameRef.current = sendFrame;

  const clearLocalState = useCallback((publish: boolean) => {
    clearDeviceInputCollections(inputCollections(), (timer) => window.clearTimeout(timer));
    schedulerRef.current?.stop();
    lastSentTouchFrameRef.current = null;
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
    for (const name of heldHardware) current.command({ type: "button_up", name });
    for (const usage of forwardedKeyboard) current.command({ type: "keyboard_up", usage });
    setDirectTouches([]);
    sendFrameRef.current(heldRef.current, released);
  }, [inputCollections]);

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

  const wasConnectedRef = useRef(options.connected);
  useEffect(() => {
    if (wasConnectedRef.current && !options.connected) clearLocalState(true);
    wasConnectedRef.current = options.connected;
  }, [clearLocalState, options.connected]);

  useEffect(() => {
    const move = (event: PointerEvent) => {
      const current = optionsRef.current;
      if (!current.connected || current.mappingEditing || current.controlMode !== "mapping" || (!event.movementX && !event.movementY)) return;
      let changed = false;
      for (const mapping of current.mappings) {
        if (!(mapping.type === "Observation" || mapping.type === "Fps" || mapping.type === "Fire" || mapping.type === "MouseCastSpell")) continue;
        const keys = mappingBindings(mapping);
        if (!keys.length || !keys.every((key) => heldRef.current.has(key))) continue;
        const offset = mappingOffsetsRef.current.get(mapping.id) ?? mapping.position;
        const sensitivityX = "sensitivity_x" in mapping ? mapping.sensitivity_x : mapping.horizontal_scale_factor;
        const sensitivityY = "sensitivity_y" in mapping ? mapping.sensitivity_y : mapping.vertical_scale_factor;
        mappingOffsetsRef.current.set(mapping.id, {
          x: Math.max(0, Math.min(1, offset.x + event.movementX * sensitivityX / current.frameSize.width)),
          y: Math.max(0, Math.min(1, offset.y + event.movementY * sensitivityY / current.frameSize.height)),
        });
        changed = true;
      }
      if (changed) sendFrameRef.current();
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
      if (triggered.some((mapping) => mapping.type === "CancelCast")) {
        for (const mapping of current.mappings) {
          if (mapping.type === "MouseCastSpell" || mapping.type === "PadCastSpell") {
            for (const key of mappingBindings(mapping)) {
              heldRef.current.delete(key);
              heldSinceRef.current.delete(key);
            }
          }
        }
        if (heldRef.current.size === 0) schedulerRef.current?.stop();
        sendFrameRef.current();
        return;
      }
      for (const mapping of triggered) {
        if (mapping.type === "Observation" || mapping.type === "Fps" || mapping.type === "Fire" || mapping.type === "MouseCastSpell") {
          mappingOffsetsRef.current.set(mapping.id, mapping.position);
        }
      }
      heldRef.current.add(event.code);
      heldSinceRef.current.set(event.code, performance.now());
      schedulerRef.current?.start();
      sendFrameRef.current();
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
      if (!heldRef.current.delete(event.code)) return;
      heldSinceRef.current.delete(event.code);
      for (const mapping of current.mappings) {
        if (mappingBindings(mapping).includes(event.code)) mappingOffsetsRef.current.delete(mapping.id);
      }
      if (heldRef.current.size === 0) schedulerRef.current?.stop();
      event.preventDefault();
      sendFrameRef.current();
    };

    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    window.addEventListener("blur", releaseAllControls);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
      window.removeEventListener("blur", releaseAllControls);
    };
  }, [releaseAllControls]);

  const handlePointerDown = useCallback((event: ReactPointerEvent<HTMLDivElement>) => {
    const current = optionsRef.current;
    if (!current.connected || current.mappingEditing || event.button !== 0 || directTouchesRef.current.has(event.pointerId)) return;
    const used = new Set([
      ...buildTouchFrame(current.mappings, heldRef.current, current.frameSize).filter((contact) => contact.touching).map((contact) => contact.identity),
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
  }, []);

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
  }, []);

  return {
    activeMappingIds,
    directTouches,
    releaseAllControls,
    handlePointerDown,
    handlePointerMove,
    handlePointerUp,
  };
}
