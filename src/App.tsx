import {
  AimOutlined,
  ApiOutlined,
  AudioMutedOutlined,
  CameraOutlined,
  CompressOutlined,
  CustomerServiceOutlined,
  EditOutlined,
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
  SendOutlined,
  SoundOutlined,
  StopOutlined,
  SyncOutlined,
  ThunderboltOutlined,
  VideoCameraOutlined,
} from "@ant-design/icons";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Button, Input, Popover, Segmented, Select, Space, Switch, Tag, Tooltip, Typography, message } from "antd";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { useTranslation } from "react-i18next";
import { AppNavigation, type AppPage } from "./components/AppNavigation";
import { DeviceInspector } from "./components/DeviceInspector";
import { LocationPage } from "./components/LocationPage";
import { MappingBackgroundToolbar, type MappingBackgroundMode } from "./components/MappingBackgroundToolbar";
import { MappingInspector } from "./components/MappingInspector";
import { MappingOverlay } from "./components/MappingOverlay";
import { PerformanceHud } from "./components/PerformanceHud";
import { PerformancePage } from "./components/PerformancePage";
import { ProfileManager } from "./components/ProfileManager";
import { SettingsPage } from "./components/SettingsPage";
import { PcmAudioPlayer, parseAudioEnvelope, readDeviceAudioPreferences, saveDeviceAudioPreferences, type DeviceAudioPreferences } from "./deviceAudio";
import { buildTouchFrame, isBoundKey, keyboardUsage, mappingBindings, mergeTouchContacts, remainingTapDuration, touchFramesEqual, type TouchContact } from "./control";
import { deviceViewScaleFactor, readDeviceViewPreferences, saveDeviceViewPreferences, type DeviceViewPreferences, type DeviceViewScale } from "./deviceViewPreferences";
import { logFrontend } from "./diagnostics";
import { devicePerformanceHudItems, readPerformanceHudPreferences, savePerformanceHudPreferences, type PerformanceHudPreferences } from "./performanceHudPreferences";
import { hasDecodedVideoActivity, isVideoStreamStalled } from "./streamHealth";
import { createMapping, defaultHardwareBindings, defaultProfile, hardwareButtons, type DeviceStatus, type HardwareButtonName, type Mapping, type Orientation, type PerformanceView, type Profile, type ScrcpyMappingType, type StreamMetrics } from "./types";

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
type ProfileList = { profiles: string[]; active: string; app_bindings: Record<string, string>; binding_conflicts: string[] };
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

