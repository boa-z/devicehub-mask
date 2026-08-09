import AimOutlined from "@ant-design/icons/es/icons/AimOutlined";
import ApiOutlined from "@ant-design/icons/es/icons/ApiOutlined";
import AppstoreOutlined from "@ant-design/icons/es/icons/AppstoreOutlined";
import AudioMutedOutlined from "@ant-design/icons/es/icons/AudioMutedOutlined";
import BugOutlined from "@ant-design/icons/es/icons/BugOutlined";
import CameraOutlined from "@ant-design/icons/es/icons/CameraOutlined";
import CustomerServiceOutlined from "@ant-design/icons/es/icons/CustomerServiceOutlined";
import EditOutlined from "@ant-design/icons/es/icons/EditOutlined";
import ExpandOutlined from "@ant-design/icons/es/icons/ExpandOutlined";
import EyeInvisibleOutlined from "@ant-design/icons/es/icons/EyeInvisibleOutlined";
import EyeOutlined from "@ant-design/icons/es/icons/EyeOutlined";
import FullscreenExitOutlined from "@ant-design/icons/es/icons/FullscreenExitOutlined";
import FullscreenOutlined from "@ant-design/icons/es/icons/FullscreenOutlined";
import HomeOutlined from "@ant-design/icons/es/icons/HomeOutlined";
import LockOutlined from "@ant-design/icons/es/icons/LockOutlined";
import MenuFoldOutlined from "@ant-design/icons/es/icons/MenuFoldOutlined";
import MenuUnfoldOutlined from "@ant-design/icons/es/icons/MenuUnfoldOutlined";
import MinusOutlined from "@ant-design/icons/es/icons/MinusOutlined";
import PushpinFilled from "@ant-design/icons/es/icons/PushpinFilled";
import PushpinOutlined from "@ant-design/icons/es/icons/PushpinOutlined";
import PlusOutlined from "@ant-design/icons/es/icons/PlusOutlined";
import RotateLeftOutlined from "@ant-design/icons/es/icons/RotateLeftOutlined";
import RotateRightOutlined from "@ant-design/icons/es/icons/RotateRightOutlined";
import SaveOutlined from "@ant-design/icons/es/icons/SaveOutlined";
import SendOutlined from "@ant-design/icons/es/icons/SendOutlined";
import SoundOutlined from "@ant-design/icons/es/icons/SoundOutlined";
import StopOutlined from "@ant-design/icons/es/icons/StopOutlined";
import SyncOutlined from "@ant-design/icons/es/icons/SyncOutlined";
import ThunderboltOutlined from "@ant-design/icons/es/icons/ThunderboltOutlined";
import VideoCameraOutlined from "@ant-design/icons/es/icons/VideoCameraOutlined";
import { Button, Dropdown, Input, Popover, Segmented, Select, Space, Switch, Tag, Tooltip, Typography, message } from "antd";
import { lazy, Suspense, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type MouseEvent as ReactMouseEvent } from "react";
import { useTranslation } from "react-i18next";
import { AppNavigation, type AppPage } from "./components/AppNavigation";
import { DeviceWindowToolbar } from "./components/DeviceWindowToolbar";
import { ErrorCopyButton } from "./components/ErrorPresentation";
import { KeyboardIcon } from "./components/KeyboardIcon";
import type { MappingBackgroundMode } from "./components/MappingBackgroundToolbar";
import { MappingOverlay } from "./components/MappingOverlay";
import { PerformanceHud } from "./components/PerformanceHud";
import { PointerDebugOverlay } from "./components/PointerDebugOverlay";
import { WorkspaceLoading } from "./components/WorkspaceLoading";
import { clearLegacyDeviceAudioPreferences, defaultDeviceAudioPreferences, deviceAudioControlAction, readLegacyDeviceAudioPreferences, type DeviceAudioPreferences } from "./deviceAudio";
import { truncatePasteText } from "./deviceText";
import { showErrorMessage } from "./errorMessage";
import { isBoundKey, isUiControl } from "./features/device-control/control";
import { deviceViewScaleFactor, readDeviceViewPreferences, saveDeviceViewPreferences, type DeviceViewPreferences, type DeviceViewScale } from "./deviceViewPreferences";
import { logFrontend } from "./diagnostics";
import { createEditorMapping, duplicateEditorMapping } from "./mappingEditor";
import { devicePerformanceHudItems, readPerformanceHudPreferences, savePerformanceHudPreferences, type PerformanceHudPreferences } from "./performanceHudPreferences";
import { hardwareButtons, keyMappingTypes, updateMappingPosition, type ClipboardEvent, type DeviceEvent, type HardwareButtonName, type KeyMappingType, type Mapping, type Position } from "./types";
import { useDeviceInput, type ControlMode } from "./features/device-control/useDeviceInput";
import { createDeviceControlApi } from "./features/device-control/deviceControlApi";
import { useDeviceRealtimeSession, type KeymapStatus } from "./features/device-media/useDeviceRealtimeSession";
import { useDeviceMediaCapture } from "./features/device-media/useDeviceMediaCapture";
import { usePerformanceTelemetry, useDeviceLogDemand } from "./usePerformanceTelemetry";
import { useBackend } from "./app/providers/backendContext";
import type { BackendRequest } from "./shared/backend/client";
import { currentHostWindow } from "./hostWindow";
import { readAppSettings, readAudioOutputStatus, setAudioEnabled, setAudioPlayback, setStartupDevicePriority, type AudioOutputStatus } from "./appSettings";
import { emptyDeviceStatus, useDeviceSessionController } from "./features/device-session/useDeviceSessionController";
import { useProfileController } from "./features/profiles/useProfileController";

