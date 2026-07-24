import {
  AppstoreOutlined,
  BugOutlined,
  CopyOutlined,
  DatabaseOutlined,
  DeleteOutlined,
  DisconnectOutlined,
  DownloadOutlined,
  EditOutlined,
  FileTextOutlined,
  FolderOpenOutlined,
  InfoCircleOutlined,
  LinkOutlined,
  LockOutlined,
  MobileOutlined,
  PlayCircleOutlined,
  PoweroffOutlined,
  ReloadOutlined,
  SafetyCertificateOutlined,
  SearchOutlined,
  StopOutlined,
  UploadOutlined,
} from "@ant-design/icons";
import { open, save } from "@tauri-apps/plugin-dialog";
import { Alert, Button, Empty, Input, Modal, Progress, Segmented, Spin, Switch, Tag, Tooltip, Typography, message } from "antd";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { AppDocumentsModal } from "./AppDocumentsModal";
import { DeviceFilesModal } from "./DeviceFilesModal";
import { appProfileBindingState, filterCrashReports, filterDeviceApps, filterProvisioningProfiles, formatCapacity, formatElapsed, formatFileSize, formatProfileDate, formatReportDate, formatStorageUsage, isEligibleWdaRunner, normalizeDeviceNameInput, shouldRefreshDeviceInspector } from "../deviceInspector";
import type { DeviceInspectorTab, ProfileStatusFilter } from "../deviceInspector";
import type { AppOperation, CompanionDevice, DeviceApp, DeviceBackupStatus, DeviceCrashReport, DeviceCrashReportList, DeviceDetails, DeviceEvent, HomeScreenLayout, ProvisioningProfile, WdaRunnerStatus } from "../types";

type Request = (path: string, init?: RequestInit) => Promise<Response>;

