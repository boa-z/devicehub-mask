import {
  AppstoreOutlined,
  BugOutlined,
  CheckOutlined,
  CopyOutlined,
  DatabaseOutlined,
  DeleteOutlined,
  DisconnectOutlined,
  DownloadOutlined,
  EditOutlined,
  FileTextOutlined,
  FilterOutlined,
  InfoCircleOutlined,
  LockOutlined,
  MobileOutlined,
  PictureOutlined,
  PoweroffOutlined,
  ReloadOutlined,
  SafetyCertificateOutlined,
  SearchOutlined,
  SortAscendingOutlined,
  SortDescendingOutlined,
  StopOutlined,
  ThunderboltOutlined,
  UploadOutlined,
} from "@ant-design/icons";
import { open, save } from "@tauri-apps/plugin-dialog";
import { Alert, Button, Dropdown, Empty, Input, Modal, Progress, Segmented, Spin, Switch, Tag, Tooltip, Typography, message } from "antd";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { showErrorMessage } from "../errorMessage";
import { AppDocumentsModal } from "./AppDocumentsModal";
import { AppConsoleModal } from "./AppConsoleModal";
import { CrashReportSummaryModal } from "./CrashReportSummaryModal";
import { ErrorAlert, ErrorCopyButton } from "./ErrorPresentation";
import { APP_RENDER_BATCH_SIZE, canTrustProvisioningProfileSigner, deviceAppScopeQuery, filterCrashReports, filterDeviceApps, filterProvisioningProfiles, formatCapacity, formatDeviceRegionalSettings, formatElapsed, formatFileSize, formatProfileDate, formatReportDate, formatStorageUsage, isAppOperationActive, isBackupActive, isDeveloperImageActive, isSysdiagnoseActive, nextAppRenderLimit, normalizeDeviceNameInput, shouldRefreshDeviceInspector, sortDeviceApps } from "../deviceInspector";
import type { DeviceAppSort, DeviceInspectorTab, ProfileStatusFilter } from "../deviceInspector";
import type { AppOperation, CompanionDevice, DeveloperImageMountStatus, DeviceApp, DeviceBackupStatus, DeviceCrashReport, DeviceCrashReportList, DeviceDetails, DeviceEvent, ForgetDeviceResult, HomeScreenLayout, IpaOperation, IpaPreflight, ProvisioningProfile, SysdiagnoseStatus, WdaRunnerStatus } from "../types";
import { useActivePolling } from "../hooks/useActivePolling";
import { DeviceAppRow } from "../features/device-inspector/apps/DeviceAppRow";

type Request = (path: string, init?: RequestInit) => Promise<Response>;
type WallpaperKind = "home" | "lock";
const APP_CATALOG_CACHE_MS = 30_000;

type Props = {
  activeUdid: string | null;
  activeDeviceId: string | null;
  canForgetTrust: boolean;
  request: Request;
  activeProfile: string;
  appProfileBindings: Record<string, string>;
  bindingConflicts: string[];
  deviceEvent: DeviceEvent | null;
  onAppLaunched?: (bundleId: string) => void;
  onAppProfileBindingChange: (bundleId: string, bind: boolean) => Promise<void>;
};

async function readJson<T>(response: Response): Promise<T> {
  if (!response.ok) {
    throw new Error((await response.text()) || `${response.status} ${response.statusText}`);
  }
  return response.json() as Promise<T>;
}

