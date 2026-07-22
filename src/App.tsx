import {
  AimOutlined,
  ApiOutlined,
  AudioMutedOutlined,
  CompressOutlined,
  CustomerServiceOutlined,
  ExpandOutlined,
  EyeInvisibleOutlined,
  EyeOutlined,
  FullscreenExitOutlined,
  FullscreenOutlined,
  HomeOutlined,
  KeyOutlined,
  LockOutlined,
  MenuFoldOutlined,
  MenuUnfoldOutlined,
  MinusOutlined,
  PushpinFilled,
  PushpinOutlined,
  PlusOutlined,
  ReloadOutlined,
  RotateLeftOutlined,
  RotateRightOutlined,
  SaveOutlined,
  SyncOutlined,
  ThunderboltOutlined,
} from "@ant-design/icons";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Button, Segmented, Select, Space, Switch, Tag, Tooltip, Typography, message } from "antd";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { useTranslation } from "react-i18next";
import { AppNavigation, type AppPage } from "./components/AppNavigation";
import { DeviceInspector } from "./components/DeviceInspector";
import { LocationPage } from "./components/LocationPage";
import { MappingBackgroundToolbar, type MappingBackgroundMode } from "./components/MappingBackgroundToolbar";
import { MappingInspector } from "./components/MappingInspector";
import { MappingOverlay } from "./components/MappingOverlay";
import { ProfileManager } from "./components/ProfileManager";
import { SettingsPage } from "./components/SettingsPage";
import { buildTouchFrame, isBoundKey, keyboardUsage, mappingBindings, touchFramesEqual, type TouchContact } from "./control";
import { logFrontend } from "./diagnostics";
import { createMapping, defaultHardwareBindings, defaultProfile, hardwareButtons, type DeviceStatus, type HardwareButtonName, type Mapping, type Orientation, type Profile, type ScrcpyMappingType, type StreamMetrics } from "./types";

const emptyStatus: DeviceStatus = {
  status: "",
  active_udid: null,
  error: null,
  orientation: "portrait",
  devices: [],
  location: { available: false, active: false, latitude: null, longitude: null, error: null },
};
const emptyMetrics: StreamMetrics = {
  source_fps: 0,
  decoded_fps: 0,
  published_fps: 0,
  sent_fps: 0,
  backend_dropped_fps: 0,
  jpeg_encode_ms: 0,
  frame_age_ms: 0,
  websocket_send_ms: 0,
  presentation_ack_ms: 0,
  megabits_per_second: 0,
};

type BackendConnection = { origin: string; token: string };
type ProfileList = { profiles: string[]; active: string };
type CapturedScreenshot = { blob: Blob; url: string; width: number; height: number };

function wsUrl(connection: BackendConnection) {
  return `${connection.origin.replace(/^http/, "ws")}/api/ws`;
}

function drawFrame(canvas: HTMLCanvasElement, context: CanvasRenderingContext2D, bitmap: ImageBitmap, orientation: Orientation) {
  const landscape = orientation === "landscape_left" || orientation === "landscape_right";
  const width = landscape ? bitmap.height : bitmap.width;
  const height = landscape ? bitmap.width : bitmap.height;
  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width;
    canvas.height = height;
  }
  context.save();
  if (orientation === "landscape_right") {
    context.translate(canvas.width, 0);
    context.rotate(Math.PI / 2);
  } else if (orientation === "landscape_left") {
    context.translate(0, canvas.height);
    context.rotate(-Math.PI / 2);
  } else if (orientation === "portrait_upside_down") {
    context.translate(canvas.width, canvas.height);
    context.rotate(Math.PI);
  }
  context.drawImage(bitmap, 0, 0);
  context.restore();
  return { width: canvas.width, height: canvas.height };
}

function containSize(containerWidth: number, containerHeight: number, contentWidth: number, contentHeight: number) {
  if (containerWidth <= 0 || containerHeight <= 0 || contentWidth <= 0 || contentHeight <= 0) {
    return { width: 0, height: 0 };
  }
  const scale = Math.min(containerWidth / contentWidth, containerHeight / contentHeight);
  return { width: contentWidth * scale, height: contentHeight * scale };
}

function canvasPng(canvas: HTMLCanvasElement) {
  return new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/png"));
}

