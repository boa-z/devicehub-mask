import AppstoreOutlined from "@ant-design/icons/es/icons/AppstoreOutlined";
import BugOutlined from "@ant-design/icons/es/icons/BugOutlined";
import CopyOutlined from "@ant-design/icons/es/icons/CopyOutlined";
import DatabaseOutlined from "@ant-design/icons/es/icons/DatabaseOutlined";
import ClearOutlined from "@ant-design/icons/es/icons/ClearOutlined";
import DisconnectOutlined from "@ant-design/icons/es/icons/DisconnectOutlined";
import DownloadOutlined from "@ant-design/icons/es/icons/DownloadOutlined";
import EditOutlined from "@ant-design/icons/es/icons/EditOutlined";
import FileTextOutlined from "@ant-design/icons/es/icons/FileTextOutlined";
import InfoCircleOutlined from "@ant-design/icons/es/icons/InfoCircleOutlined";
import LockOutlined from "@ant-design/icons/es/icons/LockOutlined";
import MobileOutlined from "@ant-design/icons/es/icons/MobileOutlined";
import PictureOutlined from "@ant-design/icons/es/icons/PictureOutlined";
import PoweroffOutlined from "@ant-design/icons/es/icons/PoweroffOutlined";
import ReloadOutlined from "@ant-design/icons/es/icons/ReloadOutlined";
import SafetyCertificateOutlined from "@ant-design/icons/es/icons/SafetyCertificateOutlined";
import SearchOutlined from "@ant-design/icons/es/icons/SearchOutlined";
import StopOutlined from "@ant-design/icons/es/icons/StopOutlined";
import UploadOutlined from "@ant-design/icons/es/icons/UploadOutlined";
import { open, save } from "@tauri-apps/plugin-dialog";
import { Alert, Button, Empty, Input, Modal, Progress, Segmented, Spin, Switch, Tag, Tooltip, Typography, message } from "antd";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { downloadBrowserResponse } from "../browserFiles";
import { showErrorMessage } from "../errorMessage";
import { runningInDesktopHost } from "../hostApi";
import { readBackendJson } from "../shared/backend/response";
import { AppDocumentsModal } from "./AppDocumentsModal";
import { AppConsoleModal } from "./AppConsoleModal";
import { CrashReportSummaryModal } from "./CrashReportSummaryModal";
import { ErrorAlert, ErrorCopyButton } from "./ErrorPresentation";
import { canTrustProvisioningProfileSigner, filterCrashReports, filterProvisioningProfiles, formatCapacity, formatDeviceRegionalSettings, formatElapsed, formatFileSize, formatProfileDate, formatReportDate, formatStorageUsage, isAppOperationActive, isBackupActive, isDeveloperImageActive, isDeveloperImageDeviceLockedError, isSysdiagnoseActive, normalizeDeviceNameInput, shouldRefreshDeviceInspector } from "../deviceInspector";
import type { DeviceAppSort, DeviceInspectorTab, ProfileStatusFilter } from "../deviceInspector";
import type { AppBindingConflict, AppOperation, AppProfileBinding, CompanionDevice, DeveloperImageMountStatus, DeveloperImageSetDescriptor, DeviceApp, DeviceBackupStatus, DeviceCrashReport, DeviceCrashReportList, DeviceDetails, DeviceEvent, ForgetDeviceResult, HomeScreenLayout, ProfileResolution, ProvisioningProfile, SysdiagnoseStatus, WdaRunnerStatus } from "../types";
import { useActivePolling } from "../hooks/useActivePolling";
import { useLatestRequestOwner } from "../hooks/latestRequest";
import { AppsPane } from "../features/device-inspector/apps/AppsPane";
import { useDeviceAppCatalog } from "../features/device-inspector/apps/useDeviceAppCatalog";

type Request = (path: string, init?: RequestInit) => Promise<Response>;
type WallpaperKind = "home" | "lock";

type Props = {
  activeUdid: string | null;
  activeDeviceId: string | null;
  canForgetTrust: boolean;
  request: Request;
  activeProfile: string;
  appProfileBindings: AppProfileBinding[];
  bindingConflicts: AppBindingConflict[];
  frameSize: ProfileResolution;
  deviceEvent: DeviceEvent | null;
  onAppLaunched?: (bundleId: string) => void;
  onAppProfileBindingChange: (bundleId: string, bind: boolean) => Promise<void>;
};