export function DeviceInspector({
  activeUdid,
  activeDeviceId,
  canForgetTrust,
  request,
  activeProfile,
  appProfileBindings,
  bindingConflicts,
  deviceEvent,
  onAppLaunched,
  onAppProfileBindingChange,
}: Props) {
  const { t, i18n } = useTranslation();
  const [tab, setTab] = useState<DeviceInspectorTab>("info");
  const [details, setDetails] = useState<DeviceDetails | null>(null);
  const [companions, setCompanions] = useState<CompanionDevice[]>([]);
  const [companionError, setCompanionError] = useState<string | null>(null);
  const [companionLoading, setCompanionLoading] = useState(false);
  const [apps, setApps] = useState<DeviceApp[]>([]);
  const [appRenderLimit, setAppRenderLimit] = useState(APP_RENDER_BATCH_SIZE);
  const [wdaRunnerStatus, setWdaRunnerStatus] = useState<WdaRunnerStatus | null>(null);
  const [homeScreenLayout, setHomeScreenLayout] = useState<HomeScreenLayout | null>(null);
  const [homeScreenError, setHomeScreenError] = useState<string | null>(null);
  const [homeScreenLoading, setHomeScreenLoading] = useState(false);
  const [profiles, setProfiles] = useState<ProvisioningProfile[]>([]);
  const [crashReports, setCrashReports] = useState<DeviceCrashReport[]>([]);
  const [crashReportsTruncated, setCrashReportsTruncated] = useState(false);
  const [query, setQuery] = useState("");
  const [appSort, setAppSort] = useState<DeviceAppSort>("name");
  const [showSystemApps, setShowSystemApps] = useState(false);
  const [showAppClips, setShowAppClips] = useState(false);
  const [appScopesLoading, setAppScopesLoading] = useState(false);
  const [profileStatus, setProfileStatus] = useState<ProfileStatusFilter>("all");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [appProcessAction, setAppProcessAction] = useState<{ bundleId: string; kind: "launch" | "stop" } | null>(null);
  const [wdaRunnerAction, setWdaRunnerAction] = useState<string | null>(null);
  const [exportingReport, setExportingReport] = useState<string | null>(null);
  const [deletingReport, setDeletingReport] = useState<string | null>(null);
  const [summaryReport, setSummaryReport] = useState<DeviceCrashReport | null>(null);
  const [bindingApp, setBindingApp] = useState<string | null>(null);
  const [appOperation, setAppOperation] = useState<AppOperation | null>(null);
  const [ipaPreflightBusy, setIpaPreflightBusy] = useState(false);
  const [devicePowerAction, setDevicePowerAction] = useState<"restart" | "shutdown" | null>(null);
  const [forgettingTrust, setForgettingTrust] = useState(false);
  const [backupStatus, setBackupStatus] = useState<DeviceBackupStatus | null>(null);
  const [backupFull, setBackupFull] = useState(false);
  const [backupAction, setBackupAction] = useState<"start" | "stop" | null>(null);
  const [sysdiagnoseStatus, setSysdiagnoseStatus] = useState<SysdiagnoseStatus | null>(null);
  const [sysdiagnoseAction, setSysdiagnoseAction] = useState<"start" | "stop" | null>(null);
  const [renameOpen, setRenameOpen] = useState(false);
  const [renameValue, setRenameValue] = useState("");
  const [renameBusy, setRenameBusy] = useState(false);
  const [developerModeBusy, setDeveloperModeBusy] = useState(false);
  const [developerImageStatus, setDeveloperImageStatus] = useState<DeveloperImageMountStatus | null>(null);
  const [developerImageAction, setDeveloperImageAction] = useState<"start" | "stop" | "unmount" | null>(null);
  const [profileMutation, setProfileMutation] = useState<string | null>(null);
  const [documentsApp, setDocumentsApp] = useState<DeviceApp | null>(null);
  const [consoleApp, setConsoleApp] = useState<DeviceApp | null>(null);
  const [wallpaperLoading, setWallpaperLoading] = useState<WallpaperKind | null>(null);
  const [wallpaperPreview, setWallpaperPreview] = useState<{ kind: WallpaperKind; source: string } | null>(null);
  const handledOperation = useRef(0);
  const handledDeviceEvent = useRef(0);
  const handledDeveloperImageState = useRef<string>("");
  const homeScreenRequest = useRef(0);
  const appListRequest = useRef(0);
  const inspectorLoadRequest = useRef(0);
  const appListAbort = useRef<AbortController | null>(null);
  const appCatalogCache = useRef(new Map<string, { loadedAt: number; apps: DeviceApp[] }>());
  const appListContainer = useRef<HTMLDivElement>(null);
  const appListSentinel = useRef<HTMLDivElement>(null);
  const homeScreenLoaded = useRef(false);
  const backupStatusRequest = useRef(0);
  const sysdiagnoseStatusRequest = useRef(0);
  const developerImageStatusRequest = useRef(0);
  const appOperationRequest = useRef(0);
  const appScopesRequest = useRef(0);
  const wallpaperRequest = useRef(0);
  const showSystemAppsRef = useRef(false);
  const showAppClipsRef = useRef(false);

  const loadHomeScreen = useCallback(async () => {
    const requestId = ++homeScreenRequest.current;
    setHomeScreenLoading(true);
    setHomeScreenError(null);
    try {
      const layout = await readJson<HomeScreenLayout>(await request("/api/device/home-screen"));
      if (homeScreenRequest.current === requestId) {
        homeScreenLoaded.current = true;
        setHomeScreenLayout(layout);
      }
    } catch (layoutError) {
      if (homeScreenRequest.current === requestId) {
        setHomeScreenLayout(null);
        setHomeScreenError(String(layoutError));
      }
    } finally {
      if (homeScreenRequest.current === requestId) setHomeScreenLoading(false);
    }
  }, [request]);

  const loadWallpaper = useCallback(async (kind: WallpaperKind) => {
    const requestId = ++wallpaperRequest.current;
    setWallpaperLoading(kind);
    try {
      const response = await request(`/api/device/wallpaper/${kind}`);
      if (!response.ok) {
        throw new Error((await response.text()) || `${response.status} ${response.statusText}`);
      }
      const source = URL.createObjectURL(await response.blob());
      if (wallpaperRequest.current !== requestId) {
        URL.revokeObjectURL(source);
        return;
      }
      setWallpaperPreview({ kind, source });
    } catch (wallpaperError) {
      if (wallpaperRequest.current === requestId) {
        void showErrorMessage(t("deviceInspector.wallpaperLoadFailed", { error: String(wallpaperError) }));
      }
    } finally {
      if (wallpaperRequest.current === requestId) setWallpaperLoading(null);
    }
  }, [request, t]);

  const loadApps = useCallback(async (
    includeSystem = showSystemAppsRef.current,
    includeAppClips = showAppClipsRef.current,
    force = false,
  ) => {
    const cacheKey = `${includeSystem}:${includeAppClips}`;
    const cached = appCatalogCache.current.get(cacheKey);
    if (!force && cached && performance.now() - cached.loadedAt < APP_CATALOG_CACHE_MS) {
      setApps(cached.apps);
      return true;
    }
    appListAbort.current?.abort();
    const controller = new AbortController();
    appListAbort.current = controller;
    const requestId = ++appListRequest.current;
    const suffix = deviceAppScopeQuery(includeSystem, includeAppClips);
    try {
      const nextApps = await readJson<DeviceApp[]>(await request(`/api/device/apps${suffix}`, {
        signal: controller.signal,
      }));
      if (appListRequest.current !== requestId) return false;
      appCatalogCache.current.set(cacheKey, { loadedAt: performance.now(), apps: nextApps });
      setApps(nextApps);
      return true;
    } catch (loadError) {
      if (controller.signal.aborted) return false;
      throw loadError;
    } finally {
      if (appListAbort.current === controller) appListAbort.current = null;
    }
  }, [request]);

  const loadWdaRunnerStatus = useCallback(async () => {
    try {
      setWdaRunnerStatus(await readJson<WdaRunnerStatus>(await request("/api/device/wda-runner")));
    } catch {
      setWdaRunnerStatus(null);
    }
  }, [request]);

  const loadBackupStatus = useCallback(async () => {
    const requestId = ++backupStatusRequest.current;
    const status = await readJson<DeviceBackupStatus>(await request("/api/device/backup"));
    if (backupStatusRequest.current === requestId) setBackupStatus(status);
    return status;
  }, [request]);

  const loadSysdiagnoseStatus = useCallback(async () => {
    const requestId = ++sysdiagnoseStatusRequest.current;
    const status = await readJson<SysdiagnoseStatus>(await request("/api/device/sysdiagnose"));
    if (sysdiagnoseStatusRequest.current === requestId) setSysdiagnoseStatus(status);
    return status;
  }, [request]);

  const loadDeveloperImageStatus = useCallback(async () => {
    const requestId = ++developerImageStatusRequest.current;
    const status = await readJson<DeveloperImageMountStatus>(await request("/api/device/developer-image"));
    if (developerImageStatusRequest.current === requestId) setDeveloperImageStatus(status);
    return status;
  }, [request]);

  const readAppOperation = useCallback(
    async () => readJson<AppOperation>(await request("/api/device/apps/operation")),
    [request],
  );

  const refreshAppOperation = useCallback(async () => {
    const requestId = ++appOperationRequest.current;
    const operation = await readAppOperation();
    if (appOperationRequest.current === requestId) setAppOperation(operation);
    return operation;
  }, [readAppOperation]);

  const load = useCallback(async (force = false) => {
    if (!activeUdid) return;
    const requestId = ++inspectorLoadRequest.current;
    setLoading(true);
    setError(null);
    try {
      if (tab === "info") {
        void Promise.allSettled([
          loadBackupStatus(),
          loadSysdiagnoseStatus(),
          loadDeveloperImageStatus(),
        ]);
        const nextDetails = await readJson<DeviceDetails>(await request("/api/device/details"));
        setDetails(nextDetails);
        setCompanions([]);
        setCompanionError(null);
        if (nextDetails.product_type.startsWith("iPhone")) {
          setCompanionLoading(true);
          try {
            setCompanions(await readJson<CompanionDevice[]>(await request("/api/device/companions")));
          } catch (companionLoadError) {
            setCompanionError(String(companionLoadError));
          } finally {
            setCompanionLoading(false);
          }
        }
      } else if (tab === "apps") {
        const loaded = await loadApps(undefined, undefined, force);
        if (loaded) {
          if (force || !homeScreenLoaded.current) void loadHomeScreen();
          void loadWdaRunnerStatus();
          void refreshAppOperation();
        }
      } else if (tab === "profiles") {
        setProfiles(await readJson<ProvisioningProfile[]>(await request("/api/device/provisioning-profiles")));
      } else if (tab === "crashes") {
        const result = await readJson<DeviceCrashReportList>(await request("/api/device/crash-reports"));
        setCrashReports(result.reports);
        setCrashReportsTruncated(result.truncated);
      }
    } catch (loadError) {
      if (inspectorLoadRequest.current === requestId) setError(String(loadError));
    } finally {
      if (inspectorLoadRequest.current === requestId) setLoading(false);
    }
  }, [activeUdid, loadApps, loadBackupStatus, loadDeveloperImageStatus, loadHomeScreen, loadSysdiagnoseStatus, loadWdaRunnerStatus, refreshAppOperation, request, tab]);

  useEffect(() => {
    inspectorLoadRequest.current += 1;
    homeScreenRequest.current += 1;
    appListRequest.current += 1;
    appListAbort.current?.abort();
    appListAbort.current = null;
    appCatalogCache.current.clear();
    homeScreenLoaded.current = false;
    backupStatusRequest.current += 1;
    sysdiagnoseStatusRequest.current += 1;
    developerImageStatusRequest.current += 1;
    appOperationRequest.current += 1;
    appScopesRequest.current += 1;
    wallpaperRequest.current += 1;
    showSystemAppsRef.current = false;
    showAppClipsRef.current = false;
    setShowSystemApps(false);
    setShowAppClips(false);
    setAppScopesLoading(false);
    setDetails(null);
    setCompanions([]);
    setCompanionError(null);
    setCompanionLoading(false);
    setApps([]);
    setWdaRunnerStatus(null);
    setWdaRunnerAction(null);
    setHomeScreenLayout(null);
    setHomeScreenError(null);
    setHomeScreenLoading(false);
    setProfiles([]);
    setCrashReports([]);
    setCrashReportsTruncated(false);
    setExportingReport(null);
    setDeletingReport(null);
    setSummaryReport(null);
    setAppOperation(null);
    setIpaPreflightBusy(false);
    setProfileMutation(null);
    setDocumentsApp(null);
    setConsoleApp(null);
    setWallpaperLoading(null);
    setWallpaperPreview(null);
    setRenameOpen(false);
    setRenameValue("");
    setRenameBusy(false);
    setDeveloperModeBusy(false);
    setDeveloperImageStatus(null);
    setDeveloperImageAction(null);
    handledDeveloperImageState.current = "";
    setBackupStatus(null);
    setBackupAction(null);
    setSysdiagnoseStatus(null);
    setSysdiagnoseAction(null);
    setError(null);
  }, [activeUdid]);

  useEffect(() => {
    const source = wallpaperPreview?.source;
    return () => {
      if (source) URL.revokeObjectURL(source);
    };
  }, [wallpaperPreview?.source]);

  useEffect(() => {
    void load(false);
  }, [load]);

  useEffect(() => {
    if (tab !== "apps") appListAbort.current?.abort();
  }, [tab]);

  useActivePolling(Boolean(activeUdid) && isBackupActive(backupStatus), loadBackupStatus, 350);
  useActivePolling(Boolean(activeUdid) && isSysdiagnoseActive(sysdiagnoseStatus), loadSysdiagnoseStatus, 350);
  useActivePolling(Boolean(activeUdid) && isDeveloperImageActive(developerImageStatus), loadDeveloperImageStatus, 350);
  useActivePolling(Boolean(activeUdid) && isAppOperationActive(appOperation), refreshAppOperation, 500);

  useEffect(() => {
    if (!developerImageStatus) return;
    const marker = `${developerImageStatus.state}:${developerImageStatus.error ?? ""}`;
    if (handledDeveloperImageState.current === marker) return;
    handledDeveloperImageState.current = marker;
    if (developerImageStatus.state === "mounted") {
      setDetails((current) => current ? { ...current, developer_image_mounted: true } : current);
      void message.success(t("deviceInspector.developerImageMounted"));
    } else if (developerImageStatus.state === "unmounted") {
      setDetails((current) => current ? { ...current, developer_image_mounted: false } : current);
      void message.success(t("deviceInspector.developerImageUnmounted"));
    } else if (developerImageStatus.state === "failed") {
      void showErrorMessage(t("deviceInspector.developerImageMountFailed", { error: developerImageStatus.error ?? "" }));
    }
  }, [developerImageStatus, t]);

  useEffect(() => {
    if (!deviceEvent || deviceEvent.sequence <= handledDeviceEvent.current) return;
    handledDeviceEvent.current = deviceEvent.sequence;
    if (shouldRefreshDeviceInspector(deviceEvent.kind, tab)) void load(true);
  }, [deviceEvent, load, tab]);

  const toggleAppScope = async (scope: "system" | "clips") => {
    if (loading || appScopesLoading) return;
    const nextSystem = scope === "system" ? !showSystemApps : showSystemApps;
    const nextAppClips = scope === "clips" ? !showAppClips : showAppClips;
    const requestId = ++appScopesRequest.current;
    setAppScopesLoading(true);
    try {
      if (await loadApps(nextSystem, nextAppClips)) {
        showSystemAppsRef.current = nextSystem;
        showAppClipsRef.current = nextAppClips;
        setShowSystemApps(nextSystem);
        setShowAppClips(nextAppClips);
      }
    } catch (scopeError) {
      if (appScopesRequest.current !== requestId) return;
      void showErrorMessage(t("deviceInspector.appScopesUnavailable", { error: String(scopeError) }));
    } finally {
      if (appScopesRequest.current === requestId) setAppScopesLoading(false);
    }
  };

  useEffect(() => {
    if (!appOperation || appOperation.id === 0 || appOperation.state === "running" || appOperation.state === "idle") return;
    if (handledOperation.current === appOperation.id) return;
    handledOperation.current = appOperation.id;
    if (appOperation.state === "succeeded") {
      void message.success(t(`deviceInspector.appOperationResult.${appOperation.kind ?? "install"}`));
      if (tab === "apps") void load(true);
    } else if (appOperation.state === "failed") {
      void showErrorMessage(t("deviceInspector.appOperationFailed", { error: appOperation.error ?? "" }));
    } else {
      void message.info(t("deviceInspector.appOperationCancelled"));
    }
  }, [appOperation, load, t, tab]);

  const visibleApps = useMemo(
    () => sortDeviceApps(
      filterDeviceApps(apps, query),
      appSort,
      i18n.resolvedLanguage ?? i18n.language,
    ),
    [appSort, apps, i18n.language, i18n.resolvedLanguage, query],
  );
  const renderedApps = useMemo(
    () => visibleApps.slice(0, appRenderLimit),
    [appRenderLimit, visibleApps],
  );

  useEffect(() => {
    setAppRenderLimit(Math.min(APP_RENDER_BATCH_SIZE, visibleApps.length));
    appListContainer.current?.scrollTo({ top: 0 });
  }, [appSort, apps, query, showAppClips, showSystemApps, visibleApps.length]);

  useEffect(() => {
    const root = appListContainer.current;
    const sentinel = appListSentinel.current;
    if (tab !== "apps" || !root || !sentinel || appRenderLimit >= visibleApps.length) return;
    if (typeof IntersectionObserver === "undefined") {
      setAppRenderLimit(visibleApps.length);
      return;
    }
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) {
        setAppRenderLimit((current) => nextAppRenderLimit(current, visibleApps.length));
      }
    }, { root, rootMargin: "400px 0px" });
    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [appRenderLimit, tab, visibleApps.length]);
  const homeScreenLocations = useMemo(
    () => new Map(homeScreenLayout?.apps.map((location) => [location.bundle_id, location]) ?? []),
    [homeScreenLayout],
  );
  const homeScreenMetricSummary = useMemo(() => {
    const metrics = homeScreenLayout?.metrics;
    if (!metrics) return null;
    const parts: string[] = [];
    if (metrics.columns != null && metrics.rows != null) {
      parts.push(t("deviceInspector.homeScreenGrid", { columns: metrics.columns, rows: metrics.rows }));
    }
    if (metrics.screen_width != null && metrics.screen_height != null) {
      parts.push(t("deviceInspector.homeScreenLayoutSize", { width: metrics.screen_width, height: metrics.screen_height }));
    }
    if (metrics.icon_width != null && metrics.icon_height != null) {
      parts.push(t("deviceInspector.homeScreenIconSize", { width: metrics.icon_width, height: metrics.icon_height }));
    }
    if (metrics.folder_columns != null && metrics.folder_rows != null) {
      parts.push(t("deviceInspector.homeScreenFolderGrid", { columns: metrics.folder_columns, rows: metrics.folder_rows }));
    }
    return parts.length > 0 ? parts.join(" · ") : null;
  }, [homeScreenLayout?.metrics, t]);
  const visibleProfiles = useMemo(
    () => filterProvisioningProfiles(profiles, query, profileStatus),
    [profileStatus, profiles, query],
  );
  const visibleCrashReports = useMemo(
    () => filterCrashReports(crashReports, query),
    [crashReports, query],
  );

  const launch = useCallback(async (app: DeviceApp) => {
    setAppProcessAction({ bundleId: app.bundle_id, kind: "launch" });
    try {
      const response = await request(`/api/device/apps/${encodeURIComponent(app.bundle_id)}/launch`, { method: "PUT" });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      void message.success(t(app.is_running ? "deviceInspector.appRestarted" : "deviceInspector.appLaunched", { name: app.name }));
      onAppLaunched?.(app.bundle_id);
      await loadApps(undefined, undefined, true);
    } catch (launchError) {
      void showErrorMessage(t("deviceInspector.appLaunchFailed", { error: String(launchError) }));
    } finally {
      setAppProcessAction(null);
    }
  }, [loadApps, onAppLaunched, request, t]);

  const stopApp = useCallback(async (app: DeviceApp) => {
    setAppProcessAction({ bundleId: app.bundle_id, kind: "stop" });
    try {
      const response = await request(`/api/device/apps/${encodeURIComponent(app.bundle_id)}/stop`, { method: "PUT" });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      const result = await response.json() as { was_running: boolean };
      void message.success(t(result.was_running ? "deviceInspector.appStopped" : "deviceInspector.appAlreadyStopped", { name: app.name }));
      await loadApps(undefined, undefined, true);
    } catch (stopError) {
      void showErrorMessage(t("deviceInspector.appStopFailed", { error: String(stopError) }));
    } finally {
      setAppProcessAction(null);
    }
  }, [loadApps, request, t]);

  const startWdaRunner = useCallback((app: DeviceApp) => {
    Modal.confirm({
      title: t("deviceInspector.startWdaRunner"),
      content: t("deviceInspector.startWdaRunnerConfirm", { name: app.name, bundleId: app.bundle_id }),
      okText: t("deviceInspector.startWdaRunner"),
      cancelText: t("common.cancel"),
      async onOk() {
        setWdaRunnerAction(app.bundle_id);
        try {
          const response = await request("/api/device/wda-runner", {
            method: "PUT",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ bundle_id: app.bundle_id }),
          });
          const status = await readJson<WdaRunnerStatus>(response);
          setWdaRunnerStatus(status);
          void message.success(t("deviceInspector.wdaRunnerStarted", { name: app.name }));
        } catch (runnerError) {
          await loadWdaRunnerStatus();
          void showErrorMessage(t("deviceInspector.wdaRunnerStartFailed", { error: String(runnerError) }));
          throw runnerError;
        } finally {
          setWdaRunnerAction(null);
        }
      },
    });
  }, [loadWdaRunnerStatus, request, t]);

  const stopWdaRunner = useCallback(async () => {
    const bundleId = wdaRunnerStatus?.runner_bundle_id;
    setWdaRunnerAction(bundleId ?? "stop");
    try {
      const status = await readJson<WdaRunnerStatus>(await request("/api/device/wda-runner", { method: "DELETE" }));
      setWdaRunnerStatus(status);
      void message.success(t("deviceInspector.wdaRunnerStopped"));
    } catch (runnerError) {
      void showErrorMessage(t("deviceInspector.wdaRunnerStopFailed", { error: String(runnerError) }));
    } finally {
      setWdaRunnerAction(null);
    }
  }, [request, t, wdaRunnerStatus?.runner_bundle_id]);

  const startDeviceBackup = async () => {
    try {
      const selected = await open({
        multiple: false,
        directory: true,
        title: t("deviceInspector.backupSelectDestination"),
      });
      if (!selected || Array.isArray(selected)) return;
      setBackupAction("start");
      const response = await request("/api/device/backup", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ destination: selected, full: backupFull }),
      });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      await loadBackupStatus();
      void message.success(t("deviceInspector.backupStarted"));
    } catch (backupError) {
      void showErrorMessage(t("deviceInspector.backupStartFailed", { error: String(backupError) }));
    } finally {
      setBackupAction(null);
    }
  };

  const stopDeviceBackup = async () => {
    setBackupAction("stop");
    try {
      const response = await request("/api/device/backup", { method: "DELETE" });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      await loadBackupStatus();
      void message.info(t("deviceInspector.backupCancelled"));
    } catch (backupError) {
      void showErrorMessage(t("deviceInspector.backupStopFailed", { error: String(backupError) }));
    } finally {
      setBackupAction(null);
    }
  };

  const startSysdiagnose = async () => {
    const selected = await save({
      title: t("deviceInspector.sysdiagnoseSelectDestination"),
      defaultPath: "sysdiagnose.tar.gz",
      filters: [{ name: t("deviceInspector.sysdiagnoseArchive"), extensions: ["gz"] }],
    });
    if (!selected) return;
    Modal.confirm({
      title: t("deviceInspector.sysdiagnoseConfirmTitle"),
      content: t("deviceInspector.sysdiagnoseConfirm"),
      okText: t("deviceInspector.sysdiagnoseStart"),
      okButtonProps: { danger: true },
      cancelText: t("common.cancel"),
      async onOk() {
        setSysdiagnoseAction("start");
        try {
          const response = await request("/api/device/sysdiagnose", {
            method: "PUT",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ destination: selected }),
          });
          if (!response.ok) throw new Error((await response.text()) || response.statusText);
          await loadSysdiagnoseStatus();
          void message.success(t("deviceInspector.sysdiagnoseStarted"));
        } catch (sysdiagnoseError) {
          void showErrorMessage(t("deviceInspector.sysdiagnoseStartFailed", { error: String(sysdiagnoseError) }));
          throw sysdiagnoseError;
        } finally {
          setSysdiagnoseAction(null);
        }
      },
    });
  };

  const stopSysdiagnose = async () => {
    setSysdiagnoseAction("stop");
    try {
      const response = await request("/api/device/sysdiagnose", { method: "DELETE" });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      await loadSysdiagnoseStatus();
      void message.info(t("deviceInspector.sysdiagnoseCancelled"));
    } catch (sysdiagnoseError) {
      void showErrorMessage(t("deviceInspector.sysdiagnoseStopFailed", { error: String(sysdiagnoseError) }));
    } finally {
      setSysdiagnoseAction(null);
    }
  };

  const copyBundleId = useCallback(async (bundleId: string) => {
    await navigator.clipboard.writeText(bundleId);
    void message.success(t("deviceInspector.bundleIdCopied"));
  }, [t]);

  const openAppDocuments = useCallback((app: DeviceApp) => setDocumentsApp(app), []);
  const openAppConsole = useCallback((app: DeviceApp) => setConsoleApp(app), []);

  const copyCompanionIdentifier = async (identifier: string) => {
    await navigator.clipboard.writeText(identifier);
    void message.success(t("deviceInspector.companionIdentifierCopied"));
  };

  const changeAppProfileBinding = useCallback(async (bundleId: string, bind: boolean) => {
    setBindingApp(bundleId);
    try {
      await onAppProfileBindingChange(bundleId, bind);
      void message.success(t(bind ? "deviceInspector.appProfileBound" : "deviceInspector.appProfileUnbound", { profile: activeProfile }));
    } catch (bindingError) {
      void showErrorMessage(t("deviceInspector.appProfileBindingFailed", { error: String(bindingError) }));
    } finally {
      setBindingApp(null);
    }
  }, [activeProfile, onAppProfileBindingChange, t]);

  const installApp = async (operation: IpaOperation) => {
    if (ipaPreflightBusy || appOperation?.state === "running") return;
    try {
      const selected = await open({
        multiple: false,
        directory: false,
        filters: [{ name: t("deviceInspector.ipaFile"), extensions: ["ipa"] }],
      });
      if (!selected || Array.isArray(selected)) return;
      setIpaPreflightBusy(true);
      const preflight = await readJson<IpaPreflight>(await request("/api/device/apps/preflight", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ path: selected, operation }),
      }));
      setIpaPreflightBusy(false);
      const version = [preflight.version, preflight.bundle_version].filter(Boolean).join(" (") + (preflight.version && preflight.bundle_version ? ")" : "");
      const installedVersion = preflight.installed_app
        ? [preflight.installed_app.version, preflight.installed_app.bundle_version].filter(Boolean).join(" (") + (preflight.installed_app.version && preflight.installed_app.bundle_version ? ")" : "")
        : t("deviceInspector.notInstalled");
      const deviceFamilies = preflight.device_families.length === 0
        ? t("deviceInspector.notDeclared")
        : preflight.device_families.map((family) => t(`deviceInspector.ipaDeviceFamilies.${family}`, { defaultValue: `#${family}` })).join(", ");
      const capabilities = preflight.required_capabilities.length === 0
        ? t("deviceInspector.noneRequired")
        : preflight.required_capabilities.join(", ");
      const hasUnknownCompatibility = Object.values(preflight.compatibility).some((value) => value === null)
        || preflight.prohibited_capabilities.length > 0;
      Modal.confirm({
        title: t(operation === "upgrade" ? "deviceInspector.confirmAppUpgrade" : "deviceInspector.confirmAppInstall"),
        width: 560,
        content: (
          <div className="ipa-preflight">
            <div className="ipa-preflight-summary">
              <Typography.Text strong>{preflight.name}</Typography.Text>
              <Typography.Text type="secondary" copyable={{ text: preflight.bundle_id }}>{preflight.bundle_id}</Typography.Text>
            </div>
            <dl className="ipa-preflight-details">
              <dt>{t("deviceInspector.ipaVersion")}</dt><dd>{version || "-"}</dd>
              <dt>{t("deviceInspector.installedVersion")}</dt><dd>{installedVersion || "-"}</dd>
              <dt>{t("deviceInspector.ipaSize")}</dt><dd>{formatFileSize(preflight.file_size_bytes)}</dd>
              <dt>{t("deviceInspector.minimumOs")}</dt><dd>{preflight.minimum_os_version ?? t("deviceInspector.notDeclared")}</dd>
              <dt>{t("deviceInspector.deviceFamilies")}</dt><dd>{deviceFamilies}</dd>
              <dt>{t("deviceInspector.requiredCapabilities")}</dt><dd>{capabilities}</dd>
            </dl>
            {preflight.blocking_issues.map((issue) => (
              <Alert key={issue} type="error" showIcon message={t(`deviceInspector.ipaPreflightIssues.${issue}`)} />
            ))}
            {preflight.operation_allowed && hasUnknownCompatibility && (
              <Alert type="warning" showIcon message={t("deviceInspector.ipaCompatibilityIncomplete")} />
            )}
          </div>
        ),
        okText: t(operation === "upgrade" ? "deviceInspector.upgrade" : "deviceInspector.install"),
        cancelText: t("common.cancel"),
        okButtonProps: { disabled: !preflight.operation_allowed },
        async onOk() {
          const response = await request(`/api/device/apps/${operation}`, {
            method: "PUT",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ path: selected }),
          });
          if (!response.ok) {
            const failure = new Error((await response.text()) || response.statusText);
            void showErrorMessage(t(operation === "upgrade" ? "deviceInspector.appUpgradeFailed" : "deviceInspector.appInstallFailed", { error: String(failure) }));
            throw failure;
          }
          await refreshAppOperation();
        },
      });
    } catch (installError) {
      void showErrorMessage(t(operation === "upgrade" ? "deviceInspector.appUpgradeFailed" : "deviceInspector.appInstallFailed", { error: String(installError) }));
    } finally {
      setIpaPreflightBusy(false);
    }
  };

  const uninstallApp = useCallback((app: DeviceApp) => {
    Modal.confirm({
      title: t("deviceInspector.uninstallApp"),
      content: t("deviceInspector.uninstallConfirm", { name: app.name, bundleId: app.bundle_id }),
      okText: t("deviceInspector.uninstall"),
      cancelText: t("common.cancel"),
      okButtonProps: { danger: true },
      async onOk() {
        const response = await request(`/api/device/apps/${encodeURIComponent(app.bundle_id)}`, { method: "DELETE" });
        if (!response.ok) {
          const failure = new Error((await response.text()) || response.statusText);
          void showErrorMessage(t("deviceInspector.appUninstallFailed", { error: String(failure) }));
          throw failure;
        }
        await refreshAppOperation();
      },
    });
  }, [refreshAppOperation, request, t]);

  const installProvisioningProfile = async () => {
    try {
      const selected = await open({
        multiple: false,
        directory: false,
        filters: [{ name: t("deviceInspector.mobileProvisionFile"), extensions: ["mobileprovision"] }],
      });
      if (!selected || Array.isArray(selected)) return;
      setProfileMutation("install");
      const response = await request("/api/device/provisioning-profiles", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ path: selected }),
      });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      const installed = await response.json() as ProvisioningProfile;
      void message.success(t("deviceInspector.profileInstalled", { name: installed.name }));
      await load();
    } catch (profileError) {
      void showErrorMessage(t("deviceInspector.profileInstallFailed", { error: String(profileError) }));
    } finally {
      setProfileMutation(null);
    }
  };

  const removeProvisioningProfile = (profile: ProvisioningProfile) => {
    if (!profile.removal_supported || profileMutation) return;
    Modal.confirm({
      title: t("deviceInspector.removeProfile"),
      content: t("deviceInspector.removeProfileConfirm", { name: profile.name, uuid: profile.uuid }),
      okText: t("deviceInspector.remove"),
      cancelText: t("common.cancel"),
      okButtonProps: { danger: true },
      async onOk() {
        setProfileMutation(`remove:${profile.uuid}`);
        try {
          const response = await request(`/api/device/provisioning-profiles/${encodeURIComponent(profile.uuid)}`, {
            method: "DELETE",
          });
          if (!response.ok) throw new Error((await response.text()) || response.statusText);
          void message.success(t("deviceInspector.profileRemoved", { name: profile.name }));
          await load();
        } catch (profileError) {
          void showErrorMessage(t("deviceInspector.profileRemoveFailed", { error: String(profileError) }));
          throw profileError;
        } finally {
          setProfileMutation(null);
        }
      },
    });
  };

  const trustProvisioningProfileSigner = (profile: ProvisioningProfile) => {
    if (!canTrustProvisioningProfileSigner(profile) || profileMutation) return;
    Modal.confirm({
      title: t("deviceInspector.trustAppSigner"),
      content: t("deviceInspector.trustAppSignerConfirm", { name: profile.name, uuid: profile.uuid }),
      okText: t("deviceInspector.trustAppSignerAction"),
      cancelText: t("common.cancel"),
      async onOk() {
        setProfileMutation(`trust:${profile.uuid}`);
        try {
          const response = await request(`/api/device/provisioning-profiles/${encodeURIComponent(profile.uuid)}/trust`, {
            method: "PUT",
          });
          if (!response.ok) throw new Error((await response.text()) || response.statusText);
          void message.success(t("deviceInspector.appSignerTrusted", { name: profile.name }));
        } catch (profileError) {
          void showErrorMessage(t("deviceInspector.appSignerTrustFailed", { error: String(profileError) }));
          throw profileError;
        } finally {
          setProfileMutation(null);
        }
      },
    });
  };

  const exportCrashReport = async (report: DeviceCrashReport) => {
    const destination = await save({
      defaultPath: report.name.replaceAll("/", "_").replaceAll("\\", "_"),
      filters: [{ name: t("deviceInspector.crashReportFile"), extensions: ["ips", "crash", "panic", "tailspin", "txt"] }],
    });
    if (!destination) return;
    setExportingReport(report.path);
    try {
      const response = await request("/api/device/crash-reports/export", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ device_path: report.path, destination }),
      });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      const result = await response.json() as { bytes_written: number };
      void message.success(t("deviceInspector.crashReportExported", { size: formatFileSize(result.bytes_written) }));
    } catch (exportError) {
      void showErrorMessage(t("deviceInspector.crashReportExportFailed", { error: String(exportError) }));
    } finally {
      setExportingReport(null);
    }
  };

  const deleteCrashReport = (report: DeviceCrashReport) => {
    if (exportingReport || deletingReport) return;
    Modal.confirm({
      title: t("deviceInspector.deleteCrashReport"),
      content: t("deviceInspector.deleteCrashReportConfirm", { name: report.name }),
      okText: t("common.delete"),
      cancelText: t("common.cancel"),
      okButtonProps: { danger: true },
      async onOk() {
        setDeletingReport(report.path);
        try {
          const response = await request("/api/device/crash-reports", {
            method: "DELETE",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ device_path: report.path }),
          });
          if (!response.ok) throw new Error((await response.text()) || response.statusText);
          setCrashReports((current) => current.filter((candidate) => candidate.path !== report.path));
          void message.success(t("deviceInspector.crashReportDeleted"));
        } catch (deleteError) {
          void showErrorMessage(t("deviceInspector.crashReportDeleteFailed", { error: String(deleteError) }));
          throw deleteError;
        } finally {
          setDeletingReport(null);
        }
      },
    });
  };

  const confirmDevicePowerAction = (action: "restart" | "shutdown") => {
    if (!details || devicePowerAction) return;
    Modal.confirm({
      title: t(`deviceInspector.${action}Device`),
      content: t(`deviceInspector.${action}Confirm`, { name: details.name }),
      okText: t(`deviceInspector.${action}`),
      cancelText: t("common.cancel"),
      okButtonProps: { danger: true },
      async onOk() {
        setDevicePowerAction(action);
        try {
          const response = await request(`/api/device/${action}`, { method: "PUT" });
          if (!response.ok) throw new Error((await response.text()) || response.statusText);
          void message.success(t(`deviceInspector.${action}Requested`));
        } catch (powerError) {
          void showErrorMessage(t("deviceInspector.powerActionFailed", { error: String(powerError) }));
          throw powerError;
        } finally {
          setDevicePowerAction(null);
        }
      },
    });
  };

  const confirmForgetTrust = () => {
    if (!details || !activeDeviceId || !canForgetTrust || forgettingTrust) return;
    Modal.confirm({
      title: t("deviceInspector.forgetTrust"),
      content: t("deviceInspector.forgetTrustConfirm", { name: details.name }),
      okText: t("deviceInspector.forgetTrustAction"),
      cancelText: t("common.cancel"),
      okButtonProps: { danger: true },
      async onOk() {
        setForgettingTrust(true);
        try {
          const response = await request(`/api/devices/${encodeURIComponent(activeDeviceId)}/pair`, { method: "DELETE" });
          if (!response.ok) throw new Error((await response.text()) || response.statusText);
          const result = await response.json() as ForgetDeviceResult;
          if (result.outcome === "forgotten") {
            void message.success(t("deviceInspector.trustForgotten"));
          } else if (result.outcome === "host_record_removed") {
            void message.warning(t("deviceInspector.hostTrustRemoved", { error: result.error ?? t("device.pairingUnknownError") }));
          } else if (result.outcome === "device_forgotten_host_cleanup_failed") {
            void showErrorMessage(t("deviceInspector.hostTrustCleanupFailed", { error: result.error ?? t("device.pairingUnknownError") }));
          } else {
            throw new Error(result.error ?? t("device.pairingUnknownError"));
          }
        } catch (forgetError) {
          void showErrorMessage(t("deviceInspector.forgetTrustFailed", { error: String(forgetError) }));
          throw forgetError;
        } finally {
          setForgettingTrust(false);
        }
      },
    });
  };

  const normalizedDeviceName = normalizeDeviceNameInput(renameValue);
  const prepareDeveloperMode = async () => {
    if (developerModeBusy) return;
    setDeveloperModeBusy(true);
    try {
      const response = await request("/api/device/developer-mode/reveal", { method: "PUT" });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      const result = await response.json() as { already_enabled: boolean };
      if (result.already_enabled) {
        void message.success(t("deviceInspector.developerModeAlreadyEnabled"));
        await load();
      } else {
        void message.success(t("deviceInspector.developerModeRevealed"));
      }
    } catch (prepareError) {
      void showErrorMessage(t("deviceInspector.developerModeRevealFailed", { error: String(prepareError) }));
    } finally {
      setDeveloperModeBusy(false);
    }
  };

  const selectDeveloperImageFile = async (title: string, extensions: string[]) => {
    const selected = await open({
      multiple: false,
      directory: false,
      title,
      filters: [{ name: title, extensions }],
    });
    return selected && !Array.isArray(selected) ? selected : null;
  };

  const startDeveloperImageMount = async () => {
    if (!details || developerImageAction) return;
    const majorVersion = Number.parseInt(details.product_version.split(".")[0] ?? "", 10);
    if (!Number.isFinite(majorVersion)) {
      void showErrorMessage(t("deviceInspector.developerImageVersionInvalid"));
      return;
    }
    const image = await selectDeveloperImageFile(t("deviceInspector.selectDeveloperImage"), ["dmg"]);
    if (!image) return;

    let signature: string | undefined;
    let trustCache: string | undefined;
    let manifest: string | undefined;
    if (majorVersion < 17) {
      signature = await selectDeveloperImageFile(t("deviceInspector.selectDeveloperImageSignature"), ["signature"]) ?? undefined;
      if (!signature) return;
    } else {
      trustCache = await selectDeveloperImageFile(t("deviceInspector.selectDeveloperImageTrustCache"), ["trustcache"]) ?? undefined;
      if (!trustCache) return;
      manifest = await selectDeveloperImageFile(t("deviceInspector.selectDeveloperImageManifest"), ["plist"]) ?? undefined;
      if (!manifest) return;
    }

    Modal.confirm({
      title: t("deviceInspector.mountDeveloperImage"),
      content: t("deviceInspector.mountDeveloperImageConfirm", { version: details.product_version }),
      okText: t("deviceInspector.mountDeveloperImage"),
      cancelText: t("common.cancel"),
      async onOk() {
        setDeveloperImageAction("start");
        try {
          const response = await request("/api/device/developer-image", {
            method: "PUT",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ image, signature, trust_cache: trustCache, manifest }),
          });
          if (!response.ok) throw new Error((await response.text()) || response.statusText);
          await loadDeveloperImageStatus();
          void message.info(t("deviceInspector.developerImageMountStarted"));
        } catch (mountError) {
          void showErrorMessage(t("deviceInspector.developerImageMountFailed", { error: String(mountError) }));
        } finally {
          setDeveloperImageAction(null);
        }
      },
    });
  };

  const stopDeveloperImageMount = async () => {
    if (developerImageAction) return;
    setDeveloperImageAction("stop");
    try {
      const response = await request("/api/device/developer-image", { method: "DELETE" });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      await loadDeveloperImageStatus();
      await load();
      void message.info(t("deviceInspector.developerImageMountCancelled"));
    } catch (mountError) {
      void showErrorMessage(t("deviceInspector.developerImageCancelFailed", { error: String(mountError) }));
    } finally {
      setDeveloperImageAction(null);
    }
  };

  const unmountDeveloperImage = () => {
    if (developerImageAction) return;
    Modal.confirm({
      title: t("deviceInspector.unmountDeveloperImage"),
      content: t("deviceInspector.unmountDeveloperImageConfirm"),
      okText: t("deviceInspector.unmountDeveloperImage"),
      cancelText: t("common.cancel"),
      okButtonProps: { danger: true },
      async onOk() {
        setDeveloperImageAction("unmount");
        try {
          const response = await request("/api/device/developer-image/unmount", { method: "PUT" });
          if (!response.ok) throw new Error((await response.text()) || response.statusText);
          await loadDeveloperImageStatus();
          void message.info(t("deviceInspector.developerImageUnmountStarted"));
        } catch (unmountError) {
          void showErrorMessage(t("deviceInspector.developerImageUnmountFailed", { error: String(unmountError) }));
        } finally {
          setDeveloperImageAction(null);
        }
      },
    });
  };

  const renameDevice = async () => {
    if (!normalizedDeviceName || renameBusy) return;
    setRenameBusy(true);
    try {
      const response = await request("/api/device/name", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ name: normalizedDeviceName }),
      });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      const result = await response.json() as { name: string };
      setDetails((current) => current ? { ...current, name: result.name } : current);
      setRenameOpen(false);
      void message.success(t("deviceInspector.deviceRenamed", { name: result.name }));
    } catch (renameError) {
      void showErrorMessage(t("deviceInspector.deviceRenameFailed", { error: String(renameError) }));
    } finally {
      setRenameBusy(false);
    }
  };

  const appMutationRunning = appOperation?.state === "running";
  const { languageAndLocale, timeZoneAndClock } = formatDeviceRegionalSettings(
    details?.regional_settings ?? null,
    t("deviceInspector.clock12Hour"),
    t("deviceInspector.clock24Hour"),
  );
  const batteryHealth = details?.battery?.health_percent == null
    ? "-"
    : details.battery.full_charge_capacity_mah != null && details.battery.design_capacity_mah != null
      ? `${details.battery.health_percent.toFixed(1)}% (${details.battery.full_charge_capacity_mah}/${details.battery.design_capacity_mah} mAh)`
      : `${details.battery.health_percent.toFixed(1)}%`;

  const infoRows = details ? [
    [t("deviceInspector.os"), `iOS ${details.product_version}${details.build_version ? ` (${details.build_version})` : ""}`],
    [t("deviceInspector.udid"), details.udid],
    [t("deviceInspector.capacity"), formatCapacity(details.total_disk_capacity)],
    [t("deviceInspector.dataStorageUsed"), formatStorageUsage(details.storage?.data_capacity_bytes ?? null, details.storage?.data_available_bytes ?? null)],
    [t("deviceInspector.dataStorageAvailable"), formatCapacity(details.storage?.data_available_bytes ?? null)],
    [t("deviceInspector.deviceClass"), details.device_class ?? "-"],
    [t("deviceInspector.productType"), details.product_type],
    [t("deviceInspector.cpuArchitecture"), details.cpu_architecture ?? "-"],
    [t("deviceInspector.modelNumber"), details.model_number ?? "-"],
    [t("deviceInspector.hardwareModel"), details.hardware_model ?? "-"],
    [t("deviceInspector.deviceColor"), details.device_color ?? "-"],
    [t("deviceInspector.enclosureColor"), details.enclosure_color ?? "-"],
    [t("deviceInspector.languageAndLocale"), languageAndLocale],
    [t("deviceInspector.timeZoneAndClock"), timeZoneAndClock],
    [t("deviceInspector.serialNumber"), details.serial_number ?? "-"],
    [t("deviceInspector.ecid"), details.ecid?.toString() ?? "-"],
    [t("deviceInspector.activationState"), details.activation_state == null
      ? t("deviceInspector.activationStates.unavailable")
      : t(`deviceInspector.activationStates.${details.activation_state}`)],
    [t("deviceInspector.developerMode"), details.developer_mode_enabled == null
      ? t("deviceInspector.developerModeStates.unknown")
      : t(`deviceInspector.developerModeStates.${details.developer_mode_enabled ? "enabled" : "disabled"}`)],
    [t("deviceInspector.batteryLevel"), details.battery?.level_percent == null ? "-" : `${details.battery.level_percent}%`],
    [t("deviceInspector.batteryState"), details.battery?.fully_charged
      ? t("deviceInspector.batteryStates.full")
      : details.battery?.is_charging
        ? t("deviceInspector.batteryStates.charging")
        : details.battery?.external_connected
          ? t("deviceInspector.batteryStates.connected")
          : details.battery ? t("deviceInspector.batteryStates.discharging") : "-"],
    [t("deviceInspector.batteryHealth"), batteryHealth],
    [t("deviceInspector.batteryTemperature"), details.battery?.temperature_celsius == null
      ? "-"
      : `${details.battery.temperature_celsius.toFixed(1)} °C`],
    [t("deviceInspector.batteryCycles"), details.battery?.cycle_count?.toString() ?? "-"],
    [t("deviceInspector.batteryElectrical"), details.battery?.voltage_mv == null && details.battery?.instant_amperage_ma == null
      ? "-"
      : `${details.battery.voltage_mv == null ? "-" : (details.battery.voltage_mv / 1000).toFixed(2)} V · ${details.battery.instant_amperage_ma ?? "-"} mA`],
    [t("deviceInspector.powerAdapter"), details.battery?.adapter_name || details.battery?.adapter_watts != null
      ? [details.battery?.adapter_name, details.battery?.adapter_watts == null ? null : `${details.battery.adapter_watts} W`].filter(Boolean).join(" · ")
      : "-"],
    [t("deviceInspector.batteryTimeRemaining"), details.battery?.time_remaining_minutes == null
      ? "-"
      : t("deviceInspector.minutes", { count: details.battery.time_remaining_minutes })],
  ] : [];
  const backupRunning = isBackupActive(backupStatus);
  const developerImageMountRunning = isDeveloperImageActive(developerImageStatus);
  const backupProgress = backupStatus?.progress_percent
    ?? (backupStatus && backupStatus.bytes_total > 0
      ? Math.min(100, backupStatus.bytes_done * 100 / backupStatus.bytes_total)
      : undefined);
  const backupStatusColor = backupStatus?.state === "completed"
    ? "success"
    : backupStatus?.state === "failed"
      ? "error"
      : backupRunning
        ? "processing"
        : "default";
  const sysdiagnoseRunning = isSysdiagnoseActive(sysdiagnoseStatus);
  const sysdiagnoseProgress = sysdiagnoseStatus?.progress_percent
    ?? (sysdiagnoseStatus && sysdiagnoseStatus.bytes_total > 0
      ? Math.min(100, sysdiagnoseStatus.bytes_written * 100 / sysdiagnoseStatus.bytes_total)
      : undefined);
  const sysdiagnoseStatusColor = sysdiagnoseStatus?.state === "completed"
    ? "success"
    : sysdiagnoseStatus?.state === "failed"
      ? "error"
      : sysdiagnoseRunning
        ? "processing"
        : "default";

  return (
    <>
    <aside className="device-inspector">
      <div className="device-inspector-header">
        <Segmented<DeviceInspectorTab>
          className="device-inspector-tabs"
          block
          value={tab}
          options={[
            { value: "info", label: <Tooltip title={t("deviceInspector.info")}><span aria-label={t("deviceInspector.info")}><InfoCircleOutlined /></span></Tooltip> },
            { value: "apps", label: <Tooltip title={t("deviceInspector.apps")}><span aria-label={t("deviceInspector.apps")}><AppstoreOutlined /></span></Tooltip> },
            { value: "profiles", label: <Tooltip title={t("deviceInspector.profiles")}><span aria-label={t("deviceInspector.profiles")}><SafetyCertificateOutlined /></span></Tooltip> },
            { value: "crashes", label: <Tooltip title={t("deviceInspector.crashes")}><span aria-label={t("deviceInspector.crashes")}><BugOutlined /></span></Tooltip> },
          ]}
          onChange={(next) => {
            setTab(next);
            setQuery("");
          }}
        />
        <Tooltip title={t("deviceInspector.refresh")}>
          <Button
            icon={<ReloadOutlined />}
            loading={loading}
            disabled={!activeUdid}
            onClick={() => void load(true)}
          />
        </Tooltip>
      </div>

      {!activeUdid ? (
        <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={t("deviceInspector.noDevice")} />
      ) : error ? (
        <ErrorAlert title={t("deviceInspector.loadFailed")} error={error} />
      ) : loading && (tab === "info" ? !details : tab === "apps" ? apps.length === 0 : tab === "profiles" ? profiles.length === 0 : crashReports.length === 0) ? (
        <div className="device-inspector-loading"><Spin /></div>
      ) : tab === "info" ? (
        <div className="device-info-pane">
          {details?.activation_state === "unactivated" && (
            <Alert
              type="error"
              showIcon
              message={t("deviceInspector.deviceNotActivated")}
              description={t("deviceInspector.deviceNotActivatedHint")}
            />
          )}
          {details?.developer_mode_enabled === false && (
            <Alert
              type="warning"
              showIcon
              message={t("deviceInspector.developerModeDisabled")}
              description={t("deviceInspector.developerModeHint")}
              action={(
                <Button
                  size="small"
                  loading={developerModeBusy}
                  onClick={() => void prepareDeveloperMode()}
                >
                  {t("deviceInspector.revealDeveloperMode")}
                </Button>
              )}
            />
          )}
          {details?.developer_mode_enabled === true && details.developer_image_mounted === false && (
            <Alert
              type="info"
              showIcon
              message={t("deviceInspector.developerImageMissing")}
              description={t("deviceInspector.developerImageHint")}
            />
          )}
          {developerImageStatus && !["idle", "mounted", "unmounted"].includes(developerImageStatus.state) && (
            <div className="developer-image-progress">
              <div className="developer-image-progress-heading">
                <Typography.Text>{t(`deviceInspector.developerImageMountStates.${developerImageStatus.state}`)}</Typography.Text>
                {developerImageStatus.product_version && (
                  <Typography.Text type="secondary">iOS {developerImageStatus.product_version}</Typography.Text>
                )}
              </div>
              {developerImageMountRunning && (
                <Progress
                  size="small"
                  percent={developerImageStatus.progress_percent ?? undefined}
                  status="active"
                />
              )}
              {developerImageStatus.state === "failed" && developerImageStatus.error && (
                <ErrorAlert title={t("deviceInspector.developerImageMountFailedTitle")} error={developerImageStatus.error} />
              )}
            </div>
          )}
          <div className="device-info-list">
            {details && (
              <div className="device-info-row">
                <Typography.Text>{t("deviceInspector.name")}</Typography.Text>
                <div className="device-info-value-action">
                  <Typography.Text type="secondary" ellipsis={{ tooltip: details.name }}>{details.name}</Typography.Text>
                  <Tooltip title={t("deviceInspector.renameDevice")}>
                    <Button
                      type="text"
                      size="small"
                      aria-label={t("deviceInspector.renameDevice")}
                      icon={<EditOutlined />}
                      onClick={() => {
                        setRenameValue(details.name);
                        setRenameOpen(true);
                      }}
                    />
                  </Tooltip>
                </div>
              </div>
            )}
            {details?.developer_mode_enabled === true && (
              <section className="device-developer-image-section">
                <div>
                  <Typography.Text strong>{t("deviceInspector.developerImage")}</Typography.Text>
                  <Typography.Text type="secondary">
                    {developerImageMountRunning && developerImageStatus
                      ? t(`deviceInspector.developerImageMountStates.${developerImageStatus.state}`)
                      : details.developer_image_mounted == null
                        ? t("deviceInspector.developerImageStates.unknown")
                        : t(`deviceInspector.developerImageStates.${details.developer_image_mounted ? "mounted" : "missing"}`)}
                  </Typography.Text>
                </div>
                {developerImageMountRunning ? (
                  <Button
                    danger
                    icon={<StopOutlined />}
                    loading={developerImageAction === "stop"}
                    disabled={developerImageAction !== null}
                    onClick={() => void stopDeveloperImageMount()}
                  >{t("deviceInspector.cancelDeveloperImageMount")}</Button>
                ) : details.developer_image_mounted ? (
                  <Button
                    danger
                    icon={<DisconnectOutlined />}
                    loading={developerImageAction === "unmount"}
                    disabled={developerImageAction !== null}
                    onClick={unmountDeveloperImage}
                  >{t("deviceInspector.unmountDeveloperImage")}</Button>
                ) : details.developer_image_mounted === false ? (
                  <Button
                    icon={<UploadOutlined />}
                    loading={developerImageAction === "start"}
                    disabled={developerImageAction !== null}
                    onClick={() => void startDeveloperImageMount()}
                  >{t("deviceInspector.mountDeveloperImage")}</Button>
                ) : null}
              </section>
            )}
            {infoRows.map(([label, value]) => (
              <div className="device-info-row" key={label}>
                <Typography.Text>{label}</Typography.Text>
                <Typography.Text type="secondary" ellipsis={{ tooltip: value }}>{value}</Typography.Text>
              </div>
            ))}
            <section className="device-wallpaper-section">
              <div className="device-wallpaper-heading">
                <Typography.Text strong>{t("deviceInspector.wallpaperTitle")}</Typography.Text>
                <Typography.Text type="secondary">{t("deviceInspector.wallpaperHint")}</Typography.Text>
              </div>
              <div className="device-wallpaper-actions">
                <Button
                  icon={<PictureOutlined />}
                  loading={wallpaperLoading === "home"}
                  disabled={wallpaperLoading !== null}
                  onClick={() => void loadWallpaper("home")}
                >
                  {t("deviceInspector.homeWallpaper")}
                </Button>
                <Button
                  icon={<LockOutlined />}
                  loading={wallpaperLoading === "lock"}
                  disabled={wallpaperLoading !== null}
                  onClick={() => void loadWallpaper("lock")}
                >
                  {t("deviceInspector.lockWallpaper")}
                </Button>
              </div>
            </section>
            {details?.product_type.startsWith("iPhone") && <div className="device-companion-section">
              <div className="device-companion-heading">
                <Typography.Text strong>{t("deviceInspector.companions")}</Typography.Text>
                <Typography.Text type="secondary">{t("deviceInspector.companionsHint")}</Typography.Text>
              </div>
              {companionLoading ? (
                <div className="device-companion-loading"><Spin size="small" /></div>
              ) : companionError ? (
                <Alert
                  type="warning"
                  showIcon
                  message={t("deviceInspector.companionsUnavailable")}
                  description={companionError}
                />
              ) : companions.length === 0 ? (
                <Empty
                  image={Empty.PRESENTED_IMAGE_SIMPLE}
                  description={t("deviceInspector.noCompanions")}
                />
              ) : (
                <div className="device-companion-list">
                  {companions.map((companion) => (
                    <div className="device-companion-row" key={companion.identifier}>
                      <MobileOutlined className="device-companion-icon" aria-hidden="true" />
                      <div className="device-companion-meta">
                        <Typography.Text strong ellipsis={{ tooltip: companion.name ?? t("deviceInspector.appleWatch") }}>
                          {companion.name ?? t("deviceInspector.appleWatch")}
                        </Typography.Text>
                        <Typography.Text type="secondary" ellipsis={{ tooltip: companion.identifier }}>
                          {companion.identifier}
                        </Typography.Text>
                        <div>
                          {companion.product_type && <Tag>{companion.product_type}</Tag>}
                          {companion.product_version && (
                            <Tag color="blue">
                              {t("deviceInspector.watchOs", { version: companion.product_version })}
                            </Tag>
                          )}
                          {companion.build_version && <Tag>{companion.build_version}</Tag>}
                        </div>
                      </div>
                      <Tooltip title={t("deviceInspector.copyCompanionIdentifier")}>
                        <Button
                          type="text"
                          size="small"
                          icon={<CopyOutlined />}
                          onClick={() => void copyCompanionIdentifier(companion.identifier)}
                        />
                      </Tooltip>
                    </div>
                  ))}
                </div>
              )}
            </div>}
            <section className="device-backup-section">
              <div className="device-backup-heading">
                <div>
                  <Typography.Text strong>{t("deviceInspector.backupTitle")}</Typography.Text>
                  <Typography.Text type="secondary">{t("deviceInspector.backupHint")}</Typography.Text>
                </div>
                {backupStatus && backupStatus.state !== "idle" && (
                  <Tag color={backupStatusColor}>
                    {t(`deviceInspector.backupStates.${backupStatus.state}`)}
                  </Tag>
                )}
              </div>
              <div className="device-backup-mode">
                <div>
                  <Typography.Text>{t("deviceInspector.backupFull")}</Typography.Text>
                  <Typography.Text type="secondary">{t("deviceInspector.backupFullHint")}</Typography.Text>
                </div>
                <Switch
                  checked={backupFull}
                  disabled={backupRunning}
                  aria-label={t("deviceInspector.backupFull")}
                  onChange={setBackupFull}
                />
              </div>
              {backupStatus && backupStatus.state !== "idle" && (
                <div className="device-backup-progress">
                  <Progress
                    size="small"
                    percent={backupProgress}
                    status={backupStatus.state === "failed" ? "exception" : backupStatus.state === "completed" ? "success" : "active"}
                  />
                  <div className="device-backup-metrics">
                    <span>{t("deviceInspector.backupFiles", { count: backupStatus.files_received })}</span>
                    <span>{backupStatus.bytes_total > 0
                      ? `${formatFileSize(backupStatus.bytes_done)} / ${formatFileSize(backupStatus.bytes_total)}`
                      : formatFileSize(backupStatus.bytes_done)}</span>
                    <span>{formatElapsed(backupStatus.elapsed_ms)}</span>
                  </div>
                  {backupStatus.destination_name && (
                    <Typography.Text type="secondary" ellipsis={{ tooltip: backupStatus.destination_name }}>
                      {t("deviceInspector.backupDestination", { name: backupStatus.destination_name })}
                    </Typography.Text>
                  )}
                </div>
              )}
              {backupStatus?.state === "failed" && backupStatus.error && (
                <ErrorAlert title={t("deviceInspector.backupFailed")} error={backupStatus.error} />
              )}
              <div className="device-backup-actions">
                {backupRunning ? (
                  <Button
                    danger
                    icon={<StopOutlined />}
                    loading={backupAction === "stop"}
                    disabled={backupAction !== null}
                    onClick={() => void stopDeviceBackup()}
                  >{t("deviceInspector.backupCancel")}</Button>
                ) : (
                  <Button
                    type="primary"
                    icon={<DatabaseOutlined />}
                    loading={backupAction === "start"}
                    disabled={backupAction !== null}
                    onClick={() => void startDeviceBackup()}
                  >{t("deviceInspector.backupStart")}</Button>
                )}
              </div>
            </section>
            <section className="device-sysdiagnose-section">
              <div className="device-backup-heading">
                <div>
                  <Typography.Text strong>{t("deviceInspector.sysdiagnoseTitle")}</Typography.Text>
                  <Typography.Text type="secondary">{t("deviceInspector.sysdiagnoseHint")}</Typography.Text>
                </div>
                {sysdiagnoseStatus && sysdiagnoseStatus.state !== "idle" && (
                  <Tag color={sysdiagnoseStatusColor}>
                    {t(`deviceInspector.sysdiagnoseStates.${sysdiagnoseStatus.state}`)}
                  </Tag>
                )}
              </div>
              {sysdiagnoseStatus && sysdiagnoseStatus.state !== "idle" && (
                <div className="device-backup-progress">
                  <Progress
                    size="small"
                    percent={sysdiagnoseProgress}
                    status={sysdiagnoseStatus.state === "failed" ? "exception" : sysdiagnoseStatus.state === "completed" ? "success" : "active"}
                  />
                  <div className="device-backup-metrics">
                    <span>{sysdiagnoseStatus.bytes_total > 0
                      ? `${formatFileSize(sysdiagnoseStatus.bytes_written)} / ${formatFileSize(sysdiagnoseStatus.bytes_total)}`
                      : t("deviceInspector.sysdiagnosePreparing")}</span>
                    <span>{formatElapsed(sysdiagnoseStatus.elapsed_ms)}</span>
                  </div>
                  {sysdiagnoseStatus.destination_name && (
                    <Typography.Text type="secondary" ellipsis={{ tooltip: sysdiagnoseStatus.destination_name }}>
                      {t("deviceInspector.sysdiagnoseDestination", { name: sysdiagnoseStatus.destination_name })}
                    </Typography.Text>
                  )}
                </div>
              )}
              {sysdiagnoseStatus?.state === "failed" && sysdiagnoseStatus.error && (
                <ErrorAlert title={t("deviceInspector.sysdiagnoseFailed")} error={sysdiagnoseStatus.error} />
              )}
              <div className="device-backup-actions">
                {sysdiagnoseRunning ? (
                  <Button
                    danger
                    icon={<StopOutlined />}
                    loading={sysdiagnoseAction === "stop"}
                    disabled={sysdiagnoseAction !== null}
                    onClick={() => void stopSysdiagnose()}
                  >{t("deviceInspector.sysdiagnoseCancel")}</Button>
                ) : (
                  <Button
                    icon={<BugOutlined />}
                    loading={sysdiagnoseAction === "start"}
                    disabled={sysdiagnoseAction !== null}
                    onClick={() => void startSysdiagnose()}
                  >{t("deviceInspector.sysdiagnoseStart")}</Button>
                )}
              </div>
            </section>
          </div>
          <div className="device-power-actions">
            <div>
              <Typography.Text strong>{t("deviceInspector.powerActions")}</Typography.Text>
              <Typography.Text type="secondary">{t("deviceInspector.powerActionsHint")}</Typography.Text>
            </div>
            <Button
              icon={<ReloadOutlined />}
              loading={devicePowerAction === "restart"}
              disabled={devicePowerAction !== null}
              onClick={() => confirmDevicePowerAction("restart")}
            >{t("deviceInspector.restartDevice")}</Button>
            <Button
              danger
              icon={<PoweroffOutlined />}
              loading={devicePowerAction === "shutdown"}
              disabled={devicePowerAction !== null}
              onClick={() => confirmDevicePowerAction("shutdown")}
            >{t("deviceInspector.shutdownDevice")}</Button>
          </div>
          {canForgetTrust && (
            <div className="device-trust-actions">
              <div>
                <Typography.Text strong>{t("deviceInspector.computerTrust")}</Typography.Text>
                <Typography.Text type="secondary">{t("deviceInspector.computerTrustHint")}</Typography.Text>
              </div>
              <Button
                danger
                icon={<DisconnectOutlined />}
                loading={forgettingTrust}
                disabled={!activeDeviceId || forgettingTrust}
                onClick={confirmForgetTrust}
              >{t("deviceInspector.forgetTrust")}</Button>
            </div>
          )}
        </div>
      ) : tab === "apps" ? (
        <div className="device-apps-pane">
          <div className="device-app-toolbar">
            <Input
              allowClear
              value={query}
              prefix={<SearchOutlined />}
              placeholder={t("deviceInspector.searchApps")}
              onChange={(event) => setQuery(event.target.value)}
            />
            <Dropdown
              menu={{
                items: [
                  { key: "name", icon: appSort === "name" ? <CheckOutlined /> : <SortAscendingOutlined />, label: t("deviceInspector.sortAppsByName") },
                  { key: "storage", icon: appSort === "storage" ? <CheckOutlined /> : <DatabaseOutlined />, label: t("deviceInspector.sortAppsByStorage") },
                ],
                onClick: ({ key }) => setAppSort(key as DeviceAppSort),
              }}
            >
              <Tooltip title={t("deviceInspector.sortApps")}>
                <Button
                  aria-label={t("deviceInspector.sortApps")}
                  icon={appSort === "storage" ? <SortDescendingOutlined /> : <SortAscendingOutlined />}
                />
              </Tooltip>
            </Dropdown>
            <Dropdown
              trigger={["click"]}
              menu={{
                selectable: false,
                items: [
                  {
                    key: "system",
                    icon: showSystemApps ? <CheckOutlined /> : <AppstoreOutlined />,
                    label: t("deviceInspector.systemApps"),
                  },
                  {
                    key: "clips",
                    icon: showAppClips ? <CheckOutlined /> : <ThunderboltOutlined />,
                    label: t("deviceInspector.appClips"),
                  },
                ],
                onClick: ({ key }) => void toggleAppScope(key as "system" | "clips"),
              }}
            >
              <Tooltip title={t("deviceInspector.appScopes")}>
                <Button
                  type={showSystemApps || showAppClips ? "primary" : "default"}
                  aria-label={t("deviceInspector.appScopes")}
                  icon={<FilterOutlined />}
                  loading={appScopesLoading}
                  disabled={loading || appScopesLoading}
                />
              </Tooltip>
            </Dropdown>
            <Dropdown
              trigger={["click"]}
              menu={{
                items: [
                  { key: "install", icon: <UploadOutlined />, label: t("deviceInspector.installApp") },
                  { key: "upgrade", icon: <ReloadOutlined />, label: t("deviceInspector.upgradeApp") },
                ],
                onClick: ({ key }) => void installApp(key as IpaOperation),
              }}
            >
              <Tooltip title={t("deviceInspector.installOrUpgradeApp")}>
                <Button aria-label={t("deviceInspector.installOrUpgradeApp")} icon={<UploadOutlined />} loading={ipaPreflightBusy} disabled={appMutationRunning} />
              </Tooltip>
            </Dropdown>
          </div>
          {appOperation && appOperation.id > 0 && appOperation.state !== "idle" && (
            <div className="device-app-operation">
              <div className="device-app-operation-label">
                <Typography.Text ellipsis={{ tooltip: appOperation.label ?? undefined }}>
                  {appOperation.label ?? t("deviceInspector.appOperation")}
                </Typography.Text>
                <Typography.Text type="secondary">
                  {appOperation.stage
                    ? t(`deviceInspector.appOperationStages.${appOperation.stage}`)
                    : t(`deviceInspector.appOperationStates.${appOperation.state}`)}
                </Typography.Text>
              </div>
              {appOperation.state === "running" && appOperation.progress === null ? (
                <Spin size="small" />
              ) : (
                <Progress
                  size="small"
                  percent={appOperation.progress ?? (appOperation.state === "succeeded" ? 100 : 0)}
                  status={appOperation.state === "failed" ? "exception" : appOperation.state === "succeeded" ? "success" : "active"}
                />
              )}
            </div>
          )}
          {homeScreenLoading && (
            <div className="device-home-screen-status">
              <Spin size="small" />
              <Typography.Text type="secondary">{t("deviceInspector.homeScreenLoading")}</Typography.Text>
            </div>
          )}
          {homeScreenError && (
            <Alert
              className="device-home-screen-alert"
              type="warning"
              showIcon
              message={t("deviceInspector.homeScreenUnavailable")}
              description={homeScreenError}
            />
          )}
          {homeScreenLayout?.truncated && (
            <Alert
              className="device-home-screen-alert"
              type="warning"
              showIcon
              message={t("deviceInspector.homeScreenTruncated")}
            />
          )}
          {homeScreenMetricSummary && (
            <div className="device-home-screen-metrics">
              <InfoCircleOutlined aria-hidden="true" />
              <Typography.Text type="secondary">{homeScreenMetricSummary}</Typography.Text>
            </div>
          )}
          <div className="device-app-count">{t("deviceInspector.appCount", { count: visibleApps.length })}</div>
          <div className="device-app-list" ref={appListContainer}>
            {renderedApps.map((app) => (
              <DeviceAppRow
                key={app.bundle_id}
                app={app}
                request={request}
                location={homeScreenLocations.get(app.bundle_id)}
                activeProfile={activeProfile}
                appProfileBindings={appProfileBindings}
                bindingConflicts={bindingConflicts}
                bindingApp={bindingApp}
                appProcessAction={appProcessAction}
                appMutationRunning={appMutationRunning}
                consoleOpen={consoleApp !== null}
                wdaRunnerStatus={wdaRunnerStatus}
                wdaRunnerAction={wdaRunnerAction}
                onChangeProfileBinding={changeAppProfileBinding}
                onCopyBundleId={copyBundleId}
                onOpenDocuments={openAppDocuments}
                onStartWdaRunner={startWdaRunner}
                onStopWdaRunner={stopWdaRunner}
                onOpenConsole={openAppConsole}
                onLaunch={launch}
                onStop={stopApp}
                onUninstall={uninstallApp}
              />
            ))}
            {renderedApps.length < visibleApps.length && <div ref={appListSentinel} className="device-app-list-sentinel" />}
            {visibleApps.length === 0 && <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={t("deviceInspector.noApps")} />}
          </div>
        </div>
      ) : tab === "profiles" ? (
        <div className="device-profiles-pane">
          <div className="device-profile-toolbar">
            <Input
              allowClear
              value={query}
              prefix={<SearchOutlined />}
              placeholder={t("deviceInspector.searchProfiles")}
              onChange={(event) => setQuery(event.target.value)}
            />
            <Tooltip title={t("deviceInspector.installProfile")}>
              <Button
                icon={<UploadOutlined />}
                loading={profileMutation === "install"}
                disabled={profileMutation !== null}
                onClick={() => void installProvisioningProfile()}
              />
            </Tooltip>
          </div>
          <Segmented<ProfileStatusFilter>
            block
            size="small"
            value={profileStatus}
            options={[
              { value: "all", label: t("deviceInspector.profileAll") },
              { value: "valid", label: t("deviceInspector.profileValid") },
              { value: "expired", label: t("deviceInspector.profileExpired") },
              { value: "invalid", label: t("deviceInspector.profileInvalid") },
            ]}
            onChange={setProfileStatus}
          />
          <div className="device-app-count">{t("deviceInspector.profileCount", { count: visibleProfiles.length })}</div>
          <div className="device-profile-list">
            {visibleProfiles.map((profile) => (
              <div className="device-profile-row" key={profile.uuid}>
                <div className="device-profile-title">
                  <Typography.Text strong ellipsis={{ tooltip: profile.name }}>{profile.name}</Typography.Text>
                  {profile.parse_error ? (
                    <Tag color="error">{t("deviceInspector.profileInvalid")}</Tag>
                  ) : profile.is_expired ? (
                    <Tag color="error">{t("deviceInspector.profileExpired")}</Tag>
                  ) : (
                    <Tag color="success">{t("deviceInspector.profileValid")}</Tag>
                  )}
                  {profile.get_task_allow && <Tag color="blue">{t("deviceInspector.profileDevelopment")}</Tag>}
                  {canTrustProvisioningProfileSigner(profile) && (
                    <Tooltip title={t("deviceInspector.trustAppSigner")}>
                      <Button
                        size="small"
                        icon={<SafetyCertificateOutlined />}
                        loading={profileMutation === `trust:${profile.uuid}`}
                        disabled={profileMutation !== null}
                        onClick={() => trustProvisioningProfileSigner(profile)}
                      />
                    </Tooltip>
                  )}
                  {profile.removal_supported && (
                    <Tooltip title={t("deviceInspector.removeProfile")}>
                      <Button
                        danger
                        size="small"
                        icon={<DeleteOutlined />}
                        loading={profileMutation === `remove:${profile.uuid}`}
                        disabled={profileMutation !== null}
                        onClick={() => removeProvisioningProfile(profile)}
                      />
                    </Tooltip>
                  )}
                </div>
                {profile.parse_error ? (
                  <span className="device-profile-error">
                    <Typography.Text type="danger">{profile.parse_error}</Typography.Text>
                    <ErrorCopyButton error={profile.parse_error} />
                  </span>
                ) : (
                  <div className="device-profile-details">
                    <span>{t("deviceInspector.profileAppId")}</span>
                    <Typography.Text type="secondary">{profile.application_identifier ?? "-"}</Typography.Text>
                    <span>{t("deviceInspector.profileUuid")}</span>
                    <Typography.Text type="secondary">{profile.uuid}</Typography.Text>
                    <span>{t("deviceInspector.profileTeam")}</span>
                    <Typography.Text type="secondary">{profile.team_identifiers.join(", ") || "-"}</Typography.Text>
                    <span>{t("deviceInspector.profileCreated")}</span>
                    <Typography.Text type="secondary">{formatProfileDate(profile.creation_date, i18n.resolvedLanguage ?? i18n.language)}</Typography.Text>
                    <span>{t("deviceInspector.profileExpires")}</span>
                    <Typography.Text type="secondary">{formatProfileDate(profile.expiration_date, i18n.resolvedLanguage ?? i18n.language)}</Typography.Text>
                    <span>{t("deviceInspector.profileDevices")}</span>
                    <Typography.Text type="secondary">{profile.provisioned_devices}</Typography.Text>
                  </div>
                )}
              </div>
            ))}
            {visibleProfiles.length === 0 && <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={t("deviceInspector.noProfiles")} />}
          </div>
        </div>
      ) : (
        <div className="device-crashes-pane">
          <Input
            allowClear
            value={query}
            prefix={<SearchOutlined />}
            placeholder={t("deviceInspector.searchCrashReports")}
            onChange={(event) => setQuery(event.target.value)}
          />
          {crashReportsTruncated && (
            <Alert type="warning" showIcon message={t("deviceInspector.crashReportsTruncated")} />
          )}
          <div className="device-app-count">{t("deviceInspector.crashReportCount", { count: visibleCrashReports.length })}</div>
          <div className="device-crash-list">
            {visibleCrashReports.map((report) => (
              <div className="device-crash-row" key={report.path}>
                <FileTextOutlined className="device-crash-icon" aria-hidden="true" />
                <div className="device-crash-meta">
                  <Typography.Text strong ellipsis={{ tooltip: report.name }}>{report.name}</Typography.Text>
                  <Typography.Text type="secondary" ellipsis={{ tooltip: report.path }}>{report.path}</Typography.Text>
                  <div>
                    <Tag>{formatFileSize(report.size_bytes)}</Tag>
                    <Tag>{formatReportDate(report.modified, i18n.resolvedLanguage ?? i18n.language)}</Tag>
                  </div>
                </div>
                <div className="device-crash-actions">
                  <Tooltip title={t("crashSummary.open")}>
                    <Button
                      size="small"
                      icon={<InfoCircleOutlined />}
                      aria-label={t("crashSummary.open")}
                      disabled={exportingReport !== null || deletingReport !== null}
                      onClick={() => setSummaryReport(report)}
                    />
                  </Tooltip>
                  <Tooltip title={t("deviceInspector.exportCrashReport")}>
                    <Button
                      size="small"
                      icon={<DownloadOutlined />}
                      loading={exportingReport === report.path}
                      disabled={exportingReport !== null || deletingReport !== null}
                      onClick={() => void exportCrashReport(report)}
                    />
                  </Tooltip>
                  <Tooltip title={t("deviceInspector.deleteCrashReport")}>
                    <Button
                      danger
                      size="small"
                      icon={<DeleteOutlined />}
                      loading={deletingReport === report.path}
                      disabled={exportingReport !== null || deletingReport !== null}
                      onClick={() => deleteCrashReport(report)}
                    />
                  </Tooltip>
                </div>
              </div>
            ))}
            {visibleCrashReports.length === 0 && <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={t("deviceInspector.noCrashReports")} />}
          </div>
        </div>
      )}
    </aside>
    <CrashReportSummaryModal
      open={summaryReport !== null}
      devicePath={summaryReport?.path ?? null}
      reportName={summaryReport?.name ?? null}
      request={request}
      onClose={() => setSummaryReport(null)}
    />
    <Modal
      title={wallpaperPreview?.kind === "lock"
        ? t("deviceInspector.lockWallpaperPreview")
        : t("deviceInspector.homeWallpaperPreview")}
      open={wallpaperPreview !== null}
      footer={null}
      width={460}
      centered
      destroyOnHidden
      onCancel={() => setWallpaperPreview(null)}
    >
      {wallpaperPreview && (
        <div className="device-wallpaper-preview">
          <img
            src={wallpaperPreview.source}
            alt={wallpaperPreview.kind === "lock"
              ? t("deviceInspector.lockWallpaperPreview")
              : t("deviceInspector.homeWallpaperPreview")}
          />
        </div>
      )}
    </Modal>
    <Modal
      title={t("deviceInspector.renameDevice")}
      open={renameOpen}
      okText={t("deviceInspector.rename")}
      cancelText={t("common.cancel")}
      confirmLoading={renameBusy}
      okButtonProps={{ disabled: !normalizedDeviceName || normalizedDeviceName === details?.name }}
      onOk={() => void renameDevice()}
      onCancel={() => {
        if (!renameBusy) setRenameOpen(false);
      }}
    >
      <Input
        value={renameValue}
        aria-label={t("deviceInspector.deviceName")}
        placeholder={t("deviceInspector.deviceName")}
        disabled={renameBusy}
        onChange={(event) => setRenameValue(event.target.value)}
        onPressEnter={() => {
          if (normalizedDeviceName && normalizedDeviceName !== details?.name) void renameDevice();
        }}
      />
    </Modal>
    <AppConsoleModal app={consoleApp} request={request} onClose={() => setConsoleApp(null)} />
    <AppDocumentsModal app={documentsApp} request={request} onClose={() => setDocumentsApp(null)} />
    </>
  );
}