function recordingFilename(deviceName: string, extension: string) {
  const safeName = deviceName.trim().replace(/[<>:"/\\|?*]+/g, "-") || "iPhone";
  const timestamp = new Date().toISOString().replace(/[:.]/g, "-");
  return `devicehub-mask_${safeName}_${timestamp}.${extension}`;
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
  const [profileSwitching, setProfileSwitching] = useState<string | null>(null);
  const [appProfileBindings, setAppProfileBindings] = useState<Record<string, string>>({});
  const [appBindingConflicts, setAppBindingConflicts] = useState<string[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>("move");
  const [editing, setEditing] = useState(true);
  const [controlMode, setControlMode] = useState<ControlMode>("mapping");
  const [alwaysOnTop, setAlwaysOnTop] = useState(false);
  const [systemFullscreen, setSystemFullscreen] = useState(false);
  const [deviceFullscreen, setDeviceFullscreen] = useState(false);
  const [deviceViewPreferences, setDeviceViewPreferences] = useState<DeviceViewPreferences>(readDeviceViewPreferences);
  const [fullscreenToolbarVisible, setFullscreenToolbarVisible] = useState(true);
  const [selectedUdid, setSelectedUdid] = useState<string | null>(null);
  const [inspectorVisible, setInspectorVisible] = useState(true);
  const [connected, setConnected] = useState(false);
  const [streamMetrics, setStreamMetrics] = useState<StreamMetrics>(emptyMetrics);
  const [renderFps, setRenderFps] = useState(0);
  const [performanceView, setPerformanceView] = useState<PerformanceView | null>(null);
  const [performanceError, setPerformanceError] = useState<string | null>(null);
  const [performanceHud, setPerformanceHud] = useState<PerformanceHudPreferences>(readPerformanceHudPreferences);
  const [audioPlayback, setAudioPlayback] = useState<DeviceAudioPreferences>(readDeviceAudioPreferences);
  const [activeIds, setActiveIds] = useState<Set<number>>(new Set());
  const [directTouches, setDirectTouches] = useState<TouchContact[]>([]);
  const [frameSize, setFrameSize] = useState({ width: 1296, height: 2816 });
  const [hasFrame, setHasFrame] = useState(false);
  const [canvasReady, setCanvasReady] = useState(false);
  const [streamStalled, setStreamStalled] = useState(false);
  const [recording, setRecording] = useState(false);
  const [textInputOpen, setTextInputOpen] = useState(false);
  const [textInput, setTextInput] = useState("");
  const [displayScaleOpen, setDisplayScaleOpen] = useState(false);
  const [mappingBackgroundMode, setMappingBackgroundMode] = useState<MappingBackgroundMode>("live");
  const [capturedScreenshot, setCapturedScreenshot] = useState<CapturedScreenshot | null>(null);
  const [stageSize, setStageSize] = useState({ width: 0, height: 0 });
  const stageRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const canvasContextRef = useRef<CanvasRenderingContext2D | null>(null);
  const renderedFramesRef = useRef(0);
  const lastVideoActivityAtRef = useRef(0);
  const socketRef = useRef<WebSocket | null>(null);
  const audioPlayerRef = useRef<PcmAudioPlayer | null>(null);
  const recorderRef = useRef<MediaRecorder | null>(null);
  const recordingStreamRef = useRef<MediaStream | null>(null);
  const recordingChunksRef = useRef<Blob[]>([]);
  const canvasReadyRef = useRef(false);
  const fullscreenToolbarTimerRef = useRef<number | null>(null);
  const profileSwitchingRef = useRef(false);
  const orientationRef = useRef<Orientation>("portrait");
  const heldRef = useRef(new Set<string>());
  const heldSinceRef = useRef(new Map<string, number>());
  const mappingOffsetsRef = useRef(new Map<string, { x: number; y: number }>());
  const heldHardwareRef = useRef(new Map<string, HardwareButtonName>());
  const forwardedKeyboardRef = useRef(new Map<string, number>());
  const directTouchesRef = useRef(new Map<number, TouchContact>());
  const directTouchStartedAtRef = useRef(new Map<number, number>());
  const directTouchReleaseTimersRef = useRef(new Map<number, number>());
  const activeIdsRef = useRef(new Set<number>());
  const lastSentTouchFrameRef = useRef<TouchContact[] | null>(null);
  const capturedScreenshotRef = useRef<CapturedScreenshot | null>(null);
  const hasFrameRef = useRef(false);

  orientationRef.current = status.orientation;
  useEffect(() => {
    if (status.active_udid) setSelectedUdid(status.active_udid);
  }, [status.active_udid]);

  const updateAudioPlayback = useCallback((next: DeviceAudioPreferences) => {
    setAudioPlayback(next);
    saveDeviceAudioPreferences(next);
    audioPlayerRef.current?.setPreferences(next);
    if (!next.muted) void audioPlayerRef.current?.resume();
  }, []);

  useEffect(() => {
    const player = new PcmAudioPlayer(audioPlayback);
    audioPlayerRef.current = player;
    return () => {
      player.close();
      if (audioPlayerRef.current === player) audioPlayerRef.current = null;
    };
    // Playback preference changes are applied through updateAudioPlayback.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (audioPlayback.muted) return;
    const unlockAudio = () => void audioPlayerRef.current?.resume();
    window.addEventListener("pointerdown", unlockAudio, { once: true });
    window.addEventListener("keydown", unlockAudio, { once: true });
    return () => {
      window.removeEventListener("pointerdown", unlockAudio);
      window.removeEventListener("keydown", unlockAudio);
    };
  }, [audioPlayback.muted]);

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

  const bindCanvas = useCallback((canvas: HTMLCanvasElement | null) => {
    canvasRef.current = canvas;
    canvasContextRef.current = null;
    if (canvas) {
      canvasReadyRef.current = false;
      setCanvasReady(false);
    }
  }, []);

  const updateDeviceViewPreferences = useCallback((next: DeviceViewPreferences) => {
    setDeviceViewPreferences(next);
    saveDeviceViewPreferences(next);
  }, []);

  const patchDeviceViewPreferences = useCallback((patch: Partial<DeviceViewPreferences>) => {
    setDeviceViewPreferences((current) => {
      const next = { ...current, ...patch };
      saveDeviceViewPreferences(next);
      return next;
    });
  }, []);

  const updatePerformanceHud = useCallback((preferences: PerformanceHudPreferences) => {
    setPerformanceHud(preferences);
    savePerformanceHudPreferences(preferences);
  }, []);

  const hudNeedsDeviceSampling = performanceHud.enabled
    && performanceHud.items.some((item) => devicePerformanceHudItems.has(item));
  const performanceSamplingRequired = Boolean(status.active_udid)
    && (page === "performance" || (page === "device" && hudNeedsDeviceSampling));

  useEffect(() => {
    if (!backend) return;
    const method = performanceSamplingRequired ? "PUT" : "DELETE";
    void request("/api/performance/sampling", { method }).then((response) => {
      if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
    }).catch((error) => {
      logFrontend("warn", "performance", "set_sampling", error);
      if (performanceSamplingRequired) setPerformanceError(String(error));
    });
  }, [backend, performanceSamplingRequired, request]);

  useEffect(() => {
    if (!performanceSamplingRequired) {
      setPerformanceView(null);
      setPerformanceError(null);
      return;
    }
    setPerformanceView(null);
    setPerformanceError(null);
    let disposed = false;
    let loading = false;
    let failureLogged = false;
    const refresh = async () => {
      if (loading) return;
      loading = true;
      try {
        const response = await request("/api/performance");
        if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
        const next = await response.json() as PerformanceView;
        if (!disposed) {
          setPerformanceView(next);
          setPerformanceError(null);
          failureLogged = false;
        }
      } catch (error) {
        if (!disposed) {
          setPerformanceError(String(error));
          if (!failureLogged) {
            failureLogged = true;
            logFrontend("warn", "performance", "read_telemetry", error);
          }
        }
      } finally {
        loading = false;
      }
    };
    void refresh();
    const timer = window.setInterval(() => void refresh(), 1_000);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [performanceSamplingRequired, request, status.active_udid]);

  const command = useCallback((payload: unknown) => {
    if (socketRef.current?.readyState === WebSocket.OPEN) {
      socketRef.current.send(JSON.stringify(payload));
    }
  }, []);

  const sendFrame = useCallback((nextHeld = heldRef.current, released: TouchContact[] = []) => {
    const mappedContacts = buildTouchFrame(
      controlProfile.mappings,
      nextHeld,
      frameSize,
      performance.now(),
      heldSinceRef.current,
      mappingOffsetsRef.current,
    );
    const contacts = mergeTouchContacts(
      mappedContacts,
      [...directTouchesRef.current.values()],
      released,
    );
    const nextActiveIds = new Set(mappedContacts.filter((contact) => contact.touching).map((contact) => contact.identity));
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
    for (const timer of directTouchReleaseTimersRef.current.values()) window.clearTimeout(timer);
    directTouchReleaseTimersRef.current.clear();
    directTouchStartedAtRef.current.clear();
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

  useEffect(() => {
    if (!connected || !status.active_udid || (page !== "device" && page !== "mappings")) {
      setStreamStalled(false);
      return;
    }
    const update = () => {
      setStreamStalled(isVideoStreamStalled(performance.now(), lastVideoActivityAtRef.current));
    };
    update();
    const timer = window.setInterval(update, 1_000);
    return () => window.clearInterval(timer);
  }, [connected, page, status.active_udid]);

  const showFullscreenToolbar = useCallback(() => {
    if (!deviceFullscreen || !deviceViewPreferences.fullscreenToolbarAutoHide) return;
    setFullscreenToolbarVisible(true);
    if (fullscreenToolbarTimerRef.current !== null) window.clearTimeout(fullscreenToolbarTimerRef.current);
    fullscreenToolbarTimerRef.current = window.setTimeout(() => {
      fullscreenToolbarTimerRef.current = null;
      if (textInputOpen || displayScaleOpen) return;
      setFullscreenToolbarVisible(false);
    }, 2_200);
  }, [deviceFullscreen, deviceViewPreferences.fullscreenToolbarAutoHide, displayScaleOpen, textInputOpen]);

  useEffect(() => {
    if (!deviceFullscreen || !deviceViewPreferences.fullscreenToolbarAutoHide) {
      setFullscreenToolbarVisible(true);
      if (fullscreenToolbarTimerRef.current !== null) window.clearTimeout(fullscreenToolbarTimerRef.current);
      fullscreenToolbarTimerRef.current = null;
      return;
    }
    showFullscreenToolbar();
    return () => {
      if (fullscreenToolbarTimerRef.current !== null) window.clearTimeout(fullscreenToolbarTimerRef.current);
      fullscreenToolbarTimerRef.current = null;
    };
  }, [deviceFullscreen, deviceViewPreferences.fullscreenToolbarAutoHide, showFullscreenToolbar]);

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
      socket.binaryType = "arraybuffer";
      socket.onopen = () => {
        logFrontend("info", "websocket", "opened", "Video and control socket connected");
        socketRef.current = socket;
        lastVideoActivityAtRef.current = performance.now();
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
        lastVideoActivityAtRef.current = 0;
        canvasReadyRef.current = false;
        setCanvasReady(false);
        setStreamStalled(false);
        setStreamMetrics(emptyMetrics);
        audioPlayerRef.current?.reset();
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
              const cachedContext = canvasContextRef.current;
              const context = cachedContext?.canvas === canvas
                ? cachedContext
                : canvas.getContext("2d", { alpha: false });
              if (!context) continue;
              canvasContextRef.current = context;
              const drawStarted = performance.now();
              const size = drawFrame(canvas, context, bitmap, orientationRef.current);
              frontendMetrics.canvasDrawMs += performance.now() - drawStarted;
              frontendMetrics.presentedFrames += 1;
              renderedFramesRef.current += 1;
              lastVideoActivityAtRef.current = performance.now();
              setStreamStalled(false);
              if (!canvasReadyRef.current) {
                canvasReadyRef.current = true;
                setCanvasReady(true);
              }
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
          if (data.type === "metrics") {
            const metrics = data.payload as StreamMetrics;
            setStreamMetrics(metrics);
            if (hasDecodedVideoActivity(metrics)) {
              lastVideoActivityAtRef.current = performance.now();
              setStreamStalled(false);
            }
          }
          return;
        }
        const buffer = event.data as ArrayBuffer;
        try {
          const audio = parseAudioEnvelope(buffer);
          if (audio) {
            audioPlayerRef.current?.push(audio);
            return;
          }
        } catch (error) {
          logFrontend("warn", "audio", "decode_chunk", error);
          return;
        }
        frontendMetrics.receivedFrames += 1;
        lastVideoActivityAtRef.current = performance.now();
        setStreamStalled(false);
        if (pendingFrame) {
          frontendMetrics.replacedFrames += 1;
          if (socket.readyState === WebSocket.OPEN) {
            socket.send(JSON.stringify({ type: "frame_presented" }));
          }
        }
        pendingFrame = new Blob([buffer], { type: "image/jpeg" });
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

  const saveDeviceScreenshot = useCallback(async () => {
    const canvas = canvasRef.current;
    if (!canvas || !canvasReadyRef.current) {
      void message.warning(t("device.screenshotUnavailable"));
      return;
    }
    const blob = await canvasPng(canvas);
    if (!blob) {
      void message.error(t("device.screenshotFailed"));
      return;
    }
    const deviceName = status.devices.find((device) => device.udid === status.active_udid)?.name ?? "iPhone";
    const url = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = url;
    link.download = screenshotFilename(deviceName, canvas.width, canvas.height);
    document.body.appendChild(link);
    link.click();
    link.remove();
    window.setTimeout(() => URL.revokeObjectURL(url), 1_000);
    void message.success(t("device.screenshotSaved"));
  }, [status.active_udid, status.devices, t]);

  const stopDeviceRecording = useCallback(() => {
    const recorder = recorderRef.current;
    if (recorder && recorder.state !== "inactive") recorder.stop();
  }, []);

  const toggleDeviceRecording = useCallback(() => {
    const activeRecorder = recorderRef.current;
    if (activeRecorder && activeRecorder.state !== "inactive") {
      activeRecorder.stop();
      return;
    }
    const canvas = canvasRef.current;
    if (!canvas || !canvasReadyRef.current || typeof MediaRecorder === "undefined" || typeof canvas.captureStream !== "function") {
      void message.warning(t("device.recordingUnavailable"));
      return;
    }
    try {
      const stream = canvas.captureStream(60);
      const mimeType = [
        "video/mp4;codecs=avc1.42E01E",
        "video/mp4",
        "video/webm;codecs=vp9",
        "video/webm;codecs=vp8",
        "video/webm",
      ].find((candidate) => MediaRecorder.isTypeSupported(candidate));
      const recorder = mimeType ? new MediaRecorder(stream, { mimeType }) : new MediaRecorder(stream);
      recordingChunksRef.current = [];
      recordingStreamRef.current = stream;
      recorderRef.current = recorder;
      recorder.ondataavailable = (event) => {
        if (event.data.size > 0) recordingChunksRef.current.push(event.data);
      };
      recorder.onerror = (event) => {
        logFrontend("warn", "video", "recording", event.error);
        void message.error(t("device.recordingFailed", { error: event.error.message }));
      };
      recorder.onstop = () => {
        const chunks = recordingChunksRef.current;
        const recordedType = recorder.mimeType || mimeType || "video/webm";
        recordingStreamRef.current?.getTracks().forEach((track) => track.stop());
        recordingStreamRef.current = null;
        recorderRef.current = null;
        recordingChunksRef.current = [];
        setRecording(false);
        if (chunks.length === 0) return;
        const blob = new Blob(chunks, { type: recordedType });
        const url = URL.createObjectURL(blob);
        const link = document.createElement("a");
        const deviceName = status.devices.find((device) => device.udid === status.active_udid)?.name ?? "iPhone";
        link.href = url;
        link.download = recordingFilename(deviceName, recordedType.includes("mp4") ? "mp4" : "webm");
        document.body.appendChild(link);
        link.click();
        link.remove();
        window.setTimeout(() => URL.revokeObjectURL(url), 1_000);
        void message.success(t("device.recordingSaved"));
      };
      recorder.start(1_000);
      setRecording(true);
    } catch (error) {
      recordingStreamRef.current?.getTracks().forEach((track) => track.stop());
      recordingStreamRef.current = null;
      recorderRef.current = null;
      setRecording(false);
      logFrontend("warn", "video", "start_recording", error);
      void message.error(t("device.recordingFailed", { error: String(error) }));
    }
  }, [status.active_udid, status.devices, t]);

  useEffect(() => {
    if (page !== "device" || !status.active_udid) stopDeviceRecording();
  }, [page, status.active_udid, stopDeviceRecording]);

  useEffect(() => () => {
    const recorder = recorderRef.current;
    if (recorder && recorder.state !== "inactive") recorder.stop();
    recordingStreamRef.current?.getTracks().forEach((track) => track.stop());
  }, []);

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
      bundleIdentifiers: Array.isArray(loaded.bundleIdentifiers) ? loaded.bundleIdentifiers : [],
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
    setAppProfileBindings(list.app_bindings ?? {});
    setAppBindingConflicts(list.binding_conflicts ?? []);
    return list;
  }, [request]);

  const activateSavedControlProfile = useCallback(async (target: string) => {
    if (target === activeProfile) return false;
    if (profileSwitchingRef.current) throw new Error(translateRef.current("profile.switchInProgress"));
    profileSwitchingRef.current = true;
    setProfileSwitching(target);
    try {
      const loaded = await readProfile(target);
      releaseAllControls();
      const response = await request(`/api/profiles/${encodeURIComponent(target)}/activate`, { method: "PUT" });
      if (!response.ok) throw new Error(translateRef.current("errors.activateProfile", { status: response.status }));
      setActiveProfile(target);
      setControlProfile(loaded);
      return true;
    } finally {
      profileSwitchingRef.current = false;
      setProfileSwitching(null);
    }
  }, [activeProfile, readProfile, releaseAllControls, request]);

  const activateProfileForApp = useCallback(async (bundleId: string) => {
    const target = appProfileBindings[bundleId];
    if (!target) return;
    try {
      if (await activateSavedControlProfile(target)) {
        void message.success(translateRef.current("profile.autoActivated", { profile: target }));
      }
    } catch (error) {
      void message.warning(translateRef.current("profile.autoActivateFailed", { error: String(error) }));
    }
  }, [activateSavedControlProfile, appProfileBindings]);

  const switchControlProfile = useCallback(async (target: string) => {
    try {
      if (await activateSavedControlProfile(target)) {
        void message.success(translateRef.current("profile.switched", { profile: target }));
      }
    } catch (error) {
      void message.error(translateRef.current("profile.switchFailed", { error: String(error) }));
    }
  }, [activateSavedControlProfile]);

  const writeProfile = useCallback(async (name: string, value: Profile) => {
    const response = await request(`/api/profiles/${encodeURIComponent(name)}`, {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ ...value, name }),
    });
    if (!response.ok) throw new Error(translateRef.current("errors.saveProfile", { status: response.status }));
  }, [request]);

  const changeAppProfileBinding = useCallback(async (bundleId: string, bind: boolean) => {
    if (appBindingConflicts.includes(bundleId)) {
      throw new Error(translateRef.current("profile.appBindingConflict"));
    }
    const owner = appProfileBindings[bundleId];
    const profileName = bind ? activeProfile : owner;
    if (!profileName || (bind && owner && owner !== activeProfile)) {
      throw new Error(translateRef.current("profile.appBindingOwned", { profile: owner ?? "" }));
    }
    const loaded = await readProfile(profileName);
    const bundleIdentifiers = bind
      ? [...new Set([...loaded.bundleIdentifiers, bundleId])]
      : loaded.bundleIdentifiers.filter((candidate) => candidate !== bundleId);
    const updated = { ...loaded, bundleIdentifiers };
    await writeProfile(profileName, updated);
    await refreshProfiles();
    const mergeBinding = (current: Profile) => current.name === profileName
      ? {
          ...current,
          bundleIdentifiers: bind
            ? [...new Set([...current.bundleIdentifiers, bundleId])]
            : current.bundleIdentifiers.filter((candidate) => candidate !== bundleId),
        }
      : current;
    setProfile(mergeBinding);
    setControlProfile(mergeBinding);
  }, [activeProfile, appBindingConflicts, appProfileBindings, readProfile, refreshProfiles, writeProfile]);

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
    await writeProfile(name, { ...profile, name, bundleIdentifiers: [] });
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
    setFullscreenToolbarVisible(true);
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
    directTouchStartedAtRef.current.set(event.pointerId, performance.now());
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
    if (!contact || directTouchReleaseTimersRef.current.has(event.pointerId)) return;
    event.preventDefault();
    const pointerId = event.pointerId;
    const finalContact = { ...contact, ...pointFromPointer(event) };
    directTouchesRef.current.set(pointerId, finalContact);
    const finish = () => {
      const current = directTouchesRef.current.get(pointerId);
      if (!current || current.identity !== finalContact.identity) return;
      directTouchReleaseTimersRef.current.delete(pointerId);
      directTouchStartedAtRef.current.delete(pointerId);
      directTouchesRef.current.delete(pointerId);
      setDirectTouches([...directTouchesRef.current.values()]);
      sendFrame(heldRef.current, [{ ...finalContact, touching: false }]);
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
  };
  const controlOverlayVisible = deviceViewPreferences.controlOverlayVisible;
  const selectedDevice = selectedUdid ?? undefined;
  const displayedMappings = page === "mappings" ? profile.mappings : controlProfile.mappings;
  const displayedFrameSize = page === "mappings" ? mappingFrameSize : frameSize;
  const aspectRatio = useMemo(() => `${displayedFrameSize.width} / ${displayedFrameSize.height}`, [displayedFrameSize]);
  const activeViewScale = page === "device" ? deviceViewPreferences.scale : "fit";
  const viewScaleFactor = deviceViewScaleFactor(activeViewScale);
  const viewportSize = useMemo(() => viewScaleFactor === null
    ? containSize(stageSize.width, stageSize.height, displayedFrameSize.width, displayedFrameSize.height)
    : { width: displayedFrameSize.width * viewScaleFactor, height: displayedFrameSize.height * viewScaleFactor },
  [displayedFrameSize, stageSize, viewScaleFactor]);
  const viewportScrollable = activeViewScale !== "fit";
  const stageIssue = !status.active_udid
    ? "waiting"
    : !connected
      ? "reconnecting"
      : !canvasReady
        ? "starting"
        : streamStalled
          ? "stalled"
          : null;
  const statusText = status.error ?? (backendStatusKeys[status.status] ? t(backendStatusKeys[status.status]) : status.status);
  const controlProfileSelector = (
    <Tooltip title={t("device.controlProfile")}>
      <Select
        className="control-profile-select"
        aria-label={t("device.controlProfile")}
        value={activeProfile}
        options={profiles.map((name) => ({ value: name, label: name }))}
        loading={profileSwitching !== null}
        disabled={profiles.length === 0 || profileSwitching !== null}
        onChange={(name) => void switchControlProfile(name)}
      />
    </Tooltip>
  );
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
  const recordingSupported = typeof MediaRecorder !== "undefined"
    && typeof HTMLCanvasElement !== "undefined"
    && typeof HTMLCanvasElement.prototype.captureStream === "function";
  const deviceDisplayControls = (
    <Space size={4} className="device-display-controls">
      <Select<DeviceViewScale>
        className="device-scale-select"
        aria-label={t("device.displayScale")}
        value={deviceViewPreferences.scale}
        onOpenChange={setDisplayScaleOpen}
        options={[
          { value: "fit", label: t("device.fitWindow") },
          { value: "0.25", label: "25%" },
          { value: "0.5", label: "50%" },
          { value: "0.75", label: "75%" },
          { value: "1", label: t("device.actualSize") },
          { value: "1.25", label: "125%" },
          { value: "1.5", label: "150%" },
          { value: "2", label: "200%" },
        ]}
        onChange={(scale) => patchDeviceViewPreferences({ scale })}
      />
      <Tooltip title={t("device.saveScreenshot")}>
        <Button aria-label={t("device.saveScreenshot")} disabled={!canvasReady} icon={<CameraOutlined />} onClick={() => void saveDeviceScreenshot()} />
      </Tooltip>
      <Tooltip title={t(audioPlayback.muted ? "device.unmuteDeviceAudio" : "device.muteDeviceAudio")}>
        <Button
          aria-label={t(audioPlayback.muted ? "device.unmuteDeviceAudio" : "device.muteDeviceAudio")}
          type={audioPlayback.muted ? "default" : "primary"}
          icon={audioPlayback.muted ? <AudioMutedOutlined /> : <SoundOutlined />}
          onClick={() => updateAudioPlayback({ ...audioPlayback, muted: !audioPlayback.muted })}
        />
      </Tooltip>
      <Tooltip title={t(recording ? "device.stopRecording" : recordingSupported ? "device.startRecording" : "device.recordingUnsupported")}>
        <Button
          aria-label={t(recording ? "device.stopRecording" : "device.startRecording")}
          danger={recording}
          type={recording ? "primary" : "default"}
          disabled={!recording && (!canvasReady || !recordingSupported)}
          icon={recording ? <StopOutlined /> : <VideoCameraOutlined />}
          onClick={toggleDeviceRecording}
        />
      </Tooltip>
      <Tooltip title={t(deviceViewPreferences.rotationControlsLocked ? "device.unlockRotationControls" : "device.lockRotationControls")}>
        <Button
          aria-label={t(deviceViewPreferences.rotationControlsLocked ? "device.unlockRotationControls" : "device.lockRotationControls")}
          type={deviceViewPreferences.rotationControlsLocked ? "primary" : "default"}
          icon={<LockOutlined />}
          onClick={() => patchDeviceViewPreferences({ rotationControlsLocked: !deviceViewPreferences.rotationControlsLocked })}
        />
      </Tooltip>
      <Popover
        trigger="click"
        open={textInputOpen}
        onOpenChange={setTextInputOpen}
        title={t("device.textInput")}
        content={(
          <div className="device-text-input">
            <Input.TextArea
              autoFocus
              value={textInput}
              maxLength={128}
              rows={3}
              placeholder={t("device.textInputPlaceholder")}
              onChange={(event) => setTextInput(event.target.value)}
            />
            <Typography.Text type="secondary">{t("device.textInputHint")}</Typography.Text>
            <Button
              type="primary"
              icon={<SendOutlined />}
              disabled={!textInput || !connected || !status.active_udid}
              onClick={() => {
                command({ type: "text", text: textInput });
                setTextInput("");
                setTextInputOpen(false);
              }}
            >
              {t("device.sendText")}
            </Button>
          </div>
        )}
      >
        <Tooltip title={t("device.textInput")}><Button aria-label={t("device.textInput")} disabled={!connected || !status.active_udid} icon={<EditOutlined />} /></Tooltip>
      </Popover>
    </Space>
  );

  return (
    <div
      className={`app-shell${deviceFullscreen ? " is-device-fullscreen" : ""}`}
      onPointerMove={deviceFullscreen ? (event) => {
        if (event.target instanceof Element && event.target.closest(".device-fullscreen-toolbar")) {
          setFullscreenToolbarVisible(true);
          if (fullscreenToolbarTimerRef.current !== null) window.clearTimeout(fullscreenToolbarTimerRef.current);
          fullscreenToolbarTimerRef.current = null;
        } else {
          showFullscreenToolbar();
        }
      } : undefined}
    >
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
              deviceView={deviceViewPreferences}
              performanceHud={performanceHud}
              audioPlayback={audioPlayback}
              onAlwaysOnTopChange={() => void toggleAlwaysOnTop()}
              onSystemFullscreenChange={() => void toggleSystemFullscreen()}
              onInspectorVisibleChange={setInspectorVisible}
              onDeviceViewChange={updateDeviceViewPreferences}
              onPerformanceHudChange={updatePerformanceHud}
              onAudioPlaybackChange={updateAudioPlayback}
            />
          ) : page === "location" ? (
            <LocationPage activeUdid={status.active_udid} status={status.location} request={request} />
          ) : page === "performance" ? (
            <PerformancePage activeUdid={status.active_udid} streamMetrics={streamMetrics} renderFps={renderFps} view={performanceView} error={performanceError} />
          ) : (
            <>
              {page === "mappings" && (
                <ProfileManager
                  profile={profile}
                  profiles={profiles}
                  activeProfile={activeProfile}
                  bindingConflicts={appBindingConflicts}
                  frameSize={mappingFrameSize}
                  onLoad={loadProfile}
                  onSave={save}
                  onActivate={activateCurrentProfile}
                  onCreate={createProfile}
                  onDuplicate={duplicateProfile}
                  onRename={renameProfile}
                  onDelete={deleteCurrentProfile}
                  onBundleIdentifiersChange={(bundleIdentifiers) => setProfile((current) => ({ ...current, bundleIdentifiers }))}
                  onImport={importProfile}
                />
              )}
              <main className={`workspace ${deviceFullscreen ? "inspector-hidden" : page === "device" ? "device-workspace" : page === "mappings" && inspectorVisible ? "" : "inspector-hidden"}`}>
                <section className="stage-column">
                  {deviceFullscreen ? (
                    <div className={`device-fullscreen-toolbar${fullscreenToolbarVisible ? "" : " is-hidden"}`} role="toolbar" aria-label={t("device.deviceFullscreenControls")}>
                      <Tooltip title={t("device.reconnect")}><Button aria-label={t("device.reconnect")} disabled={!backend || !selectedUdid} icon={<SyncOutlined />} onClick={() => void reconnectDevice()} /></Tooltip>
                      {controlProfileSelector}
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
                          onClick={() => patchDeviceViewPreferences({ controlOverlayVisible: !controlOverlayVisible })}
                        />
                      </Tooltip>
                      {deviceDisplayControls}
                      <Tooltip title={t("device.rotateLeft")}><Button disabled={deviceViewPreferences.rotationControlsLocked} icon={<RotateLeftOutlined />} onClick={() => command({ type: "rotate", direction: "left" })} /></Tooltip>
                      <Tooltip title={t("device.rotateRight")}><Button disabled={deviceViewPreferences.rotationControlsLocked} icon={<RotateRightOutlined />} onClick={() => command({ type: "rotate", direction: "right" })} /></Tooltip>
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
                      {page === "device" && controlProfileSelector}
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
                            onClick={() => patchDeviceViewPreferences({ controlOverlayVisible: !controlOverlayVisible })}
                          />
                        </Tooltip>
                      )}
                      {page === "device" && deviceDisplayControls}
                      {page === "mappings" && <><span>{t("device.edit")}</span><Switch disabled={controlMode === "keyboard"} checked={mappingEditing} onChange={(value) => { releaseAllControls(); setEditing(value); }} /></>}
                      <Tooltip title={t("device.rotateLeft")}><Button disabled={deviceViewPreferences.rotationControlsLocked} icon={<RotateLeftOutlined />} onClick={() => command({ type: "rotate", direction: "left" })} /></Tooltip>
                      <Tooltip title={t("device.rotateRight")}><Button disabled={deviceViewPreferences.rotationControlsLocked} icon={<RotateRightOutlined />} onClick={() => command({ type: "rotate", direction: "right" })} /></Tooltip>
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
                  <div className={`stage-wrap${viewportScrollable ? " is-scrollable" : ""}`} ref={stageRef}>
                    <div
                      className={`device-viewport ${mappingEditing ? "is-editing" : "is-controlling"}`}
                      style={{ aspectRatio, width: viewportSize.width, height: viewportSize.height }}
                      tabIndex={0}
                      onPointerDown={handlePointerDown}
                      onPointerMove={handlePointerMove}
                      onPointerUp={handlePointerUp}
                      onPointerCancel={handlePointerUp}
                      onLostPointerCapture={handlePointerUp}
                      onContextMenu={(event) => !mappingEditing && event.preventDefault()}
                    >
                      <canvas ref={bindCanvas} />
                      {page === "device" && performanceHud.enabled && (
                        <PerformanceHud items={performanceHud.items} view={performanceView} streamMetrics={streamMetrics} renderFps={renderFps} />
                      )}
                      {page === "mappings" && mappingBackgroundMode === "screenshot" && capturedScreenshot && (
                        <img className="mapping-screenshot" src={capturedScreenshot.url} alt="" draggable={false} />
                      )}
                      {(page === "mappings" || controlOverlayVisible) && (
                        <MappingOverlay mappings={displayedMappings} selectedId={selectedId} editing={mappingEditing} activeIds={activeIds} onSelect={setSelectedId} onMove={moveMapping} />
                      )}
                      {directTouches.map((contact) => (
                        <span key={contact.identity} className="direct-touch" style={{ left: `${contact.x * 100}%`, top: `${contact.y * 100}%` }} />
                      ))}
                      {stageIssue && !(page === "mappings" && mappingBackgroundMode === "screenshot" && capturedScreenshot) && (
                        <div className="device-stage-state" onPointerDown={(event) => event.stopPropagation()}>
                          <AimOutlined />
                          <span>{t(`device.stageState.${stageIssue}`)}</span>
                          {stageIssue !== "waiting" && selectedUdid && (
                            <Button size="small" icon={<SyncOutlined />} onClick={() => void reconnectDevice()}>{t("device.reconnect")}</Button>
                          )}
                        </div>
                      )}
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
                  <DeviceInspector
                    activeUdid={status.active_udid}
                    request={request}
                    activeProfile={activeProfile}
                    appProfileBindings={appProfileBindings}
                    bindingConflicts={appBindingConflicts}
                    onAppLaunched={(bundleId) => void activateProfileForApp(bundleId)}
                    onAppProfileBindingChange={changeAppProfileBinding}
                  />
                )}
              </main>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