function developerImageErrorText(error: unknown, t: (key: string) => string): string {
  const value = String(error ?? "");
  return isDeveloperImageDeviceLockedError(value)
    ? t("deviceInspector.developerImageDeviceLocked")
    : value;
}

export function DeviceInspector({
  activeUdid,
  activeDeviceId,
  canForgetTrust,
  request,
  activeProfile,
  appProfileBindings,
  bindingConflicts,
  frameSize,
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
  const [wdaRunnerStatus, setWdaRunnerStatus] = useState<WdaRunnerStatus | null>(null);
  const [homeScreenLayout, setHomeScreenLayout] = useState<HomeScreenLayout | null>(null);
  const [homeScreenError, setHomeScreenError] = useState<string | null>(null);
  const [homeScreenLoading, setHomeScreenLoading] = useState(false);
  const [profiles, setProfiles] = useState<ProvisioningProfile[]>([]);
  const [crashReports, setCrashReports] = useState<DeviceCrashReport[]>([]);
  const [crashReportsTruncated, setCrashReportsTruncated] = useState(false);
  const [query, setQuery] = useState("");
  const [appSort, setAppSort] = useState<DeviceAppSort>("name");
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
  const [developerImages, setDeveloperImages] = useState<DeveloperImageSetDescriptor[]>([]);
  const [selectedDeveloperImageId, setSelectedDeveloperImageId] = useState<string | null>(null);
  const [developerImageAction, setDeveloperImageAction] = useState<"start" | "stop" | "unmount" | "refresh" | "import" | "remove" | null>(null);
  const [profileMutation, setProfileMutation] = useState<string | null>(null);
  const [documentsApp, setDocumentsApp] = useState<DeviceApp | null>(null);
  const [consoleApp, setConsoleApp] = useState<DeviceApp | null>(null);
  const [wallpaperLoading, setWallpaperLoading] = useState<WallpaperKind | null>(null);
  const [wallpaperPreview, setWallpaperPreview] = useState<{ kind: WallpaperKind; source: string } | null>(null);
  const {
    apps,
    showSystemApps,
    showAppClips,
    scopesLoading: appScopesLoading,
    load: loadApps,
    toggleScope: toggleAppCatalogScope,
  } = useDeviceAppCatalog(activeUdid, tab === "apps", request);
  const handledOperation = useRef(0);
  const handledDeviceEvent = useRef(0);
  const handledDeveloperImageState = useRef<string>("");
  const homeScreenRequest = useLatestRequestOwner();
  const inspectorLoadRequest = useLatestRequestOwner();
  const homeScreenLoaded = useRef(false);
  const backupStatusRequest = useLatestRequestOwner();
  const sysdiagnoseStatusRequest = useLatestRequestOwner();
  const developerImageStatusRequest = useLatestRequestOwner();
  const developerImageInput = useRef<HTMLInputElement>(null);
  const appOperationRequest = useLatestRequestOwner();
  const wallpaperRequest = useLatestRequestOwner();
  const wdaRunnerStatusRequest = useLatestRequestOwner();

  const loadHomeScreen = useCallback(async () => {
    const ticket = homeScreenRequest.begin();
    setHomeScreenLoading(true);
    setHomeScreenError(null);
    try {
      const layout = await readBackendJson<HomeScreenLayout>(await request("/api/device/home-screen", {
        signal: ticket.signal,
      }));
      if (ticket.isCurrent()) {
        homeScreenLoaded.current = true;
        setHomeScreenLayout(layout);
      }
    } catch (layoutError) {
      if (ticket.isCurrent()) {
        setHomeScreenLayout(null);
        setHomeScreenError(String(layoutError));
      }
    } finally {
      if (ticket.isCurrent()) setHomeScreenLoading(false);
    }
  }, [homeScreenRequest, request]);

  const loadWallpaper = useCallback(async (kind: WallpaperKind) => {
    const ticket = wallpaperRequest.begin();
    setWallpaperLoading(kind);
    try {
      const response = await request(`/api/device/wallpaper/${kind}`, { signal: ticket.signal });
      if (!response.ok) {
        throw new Error((await response.text()) || `${response.status} ${response.statusText}`);
      }
      const source = URL.createObjectURL(await response.blob());
      if (!ticket.isCurrent()) {
        URL.revokeObjectURL(source);
        return;
      }
      setWallpaperPreview({ kind, source });
    } catch (wallpaperError) {
      if (ticket.isCurrent()) {
        void showErrorMessage(t("deviceInspector.wallpaperLoadFailed", { error: String(wallpaperError) }));
      }
    } finally {
      if (ticket.isCurrent()) setWallpaperLoading(null);
    }
  }, [request, t, wallpaperRequest]);

  const loadWdaRunnerStatus = useCallback(async () => {
    const ticket = wdaRunnerStatusRequest.begin();
    try {
      const status = await readBackendJson<WdaRunnerStatus>(await request("/api/device/wda-runner", {
        signal: ticket.signal,
      }));
      if (ticket.isCurrent()) setWdaRunnerStatus(status);
    } catch {
      if (ticket.isCurrent()) setWdaRunnerStatus(null);
    }
  }, [request, wdaRunnerStatusRequest]);

  const loadBackupStatus = useCallback(async () => {
    const ticket = backupStatusRequest.begin();
    const status = await readBackendJson<DeviceBackupStatus>(await request("/api/device/backup", {
      signal: ticket.signal,
    }));
    if (ticket.isCurrent()) setBackupStatus(status);
    return status;
  }, [backupStatusRequest, request]);

  const loadSysdiagnoseStatus = useCallback(async () => {
    const ticket = sysdiagnoseStatusRequest.begin();
    const status = await readBackendJson<SysdiagnoseStatus>(await request("/api/device/sysdiagnose", {
      signal: ticket.signal,
    }));
    if (ticket.isCurrent()) setSysdiagnoseStatus(status);
    return status;
  }, [request, sysdiagnoseStatusRequest]);

  const loadDeveloperImageStatus = useCallback(async () => {
    const ticket = developerImageStatusRequest.begin();
    const status = await readBackendJson<DeveloperImageMountStatus>(await request("/api/device/developer-image", {
      signal: ticket.signal,
    }));
    if (ticket.isCurrent()) setDeveloperImageStatus(status);
    return status;
  }, [developerImageStatusRequest, request]);

  const loadDeveloperImages = useCallback(async (refresh = false, productVersion = details?.product_version) => {
    const images = await readBackendJson<DeveloperImageSetDescriptor[]>(await request("/api/device/developer-images", {
      method: refresh ? "POST" : "GET",
    }));
    setDeveloperImages(images);
    setSelectedDeveloperImageId((current) => {
      if (current && images.some((image) => image.id === current)) return current;
      const expectedKind = Number.parseInt(productVersion?.split(".")[0] ?? "17", 10) < 17 ? "legacy" : "personalized";
      return images.find((image) => image.kind === expectedKind)?.id ?? images[0]?.id ?? null;
    });
    return images;
  }, [details?.product_version, request]);

  const readAppOperation = useCallback(
    async (signal?: AbortSignal) => readBackendJson<AppOperation>(await request("/api/device/apps/operation", { signal })),
    [request],
  );

  const refreshAppOperation = useCallback(async () => {
    const ticket = appOperationRequest.begin();
    const operation = await readAppOperation(ticket.signal);
    if (ticket.isCurrent()) setAppOperation(operation);
    return operation;
  }, [appOperationRequest, readAppOperation]);

  const load = useCallback(async (force = false) => {
    if (!activeUdid) return;
    const ticket = inspectorLoadRequest.begin();
    setLoading(true);
    setError(null);
    try {
      if (tab === "info") {
        const nextDetails = await readBackendJson<DeviceDetails>(await request("/api/device/details", {
          signal: ticket.signal,
        }));
        if (!ticket.isCurrent()) return;
        setDetails(nextDetails);
        void Promise.allSettled([
          loadBackupStatus(),
          loadSysdiagnoseStatus(),
          loadDeveloperImageStatus(),
          loadDeveloperImages(false, nextDetails.product_version),
        ]);
        setCompanions([]);
        setCompanionError(null);
        if (nextDetails.product_type.startsWith("iPhone")) {
          setCompanionLoading(true);
          try {
            const nextCompanions = await readBackendJson<CompanionDevice[]>(await request("/api/device/companions", {
              signal: ticket.signal,
            }));
            if (ticket.isCurrent()) setCompanions(nextCompanions);
          } catch (companionLoadError) {
            if (ticket.isCurrent()) setCompanionError(String(companionLoadError));
          } finally {
            if (ticket.isCurrent()) setCompanionLoading(false);
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
        const nextProfiles = await readBackendJson<ProvisioningProfile[]>(await request("/api/device/provisioning-profiles", {
          signal: ticket.signal,
        }));
        if (ticket.isCurrent()) setProfiles(nextProfiles);
      } else if (tab === "crashes") {
        const result = await readBackendJson<DeviceCrashReportList>(await request("/api/device/crash-reports", {
          signal: ticket.signal,
        }));
        if (ticket.isCurrent()) {
          setCrashReports(result.reports);
          setCrashReportsTruncated(result.truncated);
        }
      }
    } catch (loadError) {
      if (ticket.isCurrent()) setError(String(loadError));
    } finally {
      if (ticket.isCurrent()) setLoading(false);
    }
  }, [activeUdid, inspectorLoadRequest, loadApps, loadBackupStatus, loadDeveloperImageStatus, loadDeveloperImages, loadHomeScreen, loadSysdiagnoseStatus, loadWdaRunnerStatus, refreshAppOperation, request, tab]);

  useEffect(() => {
    inspectorLoadRequest.cancel();
    homeScreenRequest.cancel();
    homeScreenLoaded.current = false;
    backupStatusRequest.cancel();
    sysdiagnoseStatusRequest.cancel();
    developerImageStatusRequest.cancel();
    appOperationRequest.cancel();
    wallpaperRequest.cancel();
    wdaRunnerStatusRequest.cancel();
    setDetails(null);
    setCompanions([]);
    setCompanionError(null);
    setCompanionLoading(false);
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
    setDeveloperImages([]);
    setSelectedDeveloperImageId(null);
    setDeveloperImageAction(null);
    handledDeveloperImageState.current = "";
    setBackupStatus(null);
    setBackupAction(null);
    setSysdiagnoseStatus(null);
    setSysdiagnoseAction(null);
    setError(null);
  }, [activeUdid, appOperationRequest, backupStatusRequest, developerImageStatusRequest, homeScreenRequest, inspectorLoadRequest, sysdiagnoseStatusRequest, wallpaperRequest, wdaRunnerStatusRequest]);

  useEffect(() => {
    const source = wallpaperPreview?.source;
    return () => {
      if (source) URL.revokeObjectURL(source);
    };
  }, [wallpaperPreview?.source]);

  useEffect(() => {
    void load(false);
  }, [load]);

  useActivePolling(Boolean(activeUdid) && isBackupActive(backupStatus), loadBackupStatus, 350);
  useActivePolling(Boolean(activeUdid) && isSysdiagnoseActive(sysdiagnoseStatus), loadSysdiagnoseStatus, 350);
  useActivePolling(Boolean(activeUdid) && isDeveloperImageActive(developerImageStatus), loadDeveloperImageStatus, 350);
  useActivePolling(Boolean(activeUdid) && isAppOperationActive(appOperation), refreshAppOperation, 500);

  useEffect(() => {
    if (!developerImageStatus) return;
    const marker = `${developerImageStatus.operation ?? "none"}:${developerImageStatus.state}:${developerImageStatus.error ?? ""}`;
    if (handledDeveloperImageState.current === marker) return;
    handledDeveloperImageState.current = marker;
    if (developerImageStatus.state === "mounted") {
      setDetails((current) => current ? { ...current, developer_image_mounted: true } : current);
      void message.success(t("deviceInspector.developerImageMounted"));
    } else if (developerImageStatus.state === "unmounted") {
      setDetails((current) => current ? { ...current, developer_image_mounted: false } : current);
      void message.success(t("deviceInspector.developerImageUnmounted"));
    } else if (developerImageStatus.state === "failed") {
      const error = developerImageErrorText(developerImageStatus.error, t);
      const key = developerImageStatus.operation === "unmount"
        ? "deviceInspector.developerImageUnmountFailed"
        : "deviceInspector.developerImageMountFailed";
      void showErrorMessage(t(key, { error }));
    }
  }, [developerImageStatus, t]);

  useEffect(() => {
    if (!deviceEvent || deviceEvent.sequence <= handledDeviceEvent.current) return;
    handledDeviceEvent.current = deviceEvent.sequence;
    if (shouldRefreshDeviceInspector(deviceEvent.kind, tab)) void load(true);
  }, [deviceEvent, load, tab]);

  const toggleAppScope = useCallback(async (scope: "system" | "clips") => {
    if (loading) return;
    try {
      await toggleAppCatalogScope(scope);
    } catch (scopeError) {
      void showErrorMessage(t("deviceInspector.appScopesUnavailable", { error: String(scopeError) }));
    }
  }, [loading, t, toggleAppCatalogScope]);

  useEffect(() => {
    if (!appOperation || appOperation.id === 0 || appOperation.state === "running" || appOperation.state === "idle") return;
    if (handledOperation.current === appOperation.id) return;
    handledOperation.current = appOperation.id;
    if (appOperation.state === "succeeded") {
      void message.success(t("deviceInspector.appOperationResult.uninstall"));
      if (tab === "apps") void load(true);
    } else if (appOperation.state === "failed") {
      void showErrorMessage(t("deviceInspector.appOperationFailed", { error: appOperation.error ?? "" }));
    } else {
      void message.info(t("deviceInspector.appOperationCancelled"));
    }
  }, [appOperation, load, t, tab]);

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
          const status = await readBackendJson<WdaRunnerStatus>(response);
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
      const status = await readBackendJson<WdaRunnerStatus>(await request("/api/device/wda-runner", { method: "DELETE" }));
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
    const name = report.name.replaceAll("/", "_").replaceAll("\\", "_");
    if (!runningInDesktopHost()) {
      setExportingReport(report.path);
      try {
        const query = new URLSearchParams({ device_path: report.path, name });
        await downloadBrowserResponse(
          await request(`/api/device/crash-reports/browser-export?${query}`),
          name,
        );
        void message.success(t("deviceInspector.crashReportExported", { size: formatFileSize(report.size_bytes) }));
      } catch (exportError) {
        void showErrorMessage(t("deviceInspector.crashReportExportFailed", { error: String(exportError) }));
      } finally {
        setExportingReport(null);
      }
      return;
    }
    const destination = await save({
      defaultPath: name,
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

  const mountSelectedDeveloperImage = useCallback(async () => {
    if (!details || !selectedDeveloperImageId || developerImageAction) return;
    setDeveloperImageAction("start");
    try {
      const response = await request(`/api/device/developer-image/${encodeURIComponent(selectedDeveloperImageId)}`, { method: "PUT" });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      await loadDeveloperImageStatus();
      void message.info(t("deviceInspector.developerImageMountStarted"));
    } catch (mountError) {
      void showErrorMessage(t("deviceInspector.developerImageMountFailed", {
        error: developerImageErrorText(mountError, t),
      }));
    } finally {
      setDeveloperImageAction(null);
    }
  }, [details, developerImageAction, loadDeveloperImageStatus, request, selectedDeveloperImageId, t]);

  const startDeveloperImageMount = () => {
    if (!details || !selectedDeveloperImageId || developerImageAction) return;

    Modal.confirm({
      title: t("deviceInspector.mountDeveloperImage"),
      content: t("deviceInspector.mountDeveloperImageConfirm", { version: details.product_version }),
      okText: t("deviceInspector.mountDeveloperImage"),
      cancelText: t("common.cancel"),
      async onOk() {
        await mountSelectedDeveloperImage();
      },
    });
  };

  const refreshDeveloperImages = async () => {
    if (developerImageAction) return;
    setDeveloperImageAction("refresh");
    try {
      await loadDeveloperImages(true);
    } catch (refreshError) {
      void showErrorMessage(String(refreshError));
    } finally {
      setDeveloperImageAction(null);
    }
  };

  const importDeveloperImage = async (files: FileList | null) => {
    if (!files || files.length === 0 || developerImageAction) return;
    setDeveloperImageAction("import");
    try {
      const form = new FormData();
      for (const file of Array.from(files)) form.append("files", file, file.name);
      const imported = await readBackendJson<DeveloperImageSetDescriptor>(await request("/api/device/developer-images/import", {
        method: "POST",
        body: form,
      }));
      await loadDeveloperImages();
      setSelectedDeveloperImageId(imported.id);
    } catch (importError) {
      void showErrorMessage(String(importError));
    } finally {
      if (developerImageInput.current) developerImageInput.current.value = "";
      setDeveloperImageAction(null);
    }
  };

  const removeDeveloperImage = async () => {
    const selected = developerImages.find((image) => image.id === selectedDeveloperImageId);
    if (!selected?.removable || developerImageAction) return;
    setDeveloperImageAction("remove");
    try {
      const response = await request(`/api/device/developer-images/${encodeURIComponent(selected.id)}`, { method: "DELETE" });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      await loadDeveloperImages();
    } catch (removeError) {
      void showErrorMessage(String(removeError));
    } finally {
      setDeveloperImageAction(null);
    }
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
          void showErrorMessage(t("deviceInspector.developerImageUnmountFailed", {
            error: developerImageErrorText(unmountError, t),
          }));
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
                <ErrorAlert
                  title={t(developerImageStatus.operation === "unmount"
                    ? "deviceInspector.developerImageUnmountFailedTitle"
                    : "deviceInspector.developerImageMountFailedTitle")}
                  error={developerImageErrorText(developerImageStatus.error, t)}
                />
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
                <div className="device-developer-image-heading">
                  <Typography.Text strong>{t("deviceInspector.developerImage")}</Typography.Text>
                  <Typography.Text type="secondary">
                    {developerImageMountRunning && developerImageStatus
                      ? t(`deviceInspector.developerImageMountStates.${developerImageStatus.state}`)
                      : details.developer_image_mounted == null
                        ? t("deviceInspector.developerImageStates.unknown")
                        : t(`deviceInspector.developerImageStates.${details.developer_image_mounted ? "mounted" : "missing"}`)}
                  </Typography.Text>
                </div>
                <input
                  ref={developerImageInput}
                  className="file-input"
                  type="file"
                  multiple
                  accept=".dmg,.signature,.trustcache,.plist"
                  onChange={(event) => void importDeveloperImage(event.currentTarget.files)}
                />
                {details.developer_image_mounted === false && !developerImageMountRunning && (
                  <select
                    className="device-developer-image-select"
                    aria-label={t("deviceInspector.developerImage")}
                    value={selectedDeveloperImageId ?? ""}
                    disabled={developerImages.length === 0}
                    onChange={(event) => setSelectedDeveloperImageId(event.currentTarget.value || null)}
                  >
                    {developerImages.length === 0 && (
                      <option value="">{t("deviceInspector.noDeveloperImages")}</option>
                    )}
                    {developerImages.map((image) => (
                      <option key={image.id} value={image.id}>
                        {image.display_name}
                      </option>
                    ))}
                  </select>
                )}
                <div className="device-developer-image-actions">
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
                    <>
                      <Tooltip title={t("deviceInspector.refresh")}>
                        <Button
                          aria-label={t("deviceInspector.refresh")}
                          icon={<ReloadOutlined />}
                          loading={developerImageAction === "refresh"}
                          disabled={developerImageAction !== null}
                          onClick={() => void refreshDeveloperImages()}
                        />
                      </Tooltip>
                      <Button
                        icon={<UploadOutlined />}
                        loading={developerImageAction === "import"}
                        disabled={developerImageAction !== null}
                        onClick={() => developerImageInput.current?.click()}
                      >{t("deviceInspector.importDeveloperImage")}</Button>
                      {developerImages.find((image) => image.id === selectedDeveloperImageId)?.removable && (
                        <Tooltip title={t("deviceInspector.remove")}>
                          <Button
                            danger
                            aria-label={t("deviceInspector.remove")}
                            icon={<ClearOutlined />}
                            loading={developerImageAction === "remove"}
                            disabled={developerImageAction !== null}
                            onClick={() => void removeDeveloperImage()}
                          />
                        </Tooltip>
                      )}
                      <Button
                        type="primary"
                        icon={<UploadOutlined />}
                        loading={developerImageAction === "start"}
                        disabled={developerImageAction !== null || !selectedDeveloperImageId}
                        onClick={() => void startDeveloperImageMount()}
                      >{t("deviceInspector.mountDeveloperImage")}</Button>
                    </>
                  ) : null}
                </div>
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
        <AppsPane
          apps={apps}
          request={request}
          query={query}
          appSort={appSort}
          showSystemApps={showSystemApps}
          showAppClips={showAppClips}
          loading={loading}
          appScopesLoading={appScopesLoading}
          appOperation={appOperation}
          homeScreenLayout={homeScreenLayout}
          homeScreenLoading={homeScreenLoading}
          homeScreenError={homeScreenError}
          activeProfile={activeProfile}
          appProfileBindings={appProfileBindings}
          bindingConflicts={bindingConflicts}
          frameSize={frameSize}
          bindingApp={bindingApp}
          appProcessAction={appProcessAction}
          appMutationRunning={appMutationRunning}
          consoleOpen={consoleApp !== null}
          wdaRunnerStatus={wdaRunnerStatus}
          wdaRunnerAction={wdaRunnerAction}
          onQueryChange={setQuery}
          onSortChange={setAppSort}
          onToggleScope={toggleAppScope}
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
                          icon={<ClearOutlined />}
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
                        icon={<ClearOutlined />}
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