function screenshotFilename(deviceName: string, width: number, height: number) {
  const safeName = deviceName.trim().replace(/[<>:"/\\|?*]+/g, "-") || "iPhone";
  const timestamp = new Date().toISOString().replace(/[:.]/g, "-");
  return `devicehub-mask_${safeName}_${width}x${height}_${timestamp}.png`;
}

type ControlMode = "mapping" | "keyboard";

function isUiControl(target: EventTarget | null) {
  return target instanceof HTMLElement
    && target.closest("input, textarea, select, button, [contenteditable='true'], .ant-segmented") !== null;
}

function createLocalizedDefaultProfile(t: (key: string, options?: Record<string, unknown>) => string): Profile {
  const labels = ["mapping.defaults.move", "mapping.defaults.skill1", "mapping.defaults.skill2", "mapping.defaults.skill3"];
  return {
    ...defaultProfile,
    hardwareBindings: { ...defaultHardwareBindings },
    mappings: defaultProfile.mappings.map((mapping, index) => ({ ...mapping, label: t(labels[index]) })) as Mapping[],
  };
}

const backendStatusKeys: Record<string, string> = {
  "no device - pick one from the menu": "backendStatus.noDevice",
  "connecting to device...": "backendStatus.connecting",
  "starting screen media stream...": "backendStatus.startingStream",
  "connecting HID...": "backendStatus.connectingHid",
  "device management connected": "backendStatus.managementConnected",
  connected: "backendStatus.connected",
  "stopping...": "backendStatus.stopping",
};

export default function App() {
  const { t } = useTranslation();
  const translateRef = useRef(t);
  translateRef.current = t;
  const appWindow = useMemo(() => getCurrentWindow(), []);
  const [backend, setBackend] = useState<BackendConnection | null>(null);
  const [page, setPage] = useState<AppPage>("device");
  const [status, setStatus] = useState<DeviceStatus>(() => ({ ...emptyStatus, status: t("status.starting") }));
  const [profile, setProfile] = useState<Profile>(() => createLocalizedDefaultProfile(t));
  const initialProfileRef = useRef(profile);
  const [controlProfile, setControlProfile] = useState<Profile>(profile);
  const [profiles, setProfiles] = useState<string[]>([]);
  const [activeProfile, setActiveProfile] = useState("default");
  const [selectedId, setSelectedId] = useState<string | null>("move");
  const [editing, setEditing] = useState(true);
  const [controlMode, setControlMode] = useState<ControlMode>("mapping");
  const [alwaysOnTop, setAlwaysOnTop] = useState(false);
  const [systemFullscreen, setSystemFullscreen] = useState(false);
  const [deviceFullscreen, setDeviceFullscreen] = useState(false);
  const [controlOverlayVisible, setControlOverlayVisible] = useState(true);
  const [selectedUdid, setSelectedUdid] = useState<string | null>(null);
  const [inspectorVisible, setInspectorVisible] = useState(true);
  const [connected, setConnected] = useState(false);
  const [streamMetrics, setStreamMetrics] = useState<StreamMetrics>(emptyMetrics);
  const [renderFps, setRenderFps] = useState(0);
  const [activeIds, setActiveIds] = useState<Set<number>>(new Set());
  const [directTouches, setDirectTouches] = useState<TouchContact[]>([]);
  const [frameSize, setFrameSize] = useState({ width: 1296, height: 2816 });
  const [hasFrame, setHasFrame] = useState(false);
  const [mappingBackgroundMode, setMappingBackgroundMode] = useState<MappingBackgroundMode>("live");
  const [capturedScreenshot, setCapturedScreenshot] = useState<CapturedScreenshot | null>(null);
  const [stageSize, setStageSize] = useState({ width: 0, height: 0 });
  const stageRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const canvasContextRef = useRef<CanvasRenderingContext2D | null>(null);
  const renderedFramesRef = useRef(0);
  const socketRef = useRef<WebSocket | null>(null);
  const orientationRef = useRef<Orientation>("portrait");
  const heldRef = useRef(new Set<string>());
  const heldSinceRef = useRef(new Map<string, number>());
  const mappingOffsetsRef = useRef(new Map<string, { x: number; y: number }>());
  const heldHardwareRef = useRef(new Map<string, HardwareButtonName>());
  const forwardedKeyboardRef = useRef(new Map<string, number>());
  const directTouchesRef = useRef(new Map<number, TouchContact>());
  const activeIdsRef = useRef(new Set<number>());
  const lastSentTouchFrameRef = useRef<TouchContact[] | null>(null);
  const capturedScreenshotRef = useRef<CapturedScreenshot | null>(null);
  const hasFrameRef = useRef(false);

  orientationRef.current = status.orientation;
  useEffect(() => {
    if (status.active_udid) setSelectedUdid(status.active_udid);
  }, [status.active_udid]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const syncSystemFullscreen = async () => {
      try {
        const value = await appWindow.isFullscreen();
        if (!disposed) {
          setSystemFullscreen(value);
        }
      } catch (error) {
        logFrontend("warn", "window", "read_system_fullscreen", error);
      }
    };
    void syncSystemFullscreen();
    void appWindow.onResized(() => void syncSystemFullscreen()).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [appWindow]);
  const mappingEditing = page === "mappings" && controlMode === "mapping" && editing;
  const mappingFrameSize = mappingBackgroundMode === "screenshot" && capturedScreenshot
    ? { width: capturedScreenshot.width, height: capturedScreenshot.height }
    : frameSize;

  const request = useCallback((path: string, init: RequestInit = {}) => {
    if (!backend) return Promise.reject(new Error(translateRef.current("errors.backendNotReady")));
    const headers = new Headers(init.headers);
    headers.set("authorization", `Bearer ${backend.token}`);
    return fetch(`${backend.origin}${path}`, { ...init, headers });
  }, [backend]);

  const command = useCallback((payload: unknown) => {
    if (socketRef.current?.readyState === WebSocket.OPEN) {
      socketRef.current.send(JSON.stringify(payload));
    }
  }, []);

  const sendFrame = useCallback((nextHeld = heldRef.current, released: TouchContact[] = []) => {
    const candidates = [
      ...buildTouchFrame(controlProfile.mappings, nextHeld, frameSize, performance.now(), heldSinceRef.current, mappingOffsetsRef.current),
      ...directTouchesRef.current.values(),
      ...released,
    ];
    const ordered = [...candidates.filter((contact) => contact.touching), ...candidates.filter((contact) => !contact.touching)];
    const contacts = ordered.filter((contact, index, all) => all.findIndex((candidate) => candidate.identity === contact.identity) === index).slice(0, 5);
    const nextActiveIds = new Set(contacts.filter((contact) => contact.touching).map((contact) => contact.identity));
    if (nextActiveIds.size !== activeIdsRef.current.size || [...nextActiveIds].some((identity) => !activeIdsRef.current.has(identity))) {
      activeIdsRef.current = nextActiveIds;
      setActiveIds(nextActiveIds);
    }
    const socket = socketRef.current;
    if (socket?.readyState !== WebSocket.OPEN || touchFramesEqual(lastSentTouchFrameRef.current, contacts)) return;
    socket.send(JSON.stringify({ type: "multi_touch", contacts }));
    lastSentTouchFrameRef.current = contacts;
  }, [controlProfile.mappings, frameSize]);

  const releaseAllControls = useCallback(() => {
    const released = [...directTouchesRef.current.values()].map((contact) => ({ ...contact, touching: false }));
    directTouchesRef.current.clear();
    heldRef.current.clear();
    heldSinceRef.current.clear();
    mappingOffsetsRef.current.clear();
    for (const name of heldHardwareRef.current.values()) {
      command({ type: "button_up", name });
    }
    heldHardwareRef.current.clear();
    for (const usage of forwardedKeyboardRef.current.values()) {
      command({ type: "keyboard_up", usage });
    }
    forwardedKeyboardRef.current.clear();
    setDirectTouches([]);
    sendFrame(heldRef.current, released);
  }, [command, sendFrame]);

  useEffect(() => {
    const timer = window.setInterval(() => { if (heldRef.current.size) sendFrame(); }, 16);
    return () => clearInterval(timer);
  }, [sendFrame]);

  useEffect(() => {
    const move = (event: PointerEvent) => {
      if (mappingEditing || controlMode !== "mapping" || (!event.movementX && !event.movementY)) return;
      let changed = false;
      for (const mapping of controlProfile.mappings) {
        if (!(mapping.type === "Observation" || mapping.type === "Fps" || mapping.type === "Fire" || mapping.type === "MouseCastSpell")) continue;
        const keys = mappingBindings(mapping);
        if (!keys.length || !keys.every((key) => heldRef.current.has(key))) continue;
        const current = mappingOffsetsRef.current.get(mapping.id) ?? mapping.position;
        const sensitivityX = "sensitivity_x" in mapping ? mapping.sensitivity_x : mapping.horizontal_scale_factor;
        const sensitivityY = "sensitivity_y" in mapping ? mapping.sensitivity_y : mapping.vertical_scale_factor;
        mappingOffsetsRef.current.set(mapping.id, {
          x: Math.max(0, Math.min(1, current.x + event.movementX * sensitivityX / frameSize.width)),
          y: Math.max(0, Math.min(1, current.y + event.movementY * sensitivityY / frameSize.height)),
        });
        changed = true;
      }
      if (changed) sendFrame();
    };
    window.addEventListener("pointermove", move);
    return () => window.removeEventListener("pointermove", move);
  }, [controlMode, controlProfile.mappings, frameSize, mappingEditing, sendFrame]);

  useEffect(() => {
    invoke<BackendConnection>("backend_connection")
      .then((connection) => {
        logFrontend("info", "backend", "connection_ready", "Private backend connection acquired");
        setBackend(connection);
      })
      .catch((error) => {
        logFrontend("error", "backend", "connection_failed", error);
        setStatus({ ...emptyStatus, status: translateRef.current("status.backendUnavailable"), error: String(error) });
      });
    Promise.all([appWindow.isAlwaysOnTop(), appWindow.isFullscreen()])
      .then(([top, full]) => { setAlwaysOnTop(top); setSystemFullscreen(full); })
      .catch(() => undefined);
  }, [appWindow]);

  useEffect(() => {
    let measuredAt = performance.now();
    const timer = window.setInterval(() => {
      const now = performance.now();
      const elapsed = Math.max((now - measuredAt) / 1000, Number.EPSILON);
      setRenderFps(renderedFramesRef.current / elapsed);
      renderedFramesRef.current = 0;
      measuredAt = now;
    }, 1000);
    return () => clearInterval(timer);
  }, []);

  useLayoutEffect(() => {
    const stage = stageRef.current;
    if (!stage) return;
    const update = (width: number, height: number) => {
      setStageSize((current) => current.width === width && current.height === height ? current : { width, height });
    };
    const observer = new ResizeObserver(([entry]) => {
      if (entry) update(entry.contentRect.width, entry.contentRect.height);
    });
    observer.observe(stage);
    const bounds = stage.getBoundingClientRect();
    const styles = getComputedStyle(stage);
    update(
      bounds.width - parseFloat(styles.paddingLeft) - parseFloat(styles.paddingRight),
      bounds.height - parseFloat(styles.paddingTop) - parseFloat(styles.paddingBottom),
    );
    return () => observer.disconnect();
  }, [page]);

  useEffect(() => {
    if (!backend) return;
    let disposed = false;
    let retry: number | undefined;
    const open = () => {
      const socket = new WebSocket(wsUrl(backend), ["devicehub-mask", backend.token]);
      let socketClosed = false;
      let pendingFrame: Blob | null = null;
      let decoding = false;
      let metricsTimer: number | undefined;
      let frontendMetrics = {
        startedAt: performance.now(),
        receivedFrames: 0,
        replacedFrames: 0,
        presentedFrames: 0,
        jpegDecodeMs: 0,
        canvasDrawMs: 0,
        decodeErrors: 0,
      };
      const flushFrontendMetrics = () => {
        const now = performance.now();
        if (socket.readyState === WebSocket.OPEN) {
          socket.send(JSON.stringify({
            type: "frontend_metrics",
            window_ms: now - frontendMetrics.startedAt,
            received_frames: frontendMetrics.receivedFrames,
            replaced_frames: frontendMetrics.replacedFrames,
            presented_frames: frontendMetrics.presentedFrames,
            jpeg_decode_ms: frontendMetrics.jpegDecodeMs,
            canvas_draw_ms: frontendMetrics.canvasDrawMs,
            decode_errors: frontendMetrics.decodeErrors,
          }));
        }
        frontendMetrics = {
          startedAt: now,
          receivedFrames: 0,
          replacedFrames: 0,
          presentedFrames: 0,
          jpegDecodeMs: 0,
          canvasDrawMs: 0,
          decodeErrors: 0,
        };
      };
      socket.binaryType = "blob";
      socket.onopen = () => {
        logFrontend("info", "websocket", "opened", "Video and control socket connected");
        socketRef.current = socket;
        setConnected(true);
        metricsTimer = window.setInterval(flushFrontendMetrics, 5_000);
      };
      socket.onerror = () => logFrontend("warn", "websocket", "transport_error", "WebSocket transport error");
      socket.onclose = (event) => {
        logFrontend(
          disposed ? "debug" : "warn",
          "websocket",
          "closed",
          `code=${event.code} clean=${event.wasClean} reason=${event.reason || "none"}`,
        );
        socketClosed = true;
        if (metricsTimer !== undefined) window.clearInterval(metricsTimer);
        pendingFrame = null;
        lastSentTouchFrameRef.current = null;
        activeIdsRef.current = new Set();
        setActiveIds(new Set());
        if (socketRef.current === socket) socketRef.current = null;
        setConnected(false);
        setStreamMetrics(emptyMetrics);
        if (!disposed) retry = window.setTimeout(open, 800);
      };
      const drainFrames = async () => {
        if (decoding) return;
        decoding = true;
        try {
          while (pendingFrame && !disposed && !socketClosed) {
            const blob = pendingFrame;
            pendingFrame = null;
            let bitmap: ImageBitmap | null = null;
            try {
              const decodeStarted = performance.now();
              bitmap = await createImageBitmap(blob);
              frontendMetrics.jpegDecodeMs += performance.now() - decodeStarted;
              if (disposed || socketClosed) continue;
              const canvas = canvasRef.current;
              if (!canvas) continue;
              const context = canvasContextRef.current ?? canvas.getContext("2d", { alpha: false });
              if (!context) continue;
              canvasContextRef.current = context;
              const drawStarted = performance.now();
              const size = drawFrame(canvas, context, bitmap, orientationRef.current);
              frontendMetrics.canvasDrawMs += performance.now() - drawStarted;
              frontendMetrics.presentedFrames += 1;
              renderedFramesRef.current += 1;
              if (!hasFrameRef.current) {
                hasFrameRef.current = true;
                setHasFrame(true);
              }
              setFrameSize((current) => current.width === size.width && current.height === size.height ? current : size);
            } catch (error) {
              frontendMetrics.decodeErrors += 1;
              logFrontend("warn", "video", "decode_frame", error);
            } finally {
              bitmap?.close();
              if (socket.readyState === WebSocket.OPEN) {
                socket.send(JSON.stringify({ type: "frame_presented" }));
              }
            }
          }
        } finally {
          decoding = false;
          if (pendingFrame && !disposed && !socketClosed) void drainFrames();
        }
      };
      socket.onmessage = (event) => {
        if (typeof event.data === "string") {
          const data = JSON.parse(event.data) as { type: string; payload: DeviceStatus | StreamMetrics };
          if (data.type === "status") setStatus(data.payload as DeviceStatus);
          if (data.type === "metrics") setStreamMetrics(data.payload as StreamMetrics);
          return;
        }
        frontendMetrics.receivedFrames += 1;
        if (pendingFrame) {
          frontendMetrics.replacedFrames += 1;
          if (socket.readyState === WebSocket.OPEN) {
            socket.send(JSON.stringify({ type: "frame_presented" }));
          }
        }
        pendingFrame = event.data as Blob;
        void drainFrames();
      };
    };
    open();
    return () => { disposed = true; if (retry) clearTimeout(retry); socketRef.current?.close(); };
  }, [backend]);

  useEffect(() => () => {
    if (capturedScreenshotRef.current) URL.revokeObjectURL(capturedScreenshotRef.current.url);
  }, []);

  const captureMappingScreenshot = useCallback(async (selectBackground: boolean) => {
    const canvas = canvasRef.current;
    if (!canvas || !hasFrame) {
      void message.warning(t("mapping.screenshotUnavailable"));
      return null;
    }
    const blob = await canvasPng(canvas);
    if (!blob) {
      void message.error(t("mapping.screenshotFailed"));
      return null;
    }
    const next = {
      blob,
      url: URL.createObjectURL(blob),
      width: canvas.width,
      height: canvas.height,
    };
    const previous = capturedScreenshotRef.current;
    capturedScreenshotRef.current = next;
    setCapturedScreenshot(next);
    if (selectBackground) setMappingBackgroundMode("screenshot");
    if (previous) URL.revokeObjectURL(previous.url);
    void message.success(t("mapping.screenshotCaptured"));
    return next;
  }, [hasFrame, t]);

  const saveMappingScreenshot = useCallback(async () => {
    const screenshot = mappingBackgroundMode === "live"
      ? await captureMappingScreenshot(false)
      : capturedScreenshotRef.current ?? await captureMappingScreenshot(false);
    if (!screenshot) return;
    const deviceName = status.devices.find((device) => device.udid === status.active_udid)?.name ?? "iPhone";
    const link = document.createElement("a");
    link.href = screenshot.url;
    link.download = screenshotFilename(deviceName, screenshot.width, screenshot.height);
    document.body.appendChild(link);
    link.click();
    link.remove();
    void message.success(t("mapping.screenshotSaved"));
  }, [captureMappingScreenshot, mappingBackgroundMode, status.active_udid, status.devices, t]);

  useEffect(() => {
    const down = (event: KeyboardEvent) => {
      if (event.ctrlKey && event.shiftKey && event.code === "KeyK") {
        event.preventDefault();
        releaseAllControls();
        setControlMode((current) => current === "mapping" ? "keyboard" : "mapping");
        setEditing(false);
        return;
      }
      if (controlMode === "keyboard") {
        if (event.repeat || isUiControl(event.target)) return;
        const usage = keyboardUsage(event.code);
        if (usage === undefined) return;
        event.preventDefault();
        forwardedKeyboardRef.current.set(event.code, usage);
        command({ type: "keyboard_down", usage });
        return;
      }
      if (mappingEditing || event.repeat) return;
      const hardware = hardwareButtons.find((button) => controlProfile.hardwareBindings[button.name] === event.code);
      if (hardware) {
        event.preventDefault();
        heldHardwareRef.current.set(event.code, hardware.name);
        command({ type: "button_down", name: hardware.name });
        return;
      }
      if (!isBoundKey(controlProfile.mappings, event.code)) return;
      event.preventDefault();
      const triggered = controlProfile.mappings.filter((mapping) => mappingBindings(mapping).includes(event.code));
      if (triggered.some((mapping) => mapping.type === "RawInput")) {
        releaseAllControls();
        setControlMode("keyboard");
        setEditing(false);
        return;
      }
      if (triggered.some((mapping) => mapping.type === "CancelCast")) {
        for (const mapping of controlProfile.mappings) {
          if (mapping.type === "MouseCastSpell" || mapping.type === "PadCastSpell") {
            for (const key of mappingBindings(mapping)) { heldRef.current.delete(key); heldSinceRef.current.delete(key); }
          }
        }
        sendFrame();
        return;
      }
      for (const mapping of triggered) {
        if (mapping.type === "Observation" || mapping.type === "Fps" || mapping.type === "Fire" || mapping.type === "MouseCastSpell") mappingOffsetsRef.current.set(mapping.id, mapping.position);
      }
      heldRef.current.add(event.code);
      heldSinceRef.current.set(event.code, performance.now());
      sendFrame();
    };
    const up = (event: KeyboardEvent) => {
      const forwardedUsage = forwardedKeyboardRef.current.get(event.code);
      if (forwardedUsage !== undefined) {
        event.preventDefault();
        forwardedKeyboardRef.current.delete(event.code);
        command({ type: "keyboard_up", usage: forwardedUsage });
        return;
      }
      const hardware = heldHardwareRef.current.get(event.code);
      if (hardware) {
        event.preventDefault();
        heldHardwareRef.current.delete(event.code);
        command({ type: "button_up", name: hardware });
        return;
      }
      if (!heldRef.current.delete(event.code)) return;
      heldSinceRef.current.delete(event.code);
      for (const mapping of controlProfile.mappings) if (mappingBindings(mapping).includes(event.code)) mappingOffsetsRef.current.delete(mapping.id);
      event.preventDefault();
      sendFrame();
    };
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    window.addEventListener("blur", releaseAllControls);
    return () => { window.removeEventListener("keydown", down); window.removeEventListener("keyup", up); window.removeEventListener("blur", releaseAllControls); };
  }, [command, controlMode, controlProfile.hardwareBindings, controlProfile.mappings, mappingEditing, releaseAllControls, sendFrame]);

  const readProfile = useCallback(async (name: string) => {
    const response = await request(`/api/profiles/${encodeURIComponent(name)}`);
    if (!response.ok) throw new Error(translateRef.current("errors.readProfile", { status: response.status }));
    const loaded = await response.json() as Profile;
    return {
      ...loaded,
      name,
      hardwareBindings: { ...defaultHardwareBindings, ...loaded.hardwareBindings },
    } as Profile;
  }, [request]);

  const loadProfile = useCallback(async (name: string) => {
    const loaded = await readProfile(name);
    setProfile(loaded);
    setSelectedId(loaded.mappings[0]?.id ?? null);
  }, [readProfile]);

  const refreshProfiles = useCallback(async () => {
    const response = await request("/api/profiles");
    if (!response.ok) throw new Error(translateRef.current("errors.readProfiles", { status: response.status }));
    const list = await response.json() as ProfileList;
    setProfiles(list.profiles);
    setActiveProfile(list.active);
    return list;
  }, [request]);

  const writeProfile = useCallback(async (name: string, value: Profile) => {
    const response = await request(`/api/profiles/${encodeURIComponent(name)}`, {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ ...value, name }),
    });
    if (!response.ok) throw new Error(translateRef.current("errors.saveProfile", { status: response.status }));
  }, [request]);

  useEffect(() => {
    if (!backend) return;
    const initializeProfiles = async () => {
      const list = await refreshProfiles();
      if (list.profiles.length === 0) {
        const initialProfile = initialProfileRef.current;
        await writeProfile("default", initialProfile);
        await request("/api/profiles/default/activate", { method: "PUT" });
        setProfiles(["default"]);
        setActiveProfile("default");
        setProfile(initialProfile);
        setControlProfile(initialProfile);
        return;
      }
      const selected = list.profiles.includes(list.active) ? list.active : list.profiles[0];
      const loaded = await readProfile(selected);
      setProfile(loaded);
      setControlProfile(loaded);
      setSelectedId(loaded.mappings[0]?.id ?? null);
    };
    void initializeProfiles().catch((error) => message.error(String(error)));
  }, [backend, readProfile, refreshProfiles, request, writeProfile]);

  const updateMapping = (next: Mapping) => setProfile((current) => {
    const keyConflict = hardwareButtons.some((button) => {
      const key = current.hardwareBindings[button.name];
      return key && isBoundKey([next], key);
    });
    if (keyConflict) {
      void message.warning(t("mapping.keyUsedByHardware"));
      return current;
    }
    return { ...current, mappings: current.mappings.map((mapping) => mapping.id === next.id ? next : mapping) };
  });
  const updateHardwareBinding = (name: HardwareButtonName, key: string) => setProfile((current) => {
    if (key && isBoundKey(current.mappings, key)) {
      void message.warning(t("mapping.keyUsedByTouch"));
      return current;
    }
    if (key && hardwareButtons.some((button) => button.name !== name && current.hardwareBindings[button.name] === key)) {
      void message.warning(t("mapping.keyUsedByOtherHardware"));
      return current;
    }
    return { ...current, hardwareBindings: { ...current.hardwareBindings, [name]: key } };
  });
  const moveMapping = (id: string, x: number, y: number) => setProfile((current) => ({ ...current, mappings: current.mappings.map((mapping) => mapping.id === id ? ("position" in mapping ? { ...mapping, position: { x, y } } : { ...mapping, x, y }) as Mapping : mapping) }));
  const addMapping = (type: ScrcpyMappingType) => {
    const next = createMapping(type, { x: 0.5, y: 0.5 }, mappingFrameSize);
    const id = next.id;
    setProfile((current) => ({ ...current, mappings: [...current.mappings, next] }));
    setSelectedId(id);
  };
  const deleteMapping = (id: string) => {
    setProfile((current) => ({ ...current, mappings: current.mappings.filter((mapping) => mapping.id !== id) }));
    setSelectedId(null);
  };
  const save = async () => {
    try {
      await writeProfile(profile.name, profile);
      await refreshProfiles();
      if (activeProfile === profile.name) {
        releaseAllControls();
        setControlProfile(profile);
      }
      void message.success(t("mapping.saved"));
    } catch (error) {
      void message.error(String(error));
    }
  };
  const activateCurrentProfile = async () => {
    releaseAllControls();
    const response = await request(`/api/profiles/${encodeURIComponent(profile.name)}/activate`, { method: "PUT" });
    if (!response.ok) throw new Error(t("errors.activateProfile", { status: response.status }));
    setActiveProfile(profile.name);
    setControlProfile(profile);
    void message.success(t("mapping.activated"));
  };
  const createProfile = async (name: string) => {
    const next: Profile = { ...defaultProfile, name, mappings: [], hardwareBindings: { ...defaultHardwareBindings } };
    await writeProfile(name, next);
    await refreshProfiles();
    await loadProfile(name);
  };
  const duplicateProfile = async (name: string) => {
    await writeProfile(name, { ...profile, name });
    await refreshProfiles();
    await loadProfile(name);
  };
  const renameProfile = async (name: string) => {
    const oldName = profile.name;
    if (name === oldName) return;
    await writeProfile(name, { ...profile, name });
    if (activeProfile === oldName) {
      releaseAllControls();
      const response = await request(`/api/profiles/${encodeURIComponent(name)}/activate`, { method: "PUT" });
      if (!response.ok) throw new Error(t("errors.activateProfile", { status: response.status }));
      setActiveProfile(name);
      setControlProfile({ ...profile, name });
    }
    const response = await request(`/api/profiles/${encodeURIComponent(oldName)}/delete`, { method: "PUT" });
    if (!response.ok) throw new Error(t("errors.deleteOldProfile", { status: response.status }));
    await refreshProfiles();
    await loadProfile(name);
  };
  const deleteCurrentProfile = async () => {
    const response = await request(`/api/profiles/${encodeURIComponent(profile.name)}/delete`, { method: "PUT" });
    if (!response.ok) throw new Error(t("errors.deleteProfile", { status: response.status }));
    setProfiles((current) => current.filter((name) => name !== profile.name));
    setProfile(controlProfile);
    setSelectedId(controlProfile.mappings[0]?.id ?? null);
  };
  const importProfile = async (next: Profile, imported: number, skipped: number) => {
    await writeProfile(next.name, next);
    await refreshProfiles();
    await loadProfile(next.name);
    void message.success(t(skipped ? "mapping.importedWithSkipped" : "mapping.imported", { imported, skipped }));
  };
  const toggleAlwaysOnTop = async () => {
    const next = !alwaysOnTop;
    try {
      await appWindow.setAlwaysOnTop(next);
      setAlwaysOnTop(next);
    } catch (error) {
      void message.error(t("errors.windowTop", { error: String(error) }));
    }
  };
  const toggleSystemFullscreen = async () => {
    const next = !systemFullscreen;
    releaseAllControls();
    try {
      await appWindow.setFullscreen(next);
      setSystemFullscreen(next);
    } catch (error) {
      void message.error(t("errors.systemFullscreen", { error: String(error) }));
    }
  };
  const toggleDeviceFullscreen = () => {
    releaseAllControls();
    setDeviceFullscreen((active) => !active);
    setPage("device");
  };
  const connectDevice = async (udid: string) => {
    setSelectedUdid(udid);
    releaseAllControls();
    try {
      const response = await request(`/api/devices/${encodeURIComponent(udid)}/connect`, { method: "PUT" });
      if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
    } catch (error) {
      void message.error(t("errors.reconnectDevice", { error: String(error) }));
    }
  };
  const reconnectDevice = async () => {
    if (!selectedUdid) return;
    releaseAllControls();
    try {
      const response = await request(`/api/devices/${encodeURIComponent(selectedUdid)}/reconnect`, { method: "PUT" });
      if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
    } catch (error) {
      void message.error(t("errors.reconnectDevice", { error: String(error) }));
    }
  };
  const pointFromPointer = (event: ReactPointerEvent<HTMLDivElement>) => {
    const bounds = event.currentTarget.getBoundingClientRect();
    return {
      x: Math.max(0, Math.min(1, (event.clientX - bounds.left) / bounds.width)),
      y: Math.max(0, Math.min(1, (event.clientY - bounds.top) / bounds.height)),
    };
  };
  const handlePointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (mappingEditing || event.button !== 0 || directTouchesRef.current.has(event.pointerId)) return;
    const used = new Set([
      ...buildTouchFrame(controlProfile.mappings, heldRef.current, frameSize).filter((contact) => contact.touching).map((contact) => contact.identity),
      ...[...directTouchesRef.current.values()].map((contact) => contact.identity),
    ]);
    const identity = [0, 1, 2, 3, 4].find((candidate) => !used.has(candidate));
    if (identity === undefined) {
      void message.warning(t("mapping.allContactsUsed"));
      return;
    }
    event.preventDefault();
    event.currentTarget.focus();
    event.currentTarget.setPointerCapture(event.pointerId);
    const contact = { identity, touching: true, ...pointFromPointer(event) };
    directTouchesRef.current.set(event.pointerId, contact);
    setDirectTouches([...directTouchesRef.current.values()]);
    sendFrame();
  };
  const handlePointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    const contact = directTouchesRef.current.get(event.pointerId);
    if (!contact) return;
    event.preventDefault();
    const moved = { ...contact, ...pointFromPointer(event) };
    directTouchesRef.current.set(event.pointerId, moved);
    setDirectTouches([...directTouchesRef.current.values()]);
    sendFrame();
  };
  const handlePointerUp = (event: ReactPointerEvent<HTMLDivElement>) => {
    const contact = directTouchesRef.current.get(event.pointerId);
    if (!contact) return;
    event.preventDefault();
    directTouchesRef.current.delete(event.pointerId);
    setDirectTouches([...directTouchesRef.current.values()]);
    sendFrame(heldRef.current, [{ ...contact, touching: false, ...pointFromPointer(event) }]);
  };
  const selectedDevice = selectedUdid ?? undefined;
  const displayedMappings = page === "mappings" ? profile.mappings : controlProfile.mappings;
  const displayedFrameSize = page === "mappings" ? mappingFrameSize : frameSize;
  const aspectRatio = useMemo(() => `${displayedFrameSize.width} / ${displayedFrameSize.height}`, [displayedFrameSize]);
  const viewportSize = useMemo(
    () => containSize(stageSize.width, stageSize.height, displayedFrameSize.width, displayedFrameSize.height),
    [displayedFrameSize, stageSize],
  );
  const statusText = status.error ?? (backendStatusKeys[status.status] ? t(backendStatusKeys[status.status]) : status.status);
  const hardwareControls = (
    <div className="hardware-controls" role="toolbar" aria-label={t("hardware.toolbar")}>
      {([
        ["home", <HomeOutlined />],
        ["lock", <LockOutlined />],
        ["volume-up", <PlusOutlined />],
        ["volume-down", <MinusOutlined />],
        ["mute", <AudioMutedOutlined />],
        ["siri", <CustomerServiceOutlined />],
        ["action", <ThunderboltOutlined />],
      ] as const).map(([name, icon]) => {
        const label = t(`hardware.${name}`);
        return (
          <Tooltip key={name} title={`${label}${controlProfile.hardwareBindings[name] ? ` · ${controlProfile.hardwareBindings[name]}` : ""}`}>
            <Button aria-label={label} icon={icon} onClick={() => command({ type: "button", name })} />
          </Tooltip>
        );
      })}
    </div>
  );

  return (
    <div className={`app-shell${deviceFullscreen ? " is-device-fullscreen" : ""}`}>
      {!deviceFullscreen && <header className="topbar">
        <div className="brand"><AimOutlined /><strong>DeviceHub Mask</strong><span>{t("brand.subtitle")}</span></div>
        <Space size={8} wrap>
          <Tag color={connected && status.active_udid ? "success" : "default"}>{statusText}</Tag>
          <Select
            className="device-select"
            value={selectedDevice}
            placeholder={t("device.select")}
            options={status.devices.map((device) => ({ value: device.udid, label: `${device.name} · ${device.connection}` }))}
            onChange={(udid) => void connectDevice(udid)}
          />
          <Tooltip title={t("device.refresh")}><Button aria-label={t("device.refresh")} disabled={!backend} icon={<ReloadOutlined />} onClick={() => void request("/api/devices/refresh", { method: "PUT" })} /></Tooltip>
          <Tooltip title={t("device.reconnect")}><Button aria-label={t("device.reconnect")} disabled={!backend || !selectedUdid} icon={<SyncOutlined />} onClick={() => void reconnectDevice()} /></Tooltip>
          {page === "mappings" && <Tooltip title={t("device.saveMappings")}><Button icon={<SaveOutlined />} onClick={() => void save()} /></Tooltip>}
          <Tooltip title={t(alwaysOnTop ? "device.unpin" : "device.pin")}><Button type={alwaysOnTop ? "primary" : "default"} icon={alwaysOnTop ? <PushpinFilled /> : <PushpinOutlined />} onClick={() => void toggleAlwaysOnTop()} /></Tooltip>
          {page === "mappings" && <Tooltip title={t(inspectorVisible ? "device.hideInspector" : "device.showInspector")}><Button icon={inspectorVisible ? <MenuFoldOutlined /> : <MenuUnfoldOutlined />} onClick={() => setInspectorVisible((visible) => !visible)} /></Tooltip>}
          <Tooltip title={t("device.enterDeviceFullscreen")}><Button icon={<ExpandOutlined />} onClick={toggleDeviceFullscreen} /></Tooltip>
          <Tooltip title={t(systemFullscreen ? "device.exitSystemFullscreen" : "device.enterSystemFullscreen")}><Button icon={systemFullscreen ? <FullscreenExitOutlined /> : <FullscreenOutlined />} onClick={() => void toggleSystemFullscreen()} /></Tooltip>
        </Space>
      </header>}

      <div className="desktop-body">
        {!deviceFullscreen && <AppNavigation page={page} onChange={(next) => { releaseAllControls(); setPage(next); }} />}
        <div className="page-content">
          {page === "settings" ? (
            <SettingsPage
              alwaysOnTop={alwaysOnTop}
              systemFullscreen={systemFullscreen}
              inspectorVisible={inspectorVisible}
              onAlwaysOnTopChange={() => void toggleAlwaysOnTop()}
              onSystemFullscreenChange={() => void toggleSystemFullscreen()}
              onInspectorVisibleChange={setInspectorVisible}
            />
          ) : page === "location" ? (
            <LocationPage activeUdid={status.active_udid} status={status.location} request={request} />
          ) : (
            <>
              {page === "mappings" && (
                <ProfileManager
                  profile={profile}
                  profiles={profiles}
                  activeProfile={activeProfile}
                  frameSize={mappingFrameSize}
                  onLoad={loadProfile}
                  onSave={save}
                  onActivate={activateCurrentProfile}
                  onCreate={createProfile}
                  onDuplicate={duplicateProfile}
                  onRename={renameProfile}
                  onDelete={deleteCurrentProfile}
                  onImport={importProfile}
                />
              )}
              <main className={`workspace ${deviceFullscreen ? "inspector-hidden" : page === "device" ? "device-workspace" : page === "mappings" && inspectorVisible ? "" : "inspector-hidden"}`}>
                <section className="stage-column">
                  {deviceFullscreen ? (
                    <div className="device-fullscreen-toolbar" role="toolbar" aria-label={t("device.deviceFullscreenControls")}>
                      <Tooltip title={t("device.reconnect")}><Button aria-label={t("device.reconnect")} disabled={!backend || !selectedUdid} icon={<SyncOutlined />} onClick={() => void reconnectDevice()} /></Tooltip>
                      <Segmented<ControlMode>
                        value={controlMode}
                        options={[
                          { label: <Tooltip title={t("device.mappingMode")}><AimOutlined /></Tooltip>, value: "mapping" },
                          { label: <Tooltip title={t("device.keyboardMode")}><KeyOutlined /></Tooltip>, value: "keyboard" },
                        ]}
                        onChange={(mode) => {
                          releaseAllControls();
                          setControlMode(mode);
                          if (mode === "keyboard") setEditing(false);
                        }}
                      />
                      <Tooltip title={t(controlOverlayVisible ? "device.hideControlOverlay" : "device.showControlOverlay")}>
                        <Button
                          aria-label={t(controlOverlayVisible ? "device.hideControlOverlay" : "device.showControlOverlay")}
                          icon={controlOverlayVisible ? <EyeInvisibleOutlined /> : <EyeOutlined />}
                          onClick={() => setControlOverlayVisible((visible) => !visible)}
                        />
                      </Tooltip>
                      <Tooltip title={t("device.rotateLeft")}><Button icon={<RotateLeftOutlined />} onClick={() => command({ type: "rotate", direction: "left" })} /></Tooltip>
                      <Tooltip title={t("device.rotateRight")}><Button icon={<RotateRightOutlined />} onClick={() => command({ type: "rotate", direction: "right" })} /></Tooltip>
                      {hardwareControls}
                      <Tooltip title={t(systemFullscreen ? "device.exitSystemFullscreen" : "device.enterSystemFullscreen")}><Button icon={systemFullscreen ? <FullscreenExitOutlined /> : <FullscreenOutlined />} onClick={() => void toggleSystemFullscreen()} /></Tooltip>
                      <Tooltip title={t("device.exitDeviceFullscreen")}><Button icon={<CompressOutlined />} onClick={toggleDeviceFullscreen} /></Tooltip>
                    </div>
                  ) : <div className="stage-toolbar">
                    <div className="stream-status">
                      <Space><ApiOutlined /><Typography.Text>{t(connected ? "status.websocketConnected" : "status.reconnecting")}</Typography.Text></Space>
                      <Tooltip title={t("device.bandwidth", { value: streamMetrics.megabits_per_second.toFixed(1) })}>
                        <Typography.Text className="stream-metrics">
                          {t("device.metrics", {
                            source: streamMetrics.source_fps.toFixed(0),
                            decoded: streamMetrics.decoded_fps.toFixed(0),
                            sent: streamMetrics.sent_fps.toFixed(0),
                            render: renderFps.toFixed(0),
                            jpeg: streamMetrics.jpeg_encode_ms.toFixed(1),
                          })}
                        </Typography.Text>
                      </Tooltip>
                    </div>
                    <Space>
                      <Segmented<ControlMode>
                        value={controlMode}
                        options={[
                          { label: t("device.mappingMode"), value: "mapping", icon: <AimOutlined /> },
                          { label: t("device.keyboardMode"), value: "keyboard", icon: <KeyOutlined /> },
                        ]}
                        onChange={(mode) => {
                          releaseAllControls();
                          setControlMode(mode);
                          if (mode === "keyboard") setEditing(false);
                        }}
                      />
                      {page === "device" && (
                        <Tooltip title={t(controlOverlayVisible ? "device.hideControlOverlay" : "device.showControlOverlay")}>
                          <Button
                            aria-label={t(controlOverlayVisible ? "device.hideControlOverlay" : "device.showControlOverlay")}
                            icon={controlOverlayVisible ? <EyeInvisibleOutlined /> : <EyeOutlined />}
                            onClick={() => setControlOverlayVisible((visible) => !visible)}
                          />
                        </Tooltip>
                      )}
                      {page === "mappings" && <><span>{t("device.edit")}</span><Switch disabled={controlMode === "keyboard"} checked={mappingEditing} onChange={(value) => { releaseAllControls(); setEditing(value); }} /></>}
                      <Tooltip title={t("device.rotateLeft")}><Button icon={<RotateLeftOutlined />} onClick={() => command({ type: "rotate", direction: "left" })} /></Tooltip>
                      <Tooltip title={t("device.rotateRight")}><Button icon={<RotateRightOutlined />} onClick={() => command({ type: "rotate", direction: "right" })} /></Tooltip>
                    </Space>
                    {hardwareControls}
                  </div>}
                  {page === "mappings" && (
                    <MappingBackgroundToolbar
                      mode={mappingBackgroundMode}
                      sourceSize={mappingFrameSize}
                      viewportSize={viewportSize}
                      screenshotAvailable={capturedScreenshot !== null}
                      canCapture={hasFrame}
                      onModeChange={setMappingBackgroundMode}
                      onCapture={() => void captureMappingScreenshot(true)}
                      onSave={() => void saveMappingScreenshot()}
                    />
                  )}
                  <div className="stage-wrap" ref={stageRef}>
                    <div
                      className={`device-viewport ${mappingEditing ? "is-editing" : "is-controlling"}`}
                      style={{ aspectRatio, width: viewportSize.width, height: viewportSize.height }}
                      tabIndex={0}
                      onPointerDown={handlePointerDown}
                      onPointerMove={handlePointerMove}
                      onPointerUp={handlePointerUp}
                      onPointerCancel={handlePointerUp}
                      onContextMenu={(event) => !mappingEditing && event.preventDefault()}
                    >
                      <canvas ref={canvasRef} />
                      {page === "mappings" && mappingBackgroundMode === "screenshot" && capturedScreenshot && (
                        <img className="mapping-screenshot" src={capturedScreenshot.url} alt="" draggable={false} />
                      )}
                      {(page === "mappings" || controlOverlayVisible) && (
                        <MappingOverlay mappings={displayedMappings} selectedId={selectedId} editing={mappingEditing} activeIds={activeIds} onSelect={setSelectedId} onMove={moveMapping} />
                      )}
                      {directTouches.map((contact) => (
                        <span key={contact.identity} className="direct-touch" style={{ left: `${contact.x * 100}%`, top: `${contact.y * 100}%` }} />
                      ))}
                      {!status.active_udid && !(page === "mappings" && mappingBackgroundMode === "screenshot" && capturedScreenshot) && <div className="empty-stage"><AimOutlined /><span>{t("status.waitingForDevice")}</span></div>}
                    </div>
                  </div>
                </section>
                {page === "mappings" && inspectorVisible && (
                  <MappingInspector
                    mappings={profile.mappings}
                    selectedId={selectedId}
                    onSelect={setSelectedId}
                    onChange={updateMapping}
                    onAdd={addMapping}
                    onDelete={deleteMapping}
                    hardwareBindings={profile.hardwareBindings}
                    onHardwareBindingChange={updateHardwareBinding}
                  />
                )}
                {page === "device" && !deviceFullscreen && (
                  <DeviceInspector activeUdid={status.active_udid} request={request} />
                )}
              </main>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
