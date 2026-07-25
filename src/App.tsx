import AimOutlined from "@ant-design/icons/es/icons/AimOutlined";
import ApiOutlined from "@ant-design/icons/es/icons/ApiOutlined";
import AudioMutedOutlined from "@ant-design/icons/es/icons/AudioMutedOutlined";
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
import ReloadOutlined from "@ant-design/icons/es/icons/ReloadOutlined";
import RotateLeftOutlined from "@ant-design/icons/es/icons/RotateLeftOutlined";
import RotateRightOutlined from "@ant-design/icons/es/icons/RotateRightOutlined";
import SaveOutlined from "@ant-design/icons/es/icons/SaveOutlined";
import SafetyCertificateOutlined from "@ant-design/icons/es/icons/SafetyCertificateOutlined";
import SendOutlined from "@ant-design/icons/es/icons/SendOutlined";
import SoundOutlined from "@ant-design/icons/es/icons/SoundOutlined";
import StopOutlined from "@ant-design/icons/es/icons/StopOutlined";
import SyncOutlined from "@ant-design/icons/es/icons/SyncOutlined";
import ThunderboltOutlined from "@ant-design/icons/es/icons/ThunderboltOutlined";
import VideoCameraOutlined from "@ant-design/icons/es/icons/VideoCameraOutlined";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Button, Dropdown, Input, Popover, Segmented, Select, Space, Switch, Tag, Tooltip, Typography, message } from "antd";
import { lazy, Suspense, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type MouseEvent as ReactMouseEvent } from "react";
import { useTranslation } from "react-i18next";
import { AppNavigation, type AppPage } from "./components/AppNavigation";
import { DeviceFullscreenToolbar } from "./components/DeviceFullscreenToolbar";
import { ErrorCopyButton } from "./components/ErrorPresentation";
import { KeyboardIcon } from "./components/KeyboardIcon";
import type { MappingBackgroundMode } from "./components/MappingBackgroundToolbar";
import { MappingOverlay } from "./components/MappingOverlay";
import { PerformanceHud } from "./components/PerformanceHud";
import { WorkspaceLoading } from "./components/WorkspaceLoading";
import { clearLegacyDeviceAudioPreferences, defaultDeviceAudioPreferences, deviceAudioControlAction, readLegacyDeviceAudioPreferences, type DeviceAudioPreferences } from "./deviceAudio";
import { truncatePasteText } from "./deviceText";
import { showErrorMessage } from "./errorMessage";
import { isBoundKey, isUiControl } from "./control";
import { deviceViewScaleFactor, readDeviceViewPreferences, saveDeviceViewPreferences, type DeviceViewPreferences, type DeviceViewScale } from "./deviceViewPreferences";
import { logFrontend } from "./diagnostics";
import { createEditorMapping, duplicateEditorMapping } from "./mappingEditor";
import { devicePerformanceHudItems, readPerformanceHudPreferences, savePerformanceHudPreferences, type PerformanceHudPreferences } from "./performanceHudPreferences";
import { defaultHardwareBindings, defaultProfile, hardwareButtons, scrcpyMappingTypes, type ClipboardEvent, type DeviceEvent, type DeviceStatus, type HardwareButtonName, type Mapping, type PairDeviceResult, type Position, type Profile, type ScrcpyMappingType } from "./types";
import { useDeviceInput, type ControlMode } from "./useDeviceInput";
import { useDeviceVideoStream } from "./useDeviceVideoStream";
import { useDeviceMediaCapture } from "./useDeviceMediaCapture";
import { usePerformanceTelemetry, useDeviceLogDemand } from "./usePerformanceTelemetry";
import { usePrivateBackend } from "./usePrivateBackend";
import { useUndoHistory } from "./useUndoHistory";
import { readAppSettings, readAudioOutputStatus, setAudioEnabled, setAudioPlayback, type AudioOutputStatus } from "./appSettings";

const AfcPage = lazy(() => import("./components/AfcPage").then((module) => ({ default: module.AfcPage })));
const DeviceInspector = lazy(() => import("./components/DeviceInspector").then((module) => ({ default: module.DeviceInspector })));
const DeviceLogsPage = lazy(() => import("./components/DeviceLogsPage").then((module) => ({ default: module.DeviceLogsPage })));
const LocationPage = lazy(() => import("./components/LocationPage").then((module) => ({ default: module.LocationPage })));
const MappingBackgroundToolbar = lazy(() => import("./components/MappingBackgroundToolbar").then((module) => ({ default: module.MappingBackgroundToolbar })));
const MappingInspector = lazy(() => import("./components/MappingInspector").then((module) => ({ default: module.MappingInspector })));
const PerformancePage = lazy(() => import("./components/PerformancePage").then((module) => ({ default: module.PerformancePage })));
const ProfileManager = lazy(() => import("./components/ProfileManager").then((module) => ({ default: module.ProfileManager })));
const SettingsPage = lazy(() => import("./components/SettingsPage").then((module) => ({ default: module.SettingsPage })));