const AfcPage = lazy(() => import("./components/AfcPage").then((module) => ({ default: module.AfcPage })));
const DeviceConnectionCenter = lazy(() => import("./components/DeviceConnectionCenter").then((module) => ({ default: module.DeviceConnectionCenter })));
const DeviceFullscreenToolbar = lazy(() => import("./components/DeviceFullscreenToolbar").then((module) => ({ default: module.DeviceFullscreenToolbar })));
const DeviceInspector = lazy(() => import("./components/DeviceInspector").then((module) => ({ default: module.DeviceInspector })));
const DeviceLogsPage = lazy(() => import("./components/DeviceLogsPage").then((module) => ({ default: module.DeviceLogsPage })));
const LocationPage = lazy(() => import("./components/LocationPage").then((module) => ({ default: module.LocationPage })));
const MappingBackgroundToolbar = lazy(() => import("./components/MappingBackgroundToolbar").then((module) => ({ default: module.MappingBackgroundToolbar })));
const MappingInspector = lazy(() => import("./components/MappingInspector").then((module) => ({ default: module.MappingInspector })));
const KeymapCatalogModal = lazy(() => import("./components/KeymapCatalogModal").then((module) => ({ default: module.KeymapCatalogModal })));
const PerformancePage = lazy(() => import("./components/PerformancePage").then((module) => ({ default: module.PerformancePage })));
const ProfileManager = lazy(() => import("./components/ProfileManager").then((module) => ({ default: module.ProfileManager })));
const SettingsPage = lazy(() => import("./components/SettingsPage").then((module) => ({ default: module.SettingsPage })));

function containSize(containerWidth: number, containerHeight: number, contentWidth: number, contentHeight: number) {
  if (containerWidth <= 0 || containerHeight <= 0 || contentWidth <= 0 || contentHeight <= 0) {
    return { width: 0, height: 0 };
  }
  const scale = Math.min(containerWidth / contentWidth, containerHeight / contentHeight);
  return { width: contentWidth * scale, height: contentHeight * scale };
}

const backendStatusKeys: Record<string, string> = {
  "no device - pick one from the menu": "backendStatus.noDevice",
  "connecting to device...": "backendStatus.connecting",
  "starting screen media stream...": "backendStatus.startingStream",
  "connecting HID...": "backendStatus.connectingHid",
  "device management connected": "backendStatus.managementConnected",
  "waiting for device trust confirmation...": "backendStatus.waitingForTrust",
  "removing device trust...": "backendStatus.removingTrust",
  connected: "backendStatus.connected",
  "stopping...": "backendStatus.stopping",
};

