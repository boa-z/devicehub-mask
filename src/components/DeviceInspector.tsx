import {
  AppstoreOutlined,
  BugOutlined,
  CopyOutlined,
  DeleteOutlined,
  DisconnectOutlined,
  DownloadOutlined,
  FileTextOutlined,
  FolderOpenOutlined,
  InfoCircleOutlined,
  LinkOutlined,
  PlayCircleOutlined,
  PoweroffOutlined,
  ReloadOutlined,
  SafetyCertificateOutlined,
  SearchOutlined,
  StopOutlined,
  UploadOutlined,
} from "@ant-design/icons";
import { open, save } from "@tauri-apps/plugin-dialog";
import { Alert, Button, Empty, Input, Modal, Progress, Segmented, Spin, Tag, Tooltip, Typography, message } from "antd";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { AppDocumentsModal } from "./AppDocumentsModal";
import { appProfileBindingState, filterCrashReports, filterDeviceApps, filterProvisioningProfiles, formatCapacity, formatFileSize, formatProfileDate, formatReportDate } from "../deviceInspector";
import type { ProfileStatusFilter } from "../deviceInspector";
import type { AppOperation, DeviceApp, DeviceCrashReport, DeviceCrashReportList, DeviceDetails, ProvisioningProfile } from "../types";

type InspectorTab = "info" | "apps" | "profiles" | "crashes";
type Request = (path: string, init?: RequestInit) => Promise<Response>;

type Props = {
  activeUdid: string | null;
  request: Request;
  activeProfile: string;
  appProfileBindings: Record<string, string>;
  bindingConflicts: string[];
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
  onAppLaunched,
  onAppProfileBindingChange,
}: Props) {
  const { t, i18n } = useTranslation();
  const [tab, setTab] = useState<InspectorTab>("info");
  const [details, setDetails] = useState<DeviceDetails | null>(null);
  const [apps, setApps] = useState<DeviceApp[]>([]);
  const [profiles, setProfiles] = useState<ProvisioningProfile[]>([]);
  const [crashReports, setCrashReports] = useState<DeviceCrashReport[]>([]);
  const [crashReportsTruncated, setCrashReportsTruncated] = useState(false);
  const [query, setQuery] = useState("");
  const [profileStatus, setProfileStatus] = useState<ProfileStatusFilter>("all");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [appProcessAction, setAppProcessAction] = useState<{ bundleId: string; kind: "launch" | "stop" } | null>(null);
  const [exportingReport, setExportingReport] = useState<string | null>(null);
  const [bindingApp, setBindingApp] = useState<string | null>(null);
  const [appOperation, setAppOperation] = useState<AppOperation | null>(null);
  const [devicePowerAction, setDevicePowerAction] = useState<"restart" | "shutdown" | null>(null);
  const [documentsApp, setDocumentsApp] = useState<DeviceApp | null>(null);
  const handledOperation = useRef(0);

  const loadApps = useCallback(async () => {
    setApps(await readJson<DeviceApp[]>(await request("/api/device/apps")));
  }, [request]);

  const load = useCallback(async () => {
    if (!activeUdid) return;
    setLoading(true);
    setError(null);
    try {
      if (tab === "info") {
        setDetails(await readJson<DeviceDetails>(await request("/api/device/details")));
      } else if (tab === "apps") {
        await loadApps();
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
  }, [activeUdid, loadApps, request, tab]);

  useEffect(() => {
    setDetails(null);
    setApps([]);
    setProfiles([]);
    setCrashReports([]);
    setCrashReportsTruncated(false);
    setAppOperation(null);
    setDocumentsApp(null);
    setError(null);
  }, [activeUdid]);

  useEffect(() => {
    void load();
  }, [load]);

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
      if (tab === "apps") void loadApps();
    } else if (appOperation.state === "failed") {
      void message.error(t("deviceInspector.appOperationFailed", { error: appOperation.error ?? "" }));
    } else {
      void message.info(t("deviceInspector.appOperationCancelled"));
    }
  }, [appOperation, loadApps, t, tab]);

  const visibleApps = useMemo(() => filterDeviceApps(apps, query), [apps, query]);
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

  const copyBundleId = async (bundleId: string) => {
    await navigator.clipboard.writeText(bundleId);
    void message.success(t("deviceInspector.bundleIdCopied"));
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

  const appMutationRunning = appOperation?.state === "running";

  const infoRows = details ? [
    [t("deviceInspector.name"), details.name],
    [t("deviceInspector.os"), `iOS ${details.product_version}${details.build_version ? ` (${details.build_version})` : ""}`],
    [t("deviceInspector.udid"), details.udid],
    [t("deviceInspector.capacity"), formatCapacity(details.total_disk_capacity)],
    [t("deviceInspector.productType"), details.product_type],
    [t("deviceInspector.hardwareModel"), details.hardware_model ?? "-"],
    [t("deviceInspector.serialNumber"), details.serial_number ?? "-"],
    [t("deviceInspector.ecid"), details.ecid?.toString() ?? "-"],
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

  return (
    <>
    <aside className="device-inspector">
      <div className="device-inspector-header">
        <Segmented<InspectorTab>
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
          {details?.developer_mode_enabled === false && (
            <Alert
              type="warning"
              showIcon
              message={t("deviceInspector.developerModeDisabled")}
              description={t("deviceInspector.developerModeHint")}
            />
          )}
          <div className="device-info-list">
            {infoRows.map(([label, value]) => (
              <div className="device-info-row" key={label}>
                <Typography.Text>{label}</Typography.Text>
                <Typography.Text type="secondary" ellipsis={{ tooltip: value }}>{value}</Typography.Text>
              </div>
            ))}
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
          <div className="device-app-count">{t("deviceInspector.appCount", { count: visibleApps.length })}</div>
          <div className="device-app-list">
            {visibleApps.map((app) => {
              const bindingState = appProfileBindingState(app.bundle_id, activeProfile, appProfileBindings, bindingConflicts);
              const boundProfile = appProfileBindings[app.bundle_id];
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
                    {app.is_running === true && <Tag color="success">{t("deviceInspector.runningApp")}</Tag>}
                    {app.is_developer_app && <Tag color="blue">{t("deviceInspector.developerApp")}</Tag>}
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
          <Input
            allowClear
            value={query}
            prefix={<SearchOutlined />}
            placeholder={t("deviceInspector.searchProfiles")}
            onChange={(event) => setQuery(event.target.value)}
          />
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
    <AppDocumentsModal app={documentsApp} request={request} onClose={() => setDocumentsApp(null)} />
    </>
  );
}