const emptyStatus: DeviceStatus = {
  status: "",
  active_udid: null,
  active_device_id: null,
  error: null,
  orientation: "portrait",
  devices: [],
  location: { available: false, active: false, backend: null, latitude: null, longitude: null, error: null },
};
type ProfileList = { profiles: string[]; active: string; app_bindings: Record<string, string>; binding_conflicts: string[] };

function containSize(containerWidth: number, containerHeight: number, contentWidth: number, contentHeight: number) {
  if (containerWidth <= 0 || containerHeight <= 0 || contentWidth <= 0 || contentHeight <= 0) {
    return { width: 0, height: 0 };
  }
  const scale = Math.min(containerWidth / contentWidth, containerHeight / contentHeight);
  return { width: contentWidth * scale, height: contentHeight * scale };
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
  "waiting for device trust confirmation...": "backendStatus.waitingForTrust",
  "removing device trust...": "backendStatus.removingTrust",
  connected: "backendStatus.connected",
  "stopping...": "backendStatus.stopping",
};

export default function App() {
  const { t } = useTranslation();
  const translateRef = useRef(t);
  translateRef.current = t;
  const appWindow = useMemo(() => getCurrentWindow(), []);
  const [page, setPage] = useState<AppPage>("device");
  const [afcVisited, setAfcVisited] = useState(false);
  const [status, setStatus] = useState<DeviceStatus>(() => ({ ...emptyStatus, status: t("status.starting") }));
  const { backend, request } = usePrivateBackend((error) => {
    setStatus({ ...emptyStatus, status: t("status.backendUnavailable"), error: String(error) });
  }, t("errors.backendNotReady"));
  const {
    value: profile,
    update: updateProfile,
    reset: resetProfile,
    undo: undoProfile,
    redo: redoProfile,
    canUndo: canUndoProfile,
    canRedo: canRedoProfile,
  } = useUndoHistory<Profile>(() => createLocalizedDefaultProfile(t));
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
  const [fullscreenToolbarHovered, setFullscreenToolbarHovered] = useState(false);
  const [fullscreenToolbarFocused, setFullscreenToolbarFocused] = useState(false);
  const [fullscreenOverflowOpen, setFullscreenOverflowOpen] = useState(false);
  const [selectedDeviceId, setSelectedDeviceId] = useState<string | null>(null);
  const [pairingDeviceId, setPairingDeviceId] = useState<string | null>(null);
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
  const fullscreenToolbarTimerRef = useRef<number | null>(null);
  const selectedDeviceIntentRef = useRef<string | null>(null);
  const profileSwitchingRef = useRef(false);

  useEffect(() => {
    const intended = selectedDeviceIntentRef.current;
    if (intended) {
      if (status.active_device_id === intended) {
        selectedDeviceIntentRef.current = null;
        setSelectedDeviceId(intended);
      } else if (status.devices.some((device) => device.id === intended)) {
        return;
      } else {
        selectedDeviceIntentRef.current = null;
      }
    }
    if (status.active_device_id) setSelectedDeviceId(status.active_device_id);
  }, [status.active_device_id, status.devices]);

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
        setAudioPlaybackPreferences({
          muted: playbackSettings.audio_muted,
          volume: playbackSettings.audio_volume,
        });
      })
      .catch((error) => logFrontend("warn", "audio", "read_settings", error));
  }, []);

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
  const videoDemand = documentVisible
    && (page === "device" || (page === "mappings" && mappingBackgroundMode === "live"));
  const {
    connected,
    streamMetrics,
    renderFps,
    frameSize,
    hasFrame,
    canvasReady,
    streamStalled,
    decoderError,
    canvasRef,
    canvasReadyRef,
    bindCanvas,
    send: command,
  } = useDeviceVideoStream({
    backend,
    orientation: status.orientation,
    videoDemand,
    monitorStall: Boolean(status.active_udid) && (page === "device" || page === "mappings"),
    onStatus: setStatus,
    onClipboard: setClipboardEvent,
    onDeviceEvent: setDeviceEvent,
  });
  const handleControlModeChange = useCallback((mode: ControlMode) => {
    setControlMode(mode);
    setEditing(false);
  }, []);
  const handleContactLimit = useCallback(() => {
    void message.warning(translateRef.current("mapping.allContactsUsed"));
  }, []);
  const {
    activeMappingIds,
    directTouches,
    releaseAllControls,
    handlePointerDown,
    handlePointerMove,
    handlePointerUp,
  } = useDeviceInput({
    connected,
    command,
    mappings: controlProfile.mappings,
    hardwareBindings: controlProfile.hardwareBindings,
    frameSize,
    mappingEditing,
    controlMode,
    onControlModeChange: handleControlModeChange,
    onContactLimit: handleContactLimit,
  });
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
    request,
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
    activeUdid: status.active_udid,
    backendReady: backend !== null,
    enabled: performanceSamplingRequired,
    request,
  });

  const deviceLogStreamingRequired = Boolean(status.active_udid) && page === "logs";
  useDeviceLogDemand({ backendReady: backend !== null, enabled: deviceLogStreamingRequired, request });

  useEffect(() => {
    Promise.all([appWindow.isAlwaysOnTop(), appWindow.isFullscreen()])
      .then(([top, full]) => { setAlwaysOnTop(top); setSystemFullscreen(full); })
      .catch(() => undefined);
  }, [appWindow]);

  const showFullscreenToolbar = useCallback(() => {
    if (!deviceFullscreen || !deviceViewPreferences.fullscreenToolbarAutoHide) return;
    setFullscreenToolbarVisible(true);
    if (fullscreenToolbarTimerRef.current !== null) window.clearTimeout(fullscreenToolbarTimerRef.current);
    if (fullscreenToolbarHovered || fullscreenToolbarFocused || fullscreenOverflowOpen || textInputOpen || displayScaleOpen) {
      fullscreenToolbarTimerRef.current = null;
      return;
    }
    fullscreenToolbarTimerRef.current = window.setTimeout(() => {
      fullscreenToolbarTimerRef.current = null;
      setFullscreenToolbarVisible(false);
    }, 2_200);
  }, [deviceFullscreen, deviceViewPreferences.fullscreenToolbarAutoHide, displayScaleOpen, fullscreenOverflowOpen, fullscreenToolbarFocused, fullscreenToolbarHovered, textInputOpen]);

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
      void request("/api/devices/refresh", { method: "PUT" });
    }
    if (deviceEvent?.kind === "lock_state_changed") releaseAllControls();
  }, [deviceEvent, releaseAllControls, request]);

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
    resetProfile(loaded);
    setSelectedId(loaded.mappings[0]?.id ?? null);
  }, [readProfile, resetProfile]);

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
      void showErrorMessage(translateRef.current("profile.switchFailed", { error: String(error) }));
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
    resetProfile(mergeBinding(profile));
    setControlProfile(mergeBinding);
  }, [activeProfile, appBindingConflicts, appProfileBindings, profile, readProfile, refreshProfiles, resetProfile, writeProfile]);

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
        resetProfile(initialProfile);
        setControlProfile(initialProfile);
        return;
      }
      const selected = list.profiles.includes(list.active) ? list.active : list.profiles[0];
      const loaded = await readProfile(selected);
      resetProfile(loaded);
      setControlProfile(loaded);
      setSelectedId(loaded.mappings[0]?.id ?? null);
    };
    void initializeProfiles().catch((error) => showErrorMessage(error));
  }, [backend, readProfile, refreshProfiles, request, resetProfile, writeProfile]);

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
  const moveMapping = (id: string, x: number, y: number) => updateProfile((current) => ({ ...current, mappings: current.mappings.map((mapping) => mapping.id === id ? ("position" in mapping ? { ...mapping, position: { x, y } } : { ...mapping, x, y }) as Mapping : mapping) }), { mergeKey: `move:${id}` });
  const addMapping = (type: ScrcpyMappingType, position: Position = { x: 0.5, y: 0.5 }) => {
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
      void showErrorMessage(error);
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
    resetProfile(controlProfile);
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
    setFullscreenOverflowOpen(false);
    setDeviceFullscreen((active) => !active);
    setPage("device");
  };
  const connectDevice = async (deviceId: string) => {
    selectedDeviceIntentRef.current = deviceId;
    setSelectedDeviceId(deviceId);
    releaseAllControls();
    try {
      const response = await request(`/api/devices/${encodeURIComponent(deviceId)}/connect`, { method: "PUT" });
      if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
    } catch (error) {
      void showErrorMessage(t("errors.reconnectDevice", { error: String(error) }));
    }
  };
  const reconnectDevice = async () => {
    if (!selectedDeviceId) return false;
    releaseAllControls();
    try {
      const response = await request(`/api/devices/${encodeURIComponent(selectedDeviceId)}/reconnect`, { method: "PUT" });
      if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
      return true;
    } catch (error) {
      void showErrorMessage(t("errors.reconnectDevice", { error: String(error) }));
      return false;
    }
  };
  const selectDevice = async (deviceId: string) => {
    selectedDeviceIntentRef.current = deviceId;
    setSelectedDeviceId(deviceId);
    const device = status.devices.find((candidate) => candidate.id === deviceId);
    if (device?.pairing === "unpaired") return;
    await connectDevice(deviceId);
  };
  const pairSelectedDevice = async () => {
    if (!selectedDeviceId || pairingDeviceId) return;
    const device = status.devices.find((candidate) => candidate.id === selectedDeviceId);
    if (!device || device.connection !== "USB" || device.pairing !== "unpaired") return;
    const messageKey = "device-pairing";
    setPairingDeviceId(selectedDeviceId);
    void message.loading({ key: messageKey, content: t("device.pairingWaiting"), duration: 0 });
    try {
      const response = await request(`/api/devices/${encodeURIComponent(selectedDeviceId)}/pair`, { method: "PUT" });
      if (!response.ok) throw new Error(await response.text() || `${response.status} ${response.statusText}`);
      const result = await response.json() as PairDeviceResult;
      if (result.outcome === "paired") {
        void message.success({ key: messageKey, content: t("device.pairingSucceeded") });
      } else {
        const key = result.outcome === "denied"
          ? "device.pairingDenied"
          : result.outcome === "locked"
            ? "device.pairingLocked"
            : result.outcome === "timed_out"
              ? "device.pairingTimedOut"
              : "device.pairingFailed";
        void showErrorMessage(t(key, { error: result.error ?? t("device.pairingUnknownError") }), { key: messageKey });
      }
    } catch (error) {
      void showErrorMessage(t("device.pairingFailed", { error: String(error) }), { key: messageKey });
    } finally {
      setPairingDeviceId(null);
    }
  };
  const pasteTextToDevice = async () => {
    if (!textInput || textInputBusy) return;
    setTextInputBusy(true);
    try {
      const response = await request("/api/device/text/paste", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ text: textInput }),
      });
      if (!response.ok) throw new Error((await response.text()) || `${response.status} ${response.statusText}`);
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
    if (page === "device" && connected && status.active_udid) {
      command({ type: "button", name: "home" });
    }
  };
  const controlOverlayVisible = deviceViewPreferences.controlOverlayVisible;
  const selectedDevice = selectedDeviceId ?? undefined;
  const selectedDeviceInfo = status.devices.find((device) => device.id === selectedDeviceId);
  const selectedDeviceNeedsPairing = selectedDeviceInfo?.connection === "USB" && selectedDeviceInfo.pairing === "unpaired";
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
  const renderHardwareControls = (includeHome: boolean) => (
    <div className="hardware-controls" role="toolbar" aria-label={t("hardware.toolbar")}>
      {hardwareControlEntries.filter(([name]) => includeHome || name !== "home").map(([name, icon]) => {
        const label = t(`hardware.${name}`);
        return (
          <Tooltip key={name} title={`${label}${controlProfile.hardwareBindings[name] ? ` · ${controlProfile.hardwareBindings[name]}` : ""}`}>
            <Button aria-label={label} icon={icon} onClick={() => command({ type: "button", name })} />
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
  const audioControlLabel = audioOutputState === "unavailable"
    ? t("device.deviceAudioOutputUnavailable")
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
      <Tooltip title={audioControlLabel}>
        <Button
          aria-label={audioControlLabel}
          type={deviceAudioEnabled && !audioPlayback.muted ? "primary" : "default"}
          danger={audioOutputState === "unavailable"}
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
          <Select
            className="device-select"
            value={selectedDevice}
            placeholder={t("device.select")}
            options={status.devices.map((device) => ({
              value: device.id,
              label: `${device.name} · ${device.connection}${device.pairing === "unpaired" ? ` · ${t("device.trustRequired")}` : ""}`,
            }))}
            onChange={(deviceId) => void selectDevice(deviceId)}
          />
          {selectedDeviceNeedsPairing && <Tooltip title={t("device.pairDeviceHint")}><Button type="primary" aria-label={t("device.pairDevice")} loading={pairingDeviceId === selectedDeviceId} icon={<SafetyCertificateOutlined />} onClick={() => void pairSelectedDevice()}>{t("device.pairDevice")}</Button></Tooltip>}
          <Tooltip title={t("device.refresh")}><Button aria-label={t("device.refresh")} disabled={!backend} icon={<ReloadOutlined />} onClick={() => void request("/api/devices/refresh", { method: "PUT" })} /></Tooltip>
          <Tooltip title={t("device.reconnect")}><Button aria-label={t("device.reconnect")} disabled={!backend || !selectedDeviceId || selectedDeviceNeedsPairing} icon={<SyncOutlined />} onClick={() => void reconnectDevice()} /></Tooltip>
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
          {(afcVisited || page === "afc") && <AfcPage active={page === "afc"} activeUdid={status.active_udid} request={request} />}
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
            <LocationPage activeUdid={status.active_udid} status={status.location} request={request} />
          ) : page === "performance" ? (
            <PerformancePage
              activeUdid={status.active_udid}
              deviceName={status.devices.find((device) => device.udid === status.active_udid)?.name ?? "iPhone"}
              streamMetrics={streamMetrics}
              renderFps={renderFps}
              view={performanceView}
              error={performanceError}
              request={request}
            />
          ) : page === "logs" ? (
            <DeviceLogsPage activeUdid={status.active_udid} request={request} />
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
                  onImport={importProfile}
                  canUndo={canUndoProfile}
                  canRedo={canRedoProfile}
                  onUndo={undoProfile}
                  onRedo={redoProfile}
                />
              )}
              <main className={`workspace ${deviceFullscreen ? "inspector-hidden" : page === "device" && deviceViewPreferences.deviceInspectorVisible ? "device-workspace" : page === "mappings" && deviceViewPreferences.mappingInspectorVisible ? "mapping-workspace" : "inspector-hidden"}`}>
                <section className="stage-column">
                  {deviceFullscreen ? (
                    <DeviceFullscreenToolbar
                      visible={fullscreenToolbarVisible}
                      canReconnect={Boolean(backend && selectedDeviceId)}
                      controlMode={controlMode}
                      controlOverlayVisible={controlOverlayVisible}
                      rotationControlsLocked={deviceViewPreferences.rotationControlsLocked}
                      overflowOpen={fullscreenOverflowOpen}
                      hardwareDock={deviceViewPreferences.fullscreenHardwareToolbarDock}
                      functionDock={deviceViewPreferences.fullscreenFunctionToolbarDock}
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
                      onRotateLeft={() => command({ type: "rotate", direction: "left" })}
                      onRotateRight={() => command({ type: "rotate", direction: "right" })}
                      onOverflowOpenChange={(open) => {
                        setFullscreenOverflowOpen(open);
                        setFullscreenToolbarVisible(true);
                      }}
                      onDocksChange={(fullscreenHardwareToolbarDock, fullscreenFunctionToolbarDock) => patchDeviceViewPreferences({ fullscreenHardwareToolbarDock, fullscreenFunctionToolbarDock })}
                      onExit={toggleDeviceFullscreen}
                      onPointerEnter={() => setFullscreenToolbarHovered(true)}
                      onPointerLeave={() => setFullscreenToolbarHovered(false)}
                      onFocus={() => setFullscreenToolbarFocused(true)}
                      onBlur={(event) => {
                        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setFullscreenToolbarFocused(false);
                      }}
                    />
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
                            accept: streamMetrics.decoder_accept_ms.toFixed(1),
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
                        items: scrcpyMappingTypes.map((type) => ({ key: type, label: t(`mapping.types.${type}`) })),
                        onClick: ({ key }) => addMapping(key as ScrcpyMappingType, mappingInsertPositionRef.current),
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
                      {directTouches.map((contact) => (
                        <span key={contact.identity} className="direct-touch" style={{ left: `${contact.x * 100}%`, top: `${contact.y * 100}%` }} />
                      ))}
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
                    <Suspense fallback={<WorkspaceLoading inspector />}>
                      <DeviceInspector
                        activeUdid={status.active_udid}
                        activeDeviceId={status.active_device_id}
                        canForgetTrust={status.devices.some((device) => device.id === status.active_device_id && device.connection === "USB" && device.pairing === "paired")}
                        request={request}
                        activeProfile={activeProfile}
                        appProfileBindings={appProfileBindings}
                        bindingConflicts={appBindingConflicts}
                        deviceEvent={deviceEvent}
                        onAppLaunched={(bundleId) => void activateProfileForApp(bundleId)}
                        onAppProfileBindingChange={changeAppProfileBinding}
                      />
                    </Suspense>
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