export default function App() {
  const { t } = useTranslation();
  const translateRef = useRef(t);
  translateRef.current = t;
  const appWindow = useMemo(() => currentHostWindow(), []);
  const [page, setPage] = useState<AppPage>("device");
  const [afcVisited, setAfcVisited] = useState(false);
  const { client, connection: backend, error: backendError } = useBackend();
  const releaseAllControlsRef = useRef<() => void>(() => undefined);
  const releaseAllControls = useCallback(() => releaseAllControlsRef.current(), []);
  const {
    status,
    setStatus,
    selectedDeviceId,
    pairingDeviceId,
    disconnect: disconnectDevice,
    reconnect: reconnectDevice,
    select: selectDevice,
    pair: pairDevice,
    refresh: refreshDevices,
  } = useDeviceSessionController({
    client,
    startingStatus: t("status.starting"),
    onReleaseControls: releaseAllControls,
    t,
  });
  const deviceRequest = useMemo<BackendRequest>(() => {
    if (!client || !selectedDeviceId) return () => Promise.reject(new Error(t("errors.backendNotReady")));
    return (path, init) => client.requestForDevice(selectedDeviceId, path, init);
  }, [client, selectedDeviceId, t]);
  const deviceControlApi = useMemo(() => client ? createDeviceControlApi(client) : null, [client]);
  const [catalogOpen, setCatalogOpen] = useState(false);
  const [editing, setEditing] = useState(true);
  const [controlMode, setControlMode] = useState<ControlMode>("mapping");
  const [keymapStatus, setKeymapStatus] = useState<KeymapStatus>({
    configured: false,
    active_mapping_ids: [],
  });
  const lastKeymapErrorRef = useRef<string | null>(null);
  const [alwaysOnTop, setAlwaysOnTop] = useState(false);
  const [systemFullscreen, setSystemFullscreen] = useState(false);
  const [deviceFullscreen, setDeviceFullscreen] = useState(false);
  const [deviceViewPreferences, setDeviceViewPreferences] = useState<DeviceViewPreferences>(readDeviceViewPreferences);
  const [fullscreenToolbarVisible, setFullscreenToolbarVisible] = useState(true);
  const [fullscreenToolbarHovered, setFullscreenToolbarHovered] = useState(false);
  const [fullscreenToolbarFocused, setFullscreenToolbarFocused] = useState(false);
  const [startupDevicePriority, setStartupDevicePriorityState] = useState<string[]>([]);
  const [performanceHud, setPerformanceHud] = useState<PerformanceHudPreferences>(readPerformanceHudPreferences);
  const [audioPlayback, setAudioPlaybackPreferences] = useState<DeviceAudioPreferences>(defaultDeviceAudioPreferences);
  const [deviceAudioEnabled, setDeviceAudioEnabled] = useState<boolean | null>(null);
  const [deviceAudioBusy, setDeviceAudioBusy] = useState(false);
  const [audioOutputState, setAudioOutputState] = useState<AudioOutputStatus["state"] | null>(null);
  const [clipboardEvent, setClipboardEvent] = useState<ClipboardEvent | null>(null);
  const [deviceEvent, setDeviceEvent] = useState<DeviceEvent | null>(null);
  const [textInputOpen, setTextInputOpen] = useState(false);
  const [textInput, setTextInput] = useState("");
  const [textInputBusy, setTextInputBusy] = useState(false);
  const [displayScaleOpen, setDisplayScaleOpen] = useState(false);
  const [mappingBackgroundMode, setMappingBackgroundMode] = useState<MappingBackgroundMode>("live");
  const [mappingGuidesVisible, setMappingGuidesVisible] = useState(false);
  const [documentVisible, setDocumentVisible] = useState(() => document.visibilityState !== "hidden");
  const [stageSize, setStageSize] = useState({ width: 0, height: 0 });
  const stageRef = useRef<HTMLDivElement>(null);
  const mappingInsertPositionRef = useRef<Position>({ x: 0.5, y: 0.5 });
  const audioPlaybackGenerationRef = useRef(0);
  const startupPriorityGenerationRef = useRef(0);
  const fullscreenToolbarTimerRef = useRef<number | null>(null);

  useEffect(() => {
    if (backendError) setStatus({ ...emptyDeviceStatus, status: t("status.backendUnavailable"), error: String(backendError) });
  }, [backendError, setStatus, t]);

  useEffect(() => {
    if (page === "afc") setAfcVisited(true);
  }, [page]);

  const updateAudioPlayback = useCallback(async (next: DeviceAudioPreferences) => {
    const previous = audioPlayback;
    const generation = audioPlaybackGenerationRef.current + 1;
    audioPlaybackGenerationRef.current = generation;
    setAudioPlaybackPreferences(next);
    try {
      const settings = await setAudioPlayback(next.muted, next.volume);
      if (audioPlaybackGenerationRef.current === generation) {
        setAudioPlaybackPreferences({
          muted: settings.audio_muted,
          volume: settings.audio_volume,
        });
      }
    } catch (error) {
      if (audioPlaybackGenerationRef.current === generation) {
        setAudioPlaybackPreferences(previous);
        void showErrorMessage(t("settings.appSettingsUnavailable", { error: String(error) }));
      }
      logFrontend("warn", "audio", "set_playback", error);
    }
  }, [audioPlayback, t]);

  useEffect(() => {
    void readAppSettings()
      .then(async (settings) => {
        let playbackSettings = settings;
        const legacy = readLegacyDeviceAudioPreferences();
        if (legacy) {
          try {
            playbackSettings = await setAudioPlayback(legacy.muted, legacy.volume);
            clearLegacyDeviceAudioPreferences();
            logFrontend("info", "audio", "migrate_playback", "Migrated Web Audio preferences to native playback");
          } catch (error) {
            logFrontend("warn", "audio", "migrate_playback", error);
          }
        }
        setDeviceAudioEnabled(settings.audio_enabled);
        setStartupDevicePriorityState(settings.startup_device_priority ?? []);
        setAudioPlaybackPreferences({
          muted: playbackSettings.audio_muted,
          volume: playbackSettings.audio_volume,
        });
      })
      .catch((error) => logFrontend("warn", "audio", "read_settings", error));
  }, []);

  const updateStartupDevicePriority = useCallback(async (priority: string[]) => {
    const previous = startupDevicePriority;
    const generation = startupPriorityGenerationRef.current + 1;
    startupPriorityGenerationRef.current = generation;
    setStartupDevicePriorityState(priority);
    try {
      const settings = await setStartupDevicePriority(priority);
      if (startupPriorityGenerationRef.current === generation) {
        setStartupDevicePriorityState(settings.startup_device_priority);
      }
    } catch (error) {
      if (startupPriorityGenerationRef.current === generation) {
        setStartupDevicePriorityState(previous);
        void showErrorMessage(t("settings.appSettingsUnavailable", { error: String(error) }));
      }
    }
  }, [startupDevicePriority, t]);

  useEffect(() => {
    if (deviceAudioEnabled !== true) {
      setAudioOutputState(null);
      return;
    }
    let disposed = false;
    const refresh = () => {
      void readAudioOutputStatus()
        .then((status) => {
          if (!disposed) setAudioOutputState(status.state);
        })
        .catch((error) => logFrontend("warn", "audio", "read_output_status", error));
    };
    refresh();
    const timer = window.setInterval(refresh, 2_000);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [deviceAudioEnabled]);

  useEffect(() => {
    if (!clipboardEvent) return;
    const direction = clipboardEvent.from_device ? "fromDevice" : "toDevice";
    const kind = t(`device.clipboardKinds.${clipboardEvent.kind}`);
    void message.info(t(`device.clipboardSynced.${direction}`, {
      kind,
      preview: clipboardEvent.preview,
    }));
  }, [clipboardEvent, t]);

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
  const videoDemand = documentVisible
    && (page === "device" || (page === "mappings" && mappingBackgroundMode === "live"));
  const {
    connected,
    mediaConnected,
    controlGranted,
    streamMetrics,
    renderFps,
    frameSize,
    hasFrame,
    canvasReady,
    streamStalled,
    decoderError,
    browserAudioState,
    resumeBrowserAudio,
    canvasRef,
    canvasReadyRef,
    bindCanvas,
    sendControl,
  } = useDeviceRealtimeSession({
    backend: client,
    deviceId: selectedDeviceId,
    orientation: status.orientation,
    videoDemand,
    monitorStall: Boolean(status.active_udid) && (page === "device" || page === "mappings"),
    audioEnabled: deviceAudioEnabled === true,
    audioMuted: audioPlayback.muted,
    audioVolume: audioPlayback.volume,
    onStatus: setStatus,
    onClipboard: setClipboardEvent,
    onDeviceEvent: setDeviceEvent,
    onKeymapStatus: setKeymapStatus,
  });
  const {
    profile,
    controlProfile,
    profiles,
    activeProfile,
    profileSwitching,
    selectedId,
    setSelectedId,
    appProfileBindings,
    appBindingConflicts,
    updateProfile,
    undoProfile,
    redoProfile,
    canUndoProfile,
    canRedoProfile,
    loadProfile,
    save,
    activateCurrentProfile,
    createProfile,
    duplicateProfile,
    renameProfile,
    deleteCurrentProfile,
    importProfile,
    installCatalogProfile,
    switchControlProfile,
    activateProfileForApp,
    changeAppProfileBinding,
  } = useProfileController({
    client,
    frameSize,
    onReleaseControls: releaseAllControls,
    t,
  });
  useEffect(() => {
    const handleHistoryShortcut = (event: KeyboardEvent) => {
      if (page !== "mappings" || isUiControl(event.target) || event.altKey) return;
      const modifier = event.ctrlKey || event.metaKey;
      const undo = modifier && event.code === "KeyZ" && !event.shiftKey;
      const redo = modifier && ((event.code === "KeyZ" && event.shiftKey) || (event.ctrlKey && event.code === "KeyY"));
      if (undo && canUndoProfile) {
        event.preventDefault();
        undoProfile();
      } else if (redo && canRedoProfile) {
        event.preventDefault();
        redoProfile();
      }
    };
    window.addEventListener("keydown", handleHistoryShortcut);
    return () => window.removeEventListener("keydown", handleHistoryShortcut);
  }, [canRedoProfile, canUndoProfile, page, redoProfile, undoProfile]);
  const handleControlModeChange = useCallback((mode: ControlMode) => {
    setControlMode(mode);
    setEditing(false);
  }, []);
  const handleContactLimit = useCallback(() => {
    void message.warning(translateRef.current("mapping.allContactsUsed"));
  }, []);
  useEffect(() => {
    const error = keymapStatus.error ?? null;
    if (error && error !== lastKeymapErrorRef.current) void message.error(error);
    lastKeymapErrorRef.current = error;
  }, [keymapStatus.error]);
  const {
    activeMappingIds,
    directTouches,
    releaseAllControls: releaseInputControls,
    handlePointerDown,
    handlePointerMove,
    handlePointerUp,
  } = useDeviceInput({
    connected: connected && controlGranted,
    sendControl,
    profile: controlProfile,
    keymapStatus,
    frameSize,
    pointerDebugEnabled: deviceViewPreferences.pointerDebugVisible,
    mappingEditing,
    controlMode,
    onControlModeChange: handleControlModeChange,
    onContactLimit: handleContactLimit,
  });
  releaseAllControlsRef.current = releaseInputControls;
  const activeDeviceName = status.devices.find((device) => device.udid === status.active_udid)?.name ?? "iPhone";
  const useCapturedMappingBackground = useCallback(() => setMappingBackgroundMode("screenshot"), []);
  const {
    capturedScreenshot,
    screenshotBusy,
    recording,
    recordingSupported,
    captureMappingScreenshot,
    saveMappingScreenshot,
    saveDeviceScreenshot,
    toggleDeviceRecording,
  } = useDeviceMediaCapture({
    canvasRef,
    canvasReadyRef,
    hasFrame,
    activeDeviceId: status.active_udid,
    activeDeviceName,
    devicePageActive: page === "device",
    request: deviceRequest,
    onUseCapturedBackground: useCapturedMappingBackground,
    t,
  });
  const mappingFrameSize = mappingBackgroundMode === "screenshot" && capturedScreenshot
    ? { width: capturedScreenshot.width, height: capturedScreenshot.height }
    : frameSize;

  useEffect(() => {
    const handleVisibilityChange = () => setDocumentVisible(document.visibilityState !== "hidden");
    document.addEventListener("visibilitychange", handleVisibilityChange);
    return () => document.removeEventListener("visibilitychange", handleVisibilityChange);
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
  const { view: performanceView, error: performanceError } = usePerformanceTelemetry({
    activeDeviceId: status.active_device_id === selectedDeviceId ? selectedDeviceId : null,
    backendReady: backend !== null,
    enabled: performanceSamplingRequired,
    request: deviceRequest,
  });

  const deviceLogStreamingRequired = Boolean(status.active_udid) && page === "logs";
  useDeviceLogDemand({
    activeDeviceId: status.active_device_id === selectedDeviceId ? selectedDeviceId : null,
    backendReady: backend !== null,
    enabled: deviceLogStreamingRequired,
    request: deviceRequest,
  });

  useEffect(() => {
    Promise.all([appWindow.isAlwaysOnTop(), appWindow.isFullscreen()])
      .then(([top, full]) => { setAlwaysOnTop(top); setSystemFullscreen(full); })
      .catch(() => undefined);
  }, [appWindow]);

  const showFullscreenToolbar = useCallback(() => {
    if (!deviceFullscreen || !deviceViewPreferences.fullscreenToolbarAutoHide) return;
    setFullscreenToolbarVisible(true);
    if (fullscreenToolbarTimerRef.current !== null) window.clearTimeout(fullscreenToolbarTimerRef.current);
    if (fullscreenToolbarHovered || fullscreenToolbarFocused || textInputOpen || displayScaleOpen) {
      fullscreenToolbarTimerRef.current = null;
      return;
    }
    fullscreenToolbarTimerRef.current = window.setTimeout(() => {
      fullscreenToolbarTimerRef.current = null;
      setFullscreenToolbarVisible(false);
    }, 2_200);
  }, [deviceFullscreen, deviceViewPreferences.fullscreenToolbarAutoHide, displayScaleOpen, fullscreenToolbarFocused, fullscreenToolbarHovered, textInputOpen]);

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
    if (deviceEvent?.kind === "device_name_changed") {
      void refreshDevices();
    }
    if (deviceEvent?.kind === "lock_state_changed") releaseAllControls();
  }, [deviceEvent, refreshDevices, releaseAllControls]);

  const updateMapping = (next: Mapping, mergeKey?: string) => updateProfile((current) => {
    const keyConflict = hardwareButtons.some((button) => {
      const key = current.hardwareBindings[button.name];
      return key && isBoundKey([next], key);
    });
    if (keyConflict) {
      void message.warning(t("mapping.keyUsedByHardware"));
      return current;
    }
    return { ...current, mappings: current.mappings.map((mapping) => mapping.id === next.id ? next : mapping) };
  }, mergeKey ? { mergeKey: `mapping:${next.id}:${mergeKey}` } : undefined);
  const updateHardwareBinding = (name: HardwareButtonName, key: string) => updateProfile((current) => {
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
  const moveMapping = (id: string, x: number, y: number) => updateProfile((current) => ({ ...current, mappings: current.mappings.map((mapping) => mapping.id === id ? updateMappingPosition(mapping, { x, y }) : mapping) }), { mergeKey: `move:${id}` });
  const addMapping = (type: KeyMappingType, position: Position = { x: 0.5, y: 0.5 }) => {
    const next = createEditorMapping(type, position, mappingFrameSize, profile.mappings);
    const id = next.id;
    updateProfile((current) => ({ ...current, mappings: [...current.mappings, next] }));
    setSelectedId(id);
  };
  const duplicateMapping = (id: string) => {
    const source = profile.mappings.find((mapping) => mapping.id === id);
    if (!source) return;
    const next = duplicateEditorMapping(source, profile.mappings);
    updateProfile((current) => ({ ...current, mappings: [...current.mappings, next] }));
    setSelectedId(next.id);
  };
  const deleteMapping = (id: string) => {
    updateProfile((current) => ({ ...current, mappings: current.mappings.filter((mapping) => mapping.id !== id) }));
    setSelectedId(null);
  };
  const toggleAlwaysOnTop = async () => {
    const next = !alwaysOnTop;
    try {
      await appWindow.setAlwaysOnTop(next);
      setAlwaysOnTop(next);
    } catch (error) {
      void showErrorMessage(t("errors.windowTop", { error: String(error) }));
    }
  };
  const toggleSystemFullscreen = async () => {
    const next = !systemFullscreen;
    releaseAllControls();
    try {
      await appWindow.setFullscreen(next);
      setSystemFullscreen(next);
    } catch (error) {
      void showErrorMessage(t("errors.systemFullscreen", { error: String(error) }));
    }
  };
  const toggleDeviceFullscreen = () => {
    releaseAllControls();
    setFullscreenToolbarVisible(true);
    setDeviceFullscreen((active) => !active);
    setPage("device");
  };
  const pasteTextToDevice = async () => {
    if (!textInput || textInputBusy) return;
    setTextInputBusy(true);
    try {
      if (!deviceControlApi || !selectedDeviceId) throw new Error(t("errors.backendNotReady"));
      await deviceControlApi.pasteText(selectedDeviceId, textInput);
      setTextInput("");
      setTextInputOpen(false);
    } catch (error) {
      void showErrorMessage(t("device.pasteTextFailed", { error: String(error) }));
      logFrontend("warn", "clipboard", "paste_text", error);
    } finally {
      setTextInputBusy(false);
    }
  };
  const toggleDeviceAudio = async () => {
    const action = deviceAudioControlAction(deviceAudioEnabled, audioPlayback.muted);
    if (action === "unavailable" || deviceAudioBusy) return;
    if (action === "mute" && browserAudioState !== "running") {
      const state = await resumeBrowserAudio();
      if (state === "running") {
        void message.success(t("device.deviceAudioPlaybackStarted"));
      } else {
        void message.warning(t("device.deviceAudioPlaybackStillSuspended"));
      }
      return;
    }
    if (action !== "enable") {
      if (action === "unmute") {
        await updateAudioPlayback({ ...audioPlayback, muted: false });
      } else if (action === "mute") {
        await updateAudioPlayback({ ...audioPlayback, muted: true });
      }
      return;
    }
    setDeviceAudioBusy(true);
    try {
      const settings = await setAudioEnabled(true);
      setDeviceAudioEnabled(settings.audio_enabled);
      await updateAudioPlayback({ ...audioPlayback, muted: false });
      const reconnecting = await reconnectDevice();
      void message.success(t(reconnecting ? "device.deviceAudioEnabled" : "device.deviceAudioEnabledReconnectManually"));
      logFrontend(
        "info",
        "audio",
        "enabled",
        reconnecting ? "Reconnect requested; native playback enabled" : "Manual reconnect required; native playback enabled",
      );
    } catch (error) {
      void showErrorMessage(t("device.deviceAudioEnableFailed", { error: String(error) }));
      logFrontend("error", "audio", "enable_failed", error);
    } finally {
      setDeviceAudioBusy(false);
    }
  };
  const handleDeviceContextMenu = (event: ReactMouseEvent<HTMLDivElement>) => {
    if (page === "mappings" && mappingEditing) {
      const bounds = event.currentTarget.getBoundingClientRect();
      mappingInsertPositionRef.current = {
        x: Math.max(0, Math.min(1, (event.clientX - bounds.left) / bounds.width)),
        y: Math.max(0, Math.min(1, (event.clientY - bounds.top) / bounds.height)),
      };
      event.preventDefault();
      return;
    }
    if (mappingEditing) return;
    event.preventDefault();
    if (page === "device" && isBoundKey(controlProfile.mappings, "MouseRight")) return;
    if (page === "device" && connected && controlGranted && status.active_udid) {
      sendControl({ type: "button", name: "home" });
    }
  };
  const controlOverlayVisible = deviceViewPreferences.controlOverlayVisible;
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
    : !mediaConnected
      ? "reconnecting"
      : decoderError
        ? "decoder"
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
  const hardwareControlEntries = [
    ["home", <HomeOutlined />],
    ["lock", <LockOutlined />],
    ["volume-up", <PlusOutlined />],
    ["volume-down", <MinusOutlined />],
    ["mute", <AudioMutedOutlined />],
    ["siri", <CustomerServiceOutlined />],
    ["action", <ThunderboltOutlined />],
  ] as const;
  const systemControlEntries = [
    ["app-switcher", <AppstoreOutlined />],
  ] as const;
  const renderHardwareControls = (includeHome: boolean) => (
    <div className="hardware-controls" role="toolbar" aria-label={t("hardware.toolbar")}>
      {hardwareControlEntries.filter(([name]) => includeHome || name !== "home").map(([name, icon]) => {
        const label = t(`hardware.${name}`);
        return (
          <Tooltip key={name} title={`${label}${controlProfile.hardwareBindings[name] ? ` · ${controlProfile.hardwareBindings[name]}` : ""}`}>
            <Button disabled={!controlGranted} aria-label={label} icon={icon} onClick={() => sendControl({ type: "button", name })} />
          </Tooltip>
        );
      })}
      {systemControlEntries.map(([name, icon]) => {
        const label = t(`system.${name}`);
        return (
          <Tooltip key={name} title={label}>
            <Button
              disabled={!controlGranted}
              aria-label={label}
              icon={icon}
              onClick={() => sendControl({ type: "system_action", action: name })}
            />
          </Tooltip>
        );
      })}
    </div>
  );
  const hardwareControls = renderHardwareControls(true);
  const audioControlAction = deviceAudioControlAction(
    deviceAudioEnabled,
    audioPlayback.muted,
  );
  const browserAudioNeedsStart = audioControlAction === "mute" && browserAudioState !== "running";
  const audioControlLabel = audioOutputState === "unavailable"
    ? t("device.deviceAudioOutputUnavailable")
    : browserAudioNeedsStart
      ? t("device.startDeviceAudioPlayback")
    : t({
    unavailable: "device.startDeviceAudioPlayback",
    enable: "device.enableDeviceAudio",
    unmute: "device.unmuteDeviceAudio",
    mute: "device.muteDeviceAudio",
  }[audioControlAction]);
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
        <Button
          aria-label={t("device.saveScreenshot")}
          disabled={!status.active_udid && !canvasReady}
          loading={screenshotBusy}
          icon={<CameraOutlined />}
          onClick={() => void saveDeviceScreenshot()}
        />
      </Tooltip>
      <Tooltip title={t(deviceViewPreferences.pointerDebugVisible ? "device.hidePointerDebug" : "device.showPointerDebug")}>
        <Button
          aria-label={t(deviceViewPreferences.pointerDebugVisible ? "device.hidePointerDebug" : "device.showPointerDebug")}
          type={deviceViewPreferences.pointerDebugVisible ? "primary" : "default"}
          icon={<BugOutlined />}
          onClick={() => patchDeviceViewPreferences({ pointerDebugVisible: !deviceViewPreferences.pointerDebugVisible })}
        />
      </Tooltip>
      <Tooltip title={audioControlLabel}>
        <Button
          aria-label={audioControlLabel}
          type={deviceAudioEnabled && !audioPlayback.muted ? "primary" : "default"}
          danger={audioOutputState === "unavailable" || browserAudioState === "failed"}
          disabled={deviceAudioEnabled === null}
          loading={deviceAudioBusy}
          icon={deviceAudioEnabled && !audioPlayback.muted
            ? <SoundOutlined />
            : <AudioMutedOutlined />}
          onClick={() => void toggleDeviceAudio()}
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
              rows={3}
              disabled={textInputBusy}
              placeholder={t("device.textInputPlaceholder")}
              onChange={(event) => setTextInput(truncatePasteText(event.target.value))}
            />
            <Typography.Text type="secondary">{t("device.textInputHint")}</Typography.Text>
            <Button
              type="primary"
              icon={<SendOutlined />}
              loading={textInputBusy}
              disabled={!textInput || !connected || !status.active_udid}
              onClick={() => void pasteTextToDevice()}
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
          <span className="topbar-status">
            <Tag color={connected && status.active_udid ? "success" : status.error ? "error" : "default"}>{statusText}</Tag>
            {status.error && <ErrorCopyButton error={status.error} />}
          </span>
          <Suspense fallback={<Button disabled>{t("device.select")}</Button>}>
            <DeviceConnectionCenter
              devices={status.devices}
              selectedDeviceId={selectedDeviceId}
              backendReady={Boolean(backend)}
              pairingDeviceId={pairingDeviceId}
              startupDevicePriority={startupDevicePriority}
              onStartupDevicePriorityChange={(priority) => void updateStartupDevicePriority(priority)}
              onConnect={(deviceId) => void selectDevice(deviceId)}
              onReconnect={(deviceId) => void reconnectDevice(deviceId)}
              onDisconnect={(deviceId) => void disconnectDevice(deviceId)}
              onPair={(deviceId) => void pairDevice(deviceId)}
              onRefresh={() => void refreshDevices()}
            />
          </Suspense>
          {page === "mappings" && <Tooltip title={t("device.saveMappings")}><Button icon={<SaveOutlined />} onClick={() => void save()} /></Tooltip>}
          <Tooltip title={t(alwaysOnTop ? "device.unpin" : "device.pin")}><Button type={alwaysOnTop ? "primary" : "default"} icon={alwaysOnTop ? <PushpinFilled /> : <PushpinOutlined />} onClick={() => void toggleAlwaysOnTop()} /></Tooltip>
          {page === "device" && <Tooltip title={t(deviceViewPreferences.deviceInspectorVisible ? "device.hideDeviceInspector" : "device.showDeviceInspector")}><Button aria-label={t(deviceViewPreferences.deviceInspectorVisible ? "device.hideDeviceInspector" : "device.showDeviceInspector")} icon={deviceViewPreferences.deviceInspectorVisible ? <MenuFoldOutlined /> : <MenuUnfoldOutlined />} onClick={() => patchDeviceViewPreferences({ deviceInspectorVisible: !deviceViewPreferences.deviceInspectorVisible })} /></Tooltip>}
          {page === "mappings" && <Tooltip title={t(deviceViewPreferences.mappingInspectorVisible ? "device.hideInspector" : "device.showInspector")}><Button aria-label={t(deviceViewPreferences.mappingInspectorVisible ? "device.hideInspector" : "device.showInspector")} icon={deviceViewPreferences.mappingInspectorVisible ? <MenuFoldOutlined /> : <MenuUnfoldOutlined />} onClick={() => patchDeviceViewPreferences({ mappingInspectorVisible: !deviceViewPreferences.mappingInspectorVisible })} /></Tooltip>}
          <Tooltip title={t("device.enterDeviceFullscreen")}><Button icon={<ExpandOutlined />} onClick={toggleDeviceFullscreen} /></Tooltip>
          <Tooltip title={t(systemFullscreen ? "device.exitSystemFullscreen" : "device.enterSystemFullscreen")}><Button icon={systemFullscreen ? <FullscreenExitOutlined /> : <FullscreenOutlined />} onClick={() => void toggleSystemFullscreen()} /></Tooltip>
        </Space>
      </header>}

      <div className="desktop-body">
        {!deviceFullscreen && <AppNavigation page={page} onChange={(next) => { releaseAllControls(); setPage(next); }} />}
        <div className="page-content">
          <Suspense fallback={<WorkspaceLoading />}>
          {(afcVisited || page === "afc") && <AfcPage active={page === "afc"} activeUdid={status.active_udid} request={deviceRequest} />}
          {page === "afc" ? null : page === "settings" ? (
            <SettingsPage
              alwaysOnTop={alwaysOnTop}
              systemFullscreen={systemFullscreen}
              deviceView={deviceViewPreferences}
              performanceHud={performanceHud}
              audioPlayback={audioPlayback}
              onAlwaysOnTopChange={() => void toggleAlwaysOnTop()}
              onSystemFullscreenChange={() => void toggleSystemFullscreen()}
              onDeviceViewChange={updateDeviceViewPreferences}
              onPerformanceHudChange={updatePerformanceHud}
              onAudioPlaybackChange={(preferences) => void updateAudioPlayback(preferences)}
              onAudioEnabledChange={setDeviceAudioEnabled}
            />
          ) : page === "location" ? (
            <LocationPage activeUdid={status.active_udid} status={status.location} request={deviceRequest} />
          ) : page === "performance" ? (
            <PerformancePage
              activeUdid={status.active_udid}
              deviceName={status.devices.find((device) => device.udid === status.active_udid)?.name ?? "iPhone"}
              streamMetrics={streamMetrics}
              renderFps={renderFps}
              view={performanceView}
              error={performanceError}
              request={deviceRequest}
            />
          ) : page === "logs" ? (
            <DeviceLogsPage activeUdid={status.active_udid} request={deviceRequest} />
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
                  onBundleIdentifiersChange={(bundleIdentifiers) => updateProfile((current) => ({ ...current, bundleIdentifiers }))}
                  onTargetResolutionChange={(targetResolution) => updateProfile((current) => ({ ...current, targetResolution }))}
                  onImport={importProfile}
                  onBrowseCatalog={() => setCatalogOpen(true)}
                  hasUnsavedChanges={canUndoProfile}
                  canUndo={canUndoProfile}
                  canRedo={canRedoProfile}
                  onUndo={undoProfile}
                  onRedo={redoProfile}
                />
              )}
              {catalogOpen && <KeymapCatalogModal
                open={catalogOpen}
                request={deviceRequest}
                profiles={profiles}
                activeDeviceId={selectedDeviceId}
                frameSize={frameSize}
                orientation={status.orientation}
                hasFrame={hasFrame}
                onClose={() => setCatalogOpen(false)}
                onInstalled={installCatalogProfile}
              />}
              <main className={`workspace ${deviceFullscreen ? "inspector-hidden" : page === "device" && deviceViewPreferences.deviceInspectorVisible ? "device-workspace" : page === "mappings" && deviceViewPreferences.mappingInspectorVisible ? "mapping-workspace" : "inspector-hidden"}`}>
                <section className="stage-column">
                  {deviceFullscreen ? (
                    <DeviceFullscreenToolbar
                      visible={fullscreenToolbarVisible}
                      canReconnect={Boolean(backend && selectedDeviceId)}
                      controlMode={controlMode}
                      controlOverlayVisible={controlOverlayVisible}
                      rotationControlsLocked={deviceViewPreferences.rotationControlsLocked || !controlGranted}
                      hardwareDock={deviceViewPreferences.fullscreenHardwareToolbarDock}
                      functionDock={deviceViewPreferences.fullscreenFunctionToolbarDock}
                      toolbarsAttached={deviceViewPreferences.fullscreenToolbarsAttached}
                      hardwareControls={hardwareControls}
                      profileSelector={controlProfileSelector}
                      displayControls={deviceDisplayControls}
                      systemFullscreenControl={<Tooltip title={t(systemFullscreen ? "device.exitSystemFullscreen" : "device.enterSystemFullscreen")}><Button aria-label={t(systemFullscreen ? "device.exitSystemFullscreen" : "device.enterSystemFullscreen")} icon={systemFullscreen ? <FullscreenExitOutlined /> : <FullscreenOutlined />} onClick={() => void toggleSystemFullscreen()} /></Tooltip>}
                      onReconnect={() => void reconnectDevice()}
                      onControlModeChange={(mode) => {
                        releaseAllControls();
                        setControlMode(mode);
                        if (mode === "keyboard") setEditing(false);
                      }}
                      onControlOverlayChange={() => patchDeviceViewPreferences({ controlOverlayVisible: !controlOverlayVisible })}
                      onRotateLeft={() => sendControl({ type: "rotate", direction: "left" })}
                      onRotateRight={() => sendControl({ type: "rotate", direction: "right" })}
                      onLayoutChange={(fullscreenHardwareToolbarDock, fullscreenFunctionToolbarDock, fullscreenToolbarsAttached) => patchDeviceViewPreferences({
                        fullscreenHardwareToolbarDock,
                        fullscreenFunctionToolbarDock,
                        fullscreenToolbarsAttached,
                      })}
                      onExit={toggleDeviceFullscreen}
                      onPointerEnter={() => setFullscreenToolbarHovered(true)}
                      onPointerLeave={() => setFullscreenToolbarHovered(false)}
                      onFocus={() => setFullscreenToolbarFocused(true)}
                      onBlur={(event) => {
                        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setFullscreenToolbarFocused(false);
                      }}
                    />
                  ) : (
                    <DeviceWindowToolbar
                      status={(
                        <div className="stream-status">
                          <Space><ApiOutlined /><Typography.Text>{t(connected ? "status.websocketConnected" : "status.reconnecting")}</Typography.Text></Space>
                          {connected && !controlGranted && <Typography.Text type="warning">{t("status.viewOnly")}</Typography.Text>}
                          <Tooltip title={t("device.bandwidth", { value: streamMetrics.megabits_per_second.toFixed(1) })}>
                            <Typography.Text className="stream-metrics">
                              {t("device.metrics", {
                                source: streamMetrics.source_fps.toFixed(0),
                                decoded: streamMetrics.decoded_fps.toFixed(0),
                                sent: streamMetrics.sent_fps.toFixed(0),
                                render: renderFps.toFixed(0),
                                accept: streamMetrics.decoder_accept_ms.toFixed(1),
                              })}
                            </Typography.Text>
                          </Tooltip>
                        </div>
                      )}
                      functionControls={(
                        <Space>
                          {page === "device" && controlProfileSelector}
                          <Segmented<ControlMode>
                            value={controlMode}
                            options={[
                              { label: t("device.mappingMode"), value: "mapping", icon: <AimOutlined /> },
                              { label: t("device.keyboardMode"), value: "keyboard", icon: <KeyboardIcon /> },
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
                          <Tooltip title={t("device.rotateLeft")}><Button disabled={deviceViewPreferences.rotationControlsLocked || !controlGranted} icon={<RotateLeftOutlined />} onClick={() => sendControl({ type: "rotate", direction: "left" })} /></Tooltip>
                          <Tooltip title={t("device.rotateRight")}><Button disabled={deviceViewPreferences.rotationControlsLocked || !controlGranted} icon={<RotateRightOutlined />} onClick={() => sendControl({ type: "rotate", direction: "right" })} /></Tooltip>
                        </Space>
                      )}
                      hardwareControls={hardwareControls}
                    />
                  )}
                  {page === "mappings" && (
                    <MappingBackgroundToolbar
                      mode={mappingBackgroundMode}
                      sourceSize={mappingFrameSize}
                      viewportSize={viewportSize}
                      screenshotAvailable={capturedScreenshot !== null}
                      canCapture={hasFrame}
                      showGuides={mappingGuidesVisible}
                      onModeChange={setMappingBackgroundMode}
                      onCapture={() => void captureMappingScreenshot(true)}
                      onSave={() => void saveMappingScreenshot(mappingBackgroundMode === "live")}
                      onShowGuidesChange={setMappingGuidesVisible}
                    />
                  )}
                  <div className={`stage-wrap${viewportScrollable ? " is-scrollable" : ""}`} ref={stageRef}>
                    <Dropdown
                      disabled={page !== "mappings" || !mappingEditing}
                      trigger={["contextMenu"]}
                      menu={{
                        items: keyMappingTypes.map((type) => ({ key: type, label: t(`mapping.types.${type}`) })),
                        onClick: ({ key }) => addMapping(key as KeyMappingType, mappingInsertPositionRef.current),
                      }}
                    >
                      <div
                        className={`device-viewport ${mappingEditing ? "is-editing" : "is-controlling"}`}
                        style={{ aspectRatio, width: viewportSize.width, height: viewportSize.height }}
                        tabIndex={0}
                        onPointerDown={handlePointerDown}
                        onPointerMove={handlePointerMove}
                        onPointerUp={handlePointerUp}
                        onPointerCancel={handlePointerUp}
                        onLostPointerCapture={handlePointerUp}
                        onContextMenu={handleDeviceContextMenu}
                      >
                      <canvas ref={bindCanvas} />
                      {page === "device" && performanceHud.enabled && (
                        <PerformanceHud
                          items={performanceHud.items}
                          view={performanceView}
                          streamMetrics={streamMetrics}
                          renderFps={renderFps}
                          avoidFullscreenToolbar={deviceFullscreen && fullscreenToolbarVisible && (
                            deviceViewPreferences.fullscreenHardwareToolbarDock.startsWith("top-")
                            || deviceViewPreferences.fullscreenFunctionToolbarDock.startsWith("top-")
                          )}
                        />
                      )}
                      {page === "mappings" && mappingBackgroundMode === "screenshot" && capturedScreenshot && (
                        <img className="mapping-screenshot" src={capturedScreenshot.url} alt="" draggable={false} />
                      )}
                      {(page === "mappings" || controlOverlayVisible) && (
                        <MappingOverlay mappings={displayedMappings} selectedId={selectedId} editing={mappingEditing} showGuides={mappingGuidesVisible} frameSize={displayedFrameSize} activeMappingIds={activeMappingIds} onSelect={setSelectedId} onMove={moveMapping} />
                      )}
                      {!deviceViewPreferences.pointerDebugVisible && directTouches.map((contact) => (
                        <span key={contact.identity} className="direct-touch" style={{ left: `${contact.x * 100}%`, top: `${contact.y * 100}%` }} />
                      ))}
                      <PointerDebugOverlay
                        visible={deviceViewPreferences.pointerDebugVisible}
                        frameSize={displayedFrameSize}
                        orientation={status.orientation}
                        directTouches={directTouches}
                        keymapConfigured={keymapStatus.configured}
                        keymapContacts={keymapStatus.active_contacts}
                        activeMappingIds={keymapStatus.active_mapping_ids}
                        unavailableMappingIds={keymapStatus.unavailable_mapping_ids ?? []}
                      />
                      {stageIssue && !(page === "mappings" && mappingBackgroundMode === "screenshot" && capturedScreenshot) && (
                        <div className="device-stage-state" onPointerDown={(event) => event.stopPropagation()}>
                          <AimOutlined />
                          <span>{t(`device.stageState.${stageIssue}`)}</span>
                          {stageIssue !== "waiting" && stageIssue !== "decoder" && selectedDeviceId && (
                            <Button size="small" icon={<SyncOutlined />} onClick={() => void reconnectDevice()}>{t("device.reconnect")}</Button>
                          )}
                        </div>
                      )}
                      </div>
                    </Dropdown>
                  </div>
                </section>
                {page === "mappings" && deviceViewPreferences.mappingInspectorVisible && (
                  <MappingInspector
                    mappings={profile.mappings}
                    selectedId={selectedId}
                    onSelect={setSelectedId}
                    onChange={updateMapping}
                    onAdd={addMapping}
                    onDuplicate={duplicateMapping}
                    onDelete={deleteMapping}
                    frameSize={mappingFrameSize}
                    hardwareBindings={profile.hardwareBindings}
                    onHardwareBindingChange={updateHardwareBinding}
                  />
                )}
                {page === "device" && !deviceFullscreen && (
                  <div className={`device-inspector-slot${deviceViewPreferences.deviceInspectorVisible ? "" : " is-hidden"}`}>
                    {status.phase !== "connected" ? <WorkspaceLoading inspector /> : (
                      <Suspense fallback={<WorkspaceLoading inspector />}>
                        <DeviceInspector
                          activeUdid={status.active_udid}
                          activeDeviceId={status.active_device_id}
                          canForgetTrust={status.devices.some((device) => device.id === status.active_device_id && device.connection === "USB" && device.pairing === "paired")}
                          request={deviceRequest}
                          activeProfile={activeProfile}
                          appProfileBindings={appProfileBindings}
                          bindingConflicts={appBindingConflicts}
                          frameSize={frameSize}
                          deviceEvent={deviceEvent}
                          onAppLaunched={(bundleId) => void activateProfileForApp(bundleId)}
                          onAppProfileBindingChange={changeAppProfileBinding}
                        />
                      </Suspense>
                    )}
                  </div>
                )}
              </main>
            </>
          )}
          </Suspense>
        </div>
      </div>
    </div>
  );
}