type Props = {
  activeUdid: string | null;
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

function DeviceAppIcon({ app, request }: { app: DeviceApp; request: Request }) {
  const container = useRef<HTMLDivElement>(null);
  const [nearViewport, setNearViewport] = useState(false);
  const [source, setSource] = useState<string | null>(null);

  useEffect(() => {
    const element = container.current;
    if (!element || nearViewport) return;
    if (typeof IntersectionObserver === "undefined") {
      setNearViewport(true);
      return;
    }
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          setNearViewport(true);
          observer.disconnect();
        }
      },
      { rootMargin: "160px" },
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, [nearViewport]);

  useEffect(() => {
    if (!nearViewport) return;
    const controller = new AbortController();
    let objectUrl: string | null = null;
    void request(`/api/device/apps/${encodeURIComponent(app.bundle_id)}/icon`, {
      signal: controller.signal,
    }).then(async (response) => {
      if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
      objectUrl = URL.createObjectURL(await response.blob());
      setSource(objectUrl);
    }).catch(() => {
      // An unavailable icon is non-fatal; keep the deterministic fallback.
    });
    return () => {
      controller.abort();
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [app.bundle_id, nearViewport, request]);

  const fallback = Array.from(app.name.trim())[0]?.toLocaleUpperCase() ?? "?";
  return (
    <div ref={container} className="device-app-icon" aria-hidden="true">
      {source ? <img src={source} alt="" draggable={false} /> : fallback}
    </div>
  );
}

export function DeviceInspector({
  activeUdid,
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
  const [wdaRunnerStatus, setWdaRunnerStatus] = useState<WdaRunnerStatus | null>(null);
  const [homeScreenLayout, setHomeScreenLayout] = useState<HomeScreenLayout | null>(null);
  const [homeScreenError, setHomeScreenError] = useState<string | null>(null);
  const [homeScreenLoading, setHomeScreenLoading] = useState(false);
  const [profiles, setProfiles] = useState<ProvisioningProfile[]>([]);
  const [crashReports, setCrashReports] = useState<DeviceCrashReport[]>([]);
  const [crashReportsTruncated, setCrashReportsTruncated] = useState(false);
  const [query, setQuery] = useState("");
  const [profileStatus, setProfileStatus] = useState<ProfileStatusFilter>("all");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [appProcessAction, setAppProcessAction] = useState<{ bundleId: string; kind: "launch" | "stop" } | null>(null);
  const [wdaRunnerAction, setWdaRunnerAction] = useState<string | null>(null);
  const [exportingReport, setExportingReport] = useState<string | null>(null);
  const [bindingApp, setBindingApp] = useState<string | null>(null);
  const [appOperation, setAppOperation] = useState<AppOperation | null>(null);
  const [devicePowerAction, setDevicePowerAction] = useState<"lock" | "restart" | "shutdown" | null>(null);
  const [backupStatus, setBackupStatus] = useState<DeviceBackupStatus | null>(null);
  const [backupFull, setBackupFull] = useState(false);
  const [backupAction, setBackupAction] = useState<"start" | "stop" | null>(null);
  const [renameOpen, setRenameOpen] = useState(false);
  const [renameValue, setRenameValue] = useState("");
  const [renameBusy, setRenameBusy] = useState(false);
  const [developerModeBusy, setDeveloperModeBusy] = useState(false);
  const [profileMutation, setProfileMutation] = useState<string | null>(null);
  const [documentsApp, setDocumentsApp] = useState<DeviceApp | null>(null);
  const [deviceFilesOpen, setDeviceFilesOpen] = useState(false);
  const handledOperation = useRef(0);
  const handledDeviceEvent = useRef(0);
  const homeScreenRequest = useRef(0);

  const loadHomeScreen = useCallback(async () => {
    const requestId = ++homeScreenRequest.current;
    setHomeScreenLoading(true);
    setHomeScreenError(null);
    try {
      const layout = await readJson<HomeScreenLayout>(await request("/api/device/home-screen"));
      if (homeScreenRequest.current === requestId) setHomeScreenLayout(layout);
    } catch (layoutError) {
      if (homeScreenRequest.current === requestId) {
        setHomeScreenLayout(null);
        setHomeScreenError(String(layoutError));
      }
    } finally {
      if (homeScreenRequest.current === requestId) setHomeScreenLoading(false);
    }
  }, [request]);

  const loadApps = useCallback(async () => {
    setApps(await readJson<DeviceApp[]>(await request("/api/device/apps")));
  }, [request]);

  const loadWdaRunnerStatus = useCallback(async () => {
    try {
      setWdaRunnerStatus(await readJson<WdaRunnerStatus>(await request("/api/device/wda-runner")));
    } catch {
      setWdaRunnerStatus(null);
    }
  }, [request]);

  const loadBackupStatus = useCallback(async () => {
    const status = await readJson<DeviceBackupStatus>(await request("/api/device/backup"));
    setBackupStatus(status);
    return status;
  }, [request]);

  const load = useCallback(async () => {
    if (!activeUdid) return;
    setLoading(true);
    setError(null);
    try {
      if (tab === "info") {
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
        await Promise.all([loadApps(), loadHomeScreen(), loadWdaRunnerStatus()]);
      } else if (tab === "profiles") {
        setProfiles(await readJson<ProvisioningProfile[]>(await request("/api/device/provisioning-profiles")));
      } else {
        const result = await readJson<DeviceCrashReportList>(await request("/api/device/crash-reports"));
        setCrashReports(result.reports);
        setCrashReportsTruncated(result.truncated);
      }
    } catch (loadError) {
      setError(String(loadError));
    } finally {
      setLoading(false);
    }
  }, [activeUdid, loadApps, loadHomeScreen, loadWdaRunnerStatus, request, tab]);

  useEffect(() => {
    homeScreenRequest.current += 1;
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
    setAppOperation(null);
    setProfileMutation(null);
    setDocumentsApp(null);
    setRenameOpen(false);
    setRenameValue("");
    setRenameBusy(false);
    setDeveloperModeBusy(false);
    setBackupStatus(null);
    setBackupAction(null);
    setDeviceFilesOpen(false);
    setError(null);
  }, [activeUdid]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    if (!activeUdid) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      let next: DeviceBackupStatus | null = null;
      try {
        next = await readJson<DeviceBackupStatus>(await request("/api/device/backup"));
        if (!cancelled) setBackupStatus(next);
      } catch {
        // The regular inspector request path surfaces connection errors.
      }
      if (!cancelled) {
        const active = next?.state === "starting" || next?.state === "backing_up";
        timer = setTimeout(poll, active ? 350 : 2_000);
      }
    };
    void poll();
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [activeUdid, request]);

  useEffect(() => {
    if (!deviceEvent || deviceEvent.sequence <= handledDeviceEvent.current) return;
    handledDeviceEvent.current = deviceEvent.sequence;
    if (shouldRefreshDeviceInspector(deviceEvent.kind, tab)) void load();
  }, [deviceEvent, load, tab]);

  const readAppOperation = useCallback(
    async () => readJson<AppOperation>(await request("/api/device/apps/operation")),
    [request],
  );

  const refreshAppOperation = useCallback(async () => {
    const operation = await readAppOperation();
    setAppOperation(operation);
    return operation;
  }, [readAppOperation]);

  useEffect(() => {
    if (!activeUdid) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      let operation: AppOperation | null = null;
      try {
        operation = await readAppOperation();
        if (!cancelled) setAppOperation(operation);
      } catch {
        // The regular inspector request path surfaces connection errors.
      }
      if (!cancelled) {
        timer = setTimeout(poll, operation?.state === "running" ? 500 : 2000);
      }
    };
    void poll();
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [activeUdid, readAppOperation]);

  useEffect(() => {
    if (!appOperation || appOperation.id === 0 || appOperation.state === "running" || appOperation.state === "idle") return;
    if (handledOperation.current === appOperation.id) return;
    handledOperation.current = appOperation.id;
    if (appOperation.state === "succeeded") {
      void message.success(t(`deviceInspector.appOperationResult.${appOperation.kind ?? "install"}`));
      if (tab === "apps") void load();
    } else if (appOperation.state === "failed") {
      void message.error(t("deviceInspector.appOperationFailed", { error: appOperation.error ?? "" }));
    } else {
      void message.info(t("deviceInspector.appOperationCancelled"));
    }
  }, [appOperation, load, t, tab]);

  const visibleApps = useMemo(() => filterDeviceApps(apps, query), [apps, query]);
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

  const launch = async (app: DeviceApp) => {
    setAppProcessAction({ bundleId: app.bundle_id, kind: "launch" });
    try {
      const response = await request(`/api/device/apps/${encodeURIComponent(app.bundle_id)}/launch`, { method: "PUT" });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      void message.success(t(app.is_running ? "deviceInspector.appRestarted" : "deviceInspector.appLaunched", { name: app.name }));
      onAppLaunched?.(app.bundle_id);
      await loadApps();
    } catch (launchError) {
      void message.error(t("deviceInspector.appLaunchFailed", { error: String(launchError) }));
    } finally {
      setAppProcessAction(null);
    }
  };

  const stopApp = async (app: DeviceApp) => {
    setAppProcessAction({ bundleId: app.bundle_id, kind: "stop" });
    try {
      const response = await request(`/api/device/apps/${encodeURIComponent(app.bundle_id)}/stop`, { method: "PUT" });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      const result = await response.json() as { was_running: boolean };
      void message.success(t(result.was_running ? "deviceInspector.appStopped" : "deviceInspector.appAlreadyStopped", { name: app.name }));
      await loadApps();
    } catch (stopError) {
      void message.error(t("deviceInspector.appStopFailed", { error: String(stopError) }));
    } finally {
      setAppProcessAction(null);
    }
  };

  const startWdaRunner = (app: DeviceApp) => {
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
          void message.error(t("deviceInspector.wdaRunnerStartFailed", { error: String(runnerError) }));
          throw runnerError;
        } finally {
          setWdaRunnerAction(null);
        }
      },
    });
  };

  const stopWdaRunner = async () => {
    const bundleId = wdaRunnerStatus?.runner_bundle_id;
    setWdaRunnerAction(bundleId ?? "stop");
    try {
      const status = await readJson<WdaRunnerStatus>(await request("/api/device/wda-runner", { method: "DELETE" }));
      setWdaRunnerStatus(status);
      void message.success(t("deviceInspector.wdaRunnerStopped"));
    } catch (runnerError) {
      void message.error(t("deviceInspector.wdaRunnerStopFailed", { error: String(runnerError) }));
    } finally {
      setWdaRunnerAction(null);
    }
  };

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
      void message.error(t("deviceInspector.backupStartFailed", { error: String(backupError) }));
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
      void message.error(t("deviceInspector.backupStopFailed", { error: String(backupError) }));
    } finally {
      setBackupAction(null);
    }
  };

  const copyBundleId = async (bundleId: string) => {
    await navigator.clipboard.writeText(bundleId);
    void message.success(t("deviceInspector.bundleIdCopied"));
  };

  const copyCompanionIdentifier = async (identifier: string) => {
    await navigator.clipboard.writeText(identifier);
    void message.success(t("deviceInspector.companionIdentifierCopied"));
  };

  const changeAppProfileBinding = async (bundleId: string, bind: boolean) => {
    setBindingApp(bundleId);
    try {
      await onAppProfileBindingChange(bundleId, bind);
      void message.success(t(bind ? "deviceInspector.appProfileBound" : "deviceInspector.appProfileUnbound", { profile: activeProfile }));
    } catch (bindingError) {
      void message.error(t("deviceInspector.appProfileBindingFailed", { error: String(bindingError) }));
    } finally {
      setBindingApp(null);
    }
  };

  const installApp = async () => {
    try {
      const selected = await open({
        multiple: false,
        directory: false,
        filters: [{ name: t("deviceInspector.ipaFile"), extensions: ["ipa"] }],
      });
      if (!selected || Array.isArray(selected)) return;
      const response = await request("/api/device/apps/install", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ path: selected }),
      });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      await refreshAppOperation();
    } catch (installError) {
      void message.error(t("deviceInspector.appInstallFailed", { error: String(installError) }));
    }
  };

  const uninstallApp = (app: DeviceApp) => {
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
          void message.error(t("deviceInspector.appUninstallFailed", { error: String(failure) }));
          throw failure;
        }
        await refreshAppOperation();
      },
    });
  };

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
      void message.error(t("deviceInspector.profileInstallFailed", { error: String(profileError) }));
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
        setProfileMutation(profile.uuid);
        try {
          const response = await request(`/api/device/provisioning-profiles/${encodeURIComponent(profile.uuid)}`, {
            method: "DELETE",
          });
          if (!response.ok) throw new Error((await response.text()) || response.statusText);
          void message.success(t("deviceInspector.profileRemoved", { name: profile.name }));
          await load();
        } catch (profileError) {
          void message.error(t("deviceInspector.profileRemoveFailed", { error: String(profileError) }));
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
      void message.error(t("deviceInspector.crashReportExportFailed", { error: String(exportError) }));
    } finally {
      setExportingReport(null);
    }
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
          void message.error(t("deviceInspector.powerActionFailed", { error: String(powerError) }));
          throw powerError;
        } finally {
          setDevicePowerAction(null);
        }
      },
    });
  };

  const lockDevice = async () => {
    if (devicePowerAction) return;
    setDevicePowerAction("lock");
    try {
      const response = await request("/api/device/lock", { method: "PUT" });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      void message.success(t("deviceInspector.lockRequested"));
    } catch (powerError) {
      void message.error(t("deviceInspector.powerActionFailed", { error: String(powerError) }));
    } finally {
      setDevicePowerAction(null);
    }
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
      void message.error(t("deviceInspector.developerModeRevealFailed", { error: String(prepareError) }));
    } finally {
      setDeveloperModeBusy(false);
    }
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
      void message.error(t("deviceInspector.deviceRenameFailed", { error: String(renameError) }));
    } finally {
      setRenameBusy(false);
    }
  };

  const appMutationRunning = appOperation?.state === "running";

  const infoRows = details ? [
    [t("deviceInspector.os"), `iOS ${details.product_version}${details.build_version ? ` (${details.build_version})` : ""}`],
    [t("deviceInspector.udid"), details.udid],
    [t("deviceInspector.capacity"), formatCapacity(details.total_disk_capacity)],
    [t("deviceInspector.dataStorageUsed"), formatStorageUsage(details.storage?.data_capacity_bytes ?? null, details.storage?.data_available_bytes ?? null)],
    [t("deviceInspector.dataStorageAvailable"), formatCapacity(details.storage?.data_available_bytes ?? null)],
    [t("deviceInspector.productType"), details.product_type],
    [t("deviceInspector.hardwareModel"), details.hardware_model ?? "-"],
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
    [t("deviceInspector.batteryHealth"), details.battery?.health_percent == null
      ? "-"
      : `${details.battery.health_percent.toFixed(1)}% (${details.battery.full_charge_capacity_mah ?? "-"}/${details.battery.design_capacity_mah ?? "-"} mAh)`],
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
  const backupRunning = backupStatus?.state === "starting" || backupStatus?.state === "backing_up";
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

  return (
    <>
    <aside className="device-inspector">
      <div className="device-inspector-header">
        <Segmented<DeviceInspectorTab>
          block
          value={tab}
          options={[
            { value: "info", label: t("deviceInspector.info"), icon: <InfoCircleOutlined /> },
            { value: "apps", label: t("deviceInspector.apps"), icon: <AppstoreOutlined /> },
            { value: "profiles", label: t("deviceInspector.profiles"), icon: <SafetyCertificateOutlined /> },
            { value: "crashes", label: t("deviceInspector.crashes"), icon: <BugOutlined /> },
          ]}
          onChange={(next) => {
            setTab(next);
            setQuery("");
          }}
        />
        <Tooltip title={t("deviceInspector.refresh")}>
          <Button icon={<ReloadOutlined />} loading={loading} disabled={!activeUdid} onClick={() => void load()} />
        </Tooltip>
      </div>

      {!activeUdid ? (
        <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={t("deviceInspector.noDevice")} />
      ) : error ? (
        <Alert type="error" showIcon message={t("deviceInspector.loadFailed")} description={error} />
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
            {infoRows.map(([label, value]) => (
              <div className="device-info-row" key={label}>
                <Typography.Text>{label}</Typography.Text>
                <Typography.Text type="secondary" ellipsis={{ tooltip: value }}>{value}</Typography.Text>
              </div>
            ))}
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
            <section className="device-files-section">
              <div>
                <Typography.Text strong>{t("deviceInspector.deviceFilesTitle")}</Typography.Text>
                <Typography.Text type="secondary">{t("deviceInspector.deviceFilesHint")}</Typography.Text>
              </div>
              <Button icon={<FolderOpenOutlined />} onClick={() => setDeviceFilesOpen(true)}>
                {t("deviceInspector.browseDeviceFiles")}
              </Button>
            </section>
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
                <Alert type="error" showIcon message={t("deviceInspector.backupFailed")} description={backupStatus.error} />
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
          </div>
          <div className="device-power-actions">
            <div>
              <Typography.Text strong>{t("deviceInspector.powerActions")}</Typography.Text>
              <Typography.Text type="secondary">{t("deviceInspector.powerActionsHint")}</Typography.Text>
            </div>
            <Button
              className="device-lock-action"
              icon={<LockOutlined />}
              loading={devicePowerAction === "lock"}
              disabled={devicePowerAction !== null}
              onClick={() => void lockDevice()}
            >{t("deviceInspector.lockDevice")}</Button>
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
            <Tooltip title={t("deviceInspector.installApp")}>
              <Button icon={<UploadOutlined />} disabled={appMutationRunning} onClick={() => void installApp()} />
            </Tooltip>
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
          <div className="device-app-list">
            {visibleApps.map((app) => {
              const location = homeScreenLocations.get(app.bundle_id);
              const folder = location?.folders.at(-1);
              const locationLabel = folder
                ? t("deviceInspector.homeScreenFolder", { name: folder.name ?? t("deviceInspector.homeScreenUnnamedFolder") })
                : location?.container === "dock"
                  ? t("deviceInspector.homeScreenDock")
                  : location?.page
                    ? t("deviceInspector.homeScreenPage", { page: location.page })
                    : null;
              const rootPosition = location
                ? t("deviceInspector.homeScreenPosition", {
                    page: location.page ?? t("deviceInspector.homeScreenDock"),
                    position: location.position,
                  })
                : null;
              const folderRoute = location?.folders.map((step) => t("deviceInspector.homeScreenFolderStep", {
                name: step.name ?? t("deviceInspector.homeScreenUnnamedFolder"),
                page: step.page,
                position: step.position,
              })) ?? [];
              const locationTooltip = location
                ? [rootPosition, ...folderRoute].join(" > ")
                : undefined;
              const bindingState = appProfileBindingState(app.bundle_id, activeProfile, appProfileBindings, bindingConflicts);
              const boundProfile = appProfileBindings[app.bundle_id];
              const eligibleWdaRunner = isEligibleWdaRunner(app);
              const activeWdaRunner = wdaRunnerStatus?.runner_bundle_id === app.bundle_id;
              const bindingTooltip = bindingState === "conflict"
                ? t("deviceInspector.appProfileConflict")
                : bindingState === "other"
                  ? t("deviceInspector.appProfileBoundOther", { profile: boundProfile })
                  : t(bindingState === "active" ? "deviceInspector.unbindAppProfile" : "deviceInspector.bindAppProfile", { profile: activeProfile });
              return <div className="device-app-row" key={app.bundle_id}>
                <DeviceAppIcon app={app} request={request} />
                <div className="device-app-meta">
                  <Typography.Text strong ellipsis={{ tooltip: app.name }}>{app.name}</Typography.Text>
                  <Typography.Text type="secondary" ellipsis={{ tooltip: app.bundle_id }}>{app.bundle_id}</Typography.Text>
                  <div className="device-app-tags">
                    {app.version && <Tag>{app.version}</Tag>}
                    {locationLabel && <Tooltip title={locationTooltip}><Tag color="cyan">{locationLabel}</Tag></Tooltip>}
                    {app.is_running === true && <Tag color="success">{t("deviceInspector.runningApp")}</Tag>}
                    {app.is_developer_app && <Tag color="blue">{t("deviceInspector.developerApp")}</Tag>}
                    {activeWdaRunner && wdaRunnerStatus?.phase === "starting" && <Tag color="processing">{t("deviceInspector.wdaRunnerStarting")}</Tag>}
                    {activeWdaRunner && wdaRunnerStatus?.phase === "running" && <Tag color="success">{t("deviceInspector.wdaRunnerRunning")}</Tag>}
                    {activeWdaRunner && wdaRunnerStatus?.phase === "failed" && <Tooltip title={wdaRunnerStatus.last_error}><Tag color="error">{t("deviceInspector.wdaRunnerFailed")}</Tag></Tooltip>}
                    {bindingState === "conflict"
                      ? <Tag color="error">{t("deviceInspector.appProfileConflictTag")}</Tag>
                      : boundProfile && <Tag color={bindingState === "active" ? "success" : "default"}>{t("deviceInspector.appProfileTag", { profile: boundProfile })}</Tag>}
                  </div>
                </div>
                <div className="device-app-actions">
                  <Tooltip title={bindingTooltip}>
                    <Button
                      size="small"
                      type={bindingState === "active" ? "primary" : "default"}
                      icon={bindingState === "active" ? <DisconnectOutlined /> : <LinkOutlined />}
                      loading={bindingApp === app.bundle_id}
                      disabled={bindingState === "conflict" || bindingState === "other"}
                      onClick={() => void changeAppProfileBinding(app.bundle_id, bindingState !== "active")}
                    />
                  </Tooltip>
                  <Tooltip title={t("deviceInspector.copyBundleId")}>
                    <Button size="small" icon={<CopyOutlined />} onClick={() => void copyBundleId(app.bundle_id)} />
                  </Tooltip>
                  {app.documents_available && (
                    <Tooltip title={t("deviceInspector.appDocuments")}>
                      <Button size="small" icon={<FolderOpenOutlined />} onClick={() => setDocumentsApp(app)} />
                    </Tooltip>
                  )}
                  {eligibleWdaRunner && (
                    <Tooltip title={t(activeWdaRunner && wdaRunnerStatus?.managed ? "deviceInspector.stopWdaRunner" : "deviceInspector.startWdaRunner")}>
                      <Button
                        size="small"
                        danger={activeWdaRunner && wdaRunnerStatus?.managed}
                        type={activeWdaRunner && wdaRunnerStatus?.managed ? "default" : "primary"}
                        icon={activeWdaRunner && wdaRunnerStatus?.managed ? <StopOutlined /> : <BugOutlined />}
                        loading={wdaRunnerAction === app.bundle_id}
                        disabled={wdaRunnerAction !== null || (wdaRunnerStatus?.managed === true && !activeWdaRunner)}
                        onClick={() => activeWdaRunner && wdaRunnerStatus?.managed ? void stopWdaRunner() : startWdaRunner(app)}
                      />
                    </Tooltip>
                  )}
                  <Tooltip title={t(app.is_running ? "deviceInspector.restartApp" : "deviceInspector.launchApp")}>
                    <Button
                      size="small"
                      type={app.is_running ? "default" : "primary"}
                      icon={app.is_running ? <ReloadOutlined /> : <PlayCircleOutlined />}
                      loading={appProcessAction?.bundleId === app.bundle_id && appProcessAction.kind === "launch"}
                      disabled={appProcessAction !== null}
                      onClick={() => void launch(app)}
                    />
                  </Tooltip>
                  {app.is_running === true && (
                    <Tooltip title={t("deviceInspector.stopApp")}>
                      <Button
                        danger
                        size="small"
                        icon={<StopOutlined />}
                        loading={appProcessAction?.bundleId === app.bundle_id && appProcessAction.kind === "stop"}
                        disabled={appProcessAction !== null}
                        onClick={() => void stopApp(app)}
                      />
                    </Tooltip>
                  )}
                  {app.is_removable && !app.is_first_party && (
                    <Tooltip title={t("deviceInspector.uninstallApp")}>
                      <Button
                        danger
                        size="small"
                        icon={<DeleteOutlined />}
                        disabled={appMutationRunning}
                        onClick={() => uninstallApp(app)}
                      />
                    </Tooltip>
                  )}
                </div>
              </div>;
            })}
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
                  {profile.removal_supported && (
                    <Tooltip title={t("deviceInspector.removeProfile")}>
                      <Button
                        danger
                        size="small"
                        icon={<DeleteOutlined />}
                        loading={profileMutation === profile.uuid}
                        disabled={profileMutation !== null}
                        onClick={() => removeProvisioningProfile(profile)}
                      />
                    </Tooltip>
                  )}
                </div>
                {profile.parse_error ? (
                  <Typography.Text type="danger" className="device-profile-error">{profile.parse_error}</Typography.Text>
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
                <Tooltip title={t("deviceInspector.exportCrashReport")}>
                  <Button
                    size="small"
                    icon={<DownloadOutlined />}
                    loading={exportingReport === report.path}
                    disabled={exportingReport !== null}
                    onClick={() => void exportCrashReport(report)}
                  />
                </Tooltip>
              </div>
            ))}
            {visibleCrashReports.length === 0 && <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={t("deviceInspector.noCrashReports")} />}
          </div>
        </div>
      )}
    </aside>
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
    <DeviceFilesModal open={deviceFilesOpen} request={request} onClose={() => setDeviceFilesOpen(false)} />
    <AppDocumentsModal app={documentsApp} request={request} onClose={() => setDocumentsApp(null)} />
    </>
  );
}
