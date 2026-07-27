import BugOutlined from "@ant-design/icons/es/icons/BugOutlined";
import CodeOutlined from "@ant-design/icons/es/icons/CodeOutlined";
import CopyOutlined from "@ant-design/icons/es/icons/CopyOutlined";
import DatabaseOutlined from "@ant-design/icons/es/icons/DatabaseOutlined";
import DeleteOutlined from "@ant-design/icons/es/icons/DeleteOutlined";
import DisconnectOutlined from "@ant-design/icons/es/icons/DisconnectOutlined";
import FolderOpenOutlined from "@ant-design/icons/es/icons/FolderOpenOutlined";
import LinkOutlined from "@ant-design/icons/es/icons/LinkOutlined";
import PlayCircleOutlined from "@ant-design/icons/es/icons/PlayCircleOutlined";
import ReloadOutlined from "@ant-design/icons/es/icons/ReloadOutlined";
import StopOutlined from "@ant-design/icons/es/icons/StopOutlined";
import { Button, Tag, Tooltip, Typography } from "antd";
import { memo, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { appProfileBindingState, formatFileSize, isEligibleWdaRunner } from "../../../deviceInspector";
import { resolveAppProfileBinding } from "../../../profileBindings";
import type { AppBindingConflict, AppProfileBinding, DeviceApp, HomeScreenAppLocation, ProfileResolution, WdaRunnerStatus } from "../../../types";

type Request = (path: string, init?: RequestInit) => Promise<Response>;

type Props = {
  app: DeviceApp;
  request: Request;
  location?: HomeScreenAppLocation;
  activeProfile: string;
  appProfileBindings: AppProfileBinding[];
  bindingConflicts: AppBindingConflict[];
  frameSize: ProfileResolution;
  bindingApp: string | null;
  appProcessAction: { bundleId: string; kind: "launch" | "stop" } | null;
  appMutationRunning: boolean;
  consoleOpen: boolean;
  wdaRunnerStatus: WdaRunnerStatus | null;
  wdaRunnerAction: string | null;
  onChangeProfileBinding: (bundleId: string, bind: boolean) => void;
  onCopyBundleId: (bundleId: string) => void;
  onOpenDocuments: (app: DeviceApp) => void;
  onStartWdaRunner: (app: DeviceApp) => void;
  onStopWdaRunner: () => void;
  onOpenConsole: (app: DeviceApp) => void;
  onLaunch: (app: DeviceApp) => void;
  onStop: (app: DeviceApp) => void;
  onUninstall: (app: DeviceApp) => void;
};

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

function appSigningTagColor(kind: DeviceApp["signing_kind"]): string | undefined {
  switch (kind) {
    case "system": return "gold";
    case "development": return "blue";
    case "test_flight": return "cyan";
    case "distribution": return "orange";
    case "app_store": return "green";
    case "unknown": return undefined;
  }
}

export const DeviceAppRow = memo(function DeviceAppRow({
  app,
  request,
  location,
  activeProfile,
  appProfileBindings,
  bindingConflicts,
  frameSize,
  bindingApp,
  appProcessAction,
  appMutationRunning,
  consoleOpen,
  wdaRunnerStatus,
  wdaRunnerAction,
  onChangeProfileBinding,
  onCopyBundleId,
  onOpenDocuments,
  onStartWdaRunner,
  onStopWdaRunner,
  onOpenConsole,
  onLaunch,
  onStop,
  onUninstall,
}: Props) {
  const { t } = useTranslation();
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
  const locationTooltip = location ? [rootPosition, ...folderRoute].join(" > ") : undefined;
  const resolvedBinding = resolveAppProfileBinding(app.bundle_id, frameSize, appProfileBindings, bindingConflicts);
  const bindingState = appProfileBindingState(app.bundle_id, activeProfile, frameSize, appProfileBindings, bindingConflicts);
  const boundProfile = resolvedBinding.binding?.profile;
  const eligibleWdaRunner = isEligibleWdaRunner(app);
  const activeWdaRunner = wdaRunnerStatus?.runner_bundle_id === app.bundle_id;
  const bindingTooltip = bindingState === "conflict"
    ? t("deviceInspector.appProfileConflict")
    : bindingState === "other"
      ? t("deviceInspector.appProfileBoundOther", { profile: boundProfile })
      : t(bindingState === "active" ? "deviceInspector.unbindAppProfile" : "deviceInspector.bindAppProfile", { profile: activeProfile });
  const signingTooltip = [
    t(`deviceInspector.appSigningKinds.${app.signing_kind}`),
    app.minimum_os_version ? t("deviceInspector.appMinimumOs", { version: app.minimum_os_version }) : null,
    app.debuggable === null
      ? null
      : t(app.debuggable ? "deviceInspector.appDebuggable" : "deviceInspector.appNotDebuggable"),
  ].filter(Boolean).join(" · ");

  return (
    <div className="device-app-row">
      <DeviceAppIcon app={app} request={request} />
      <div className="device-app-meta">
        <Typography.Text strong ellipsis={{ tooltip: app.name }}>{app.name}</Typography.Text>
        <Typography.Text type="secondary" ellipsis={{ tooltip: app.bundle_id }}>{app.bundle_id}</Typography.Text>
        <div className="device-app-tags">
          {app.version && <Tag>{app.version}</Tag>}
          {app.total_disk_usage_bytes !== null && (
            <Tooltip title={t("deviceInspector.appStorageBreakdown", {
              installed: app.static_disk_usage_bytes === null ? "-" : formatFileSize(app.static_disk_usage_bytes),
              data: app.dynamic_disk_usage_bytes === null ? "-" : formatFileSize(app.dynamic_disk_usage_bytes),
            })}>
              <Tag icon={<DatabaseOutlined />}>{formatFileSize(app.total_disk_usage_bytes)}</Tag>
            </Tooltip>
          )}
          {locationLabel && <Tooltip title={locationTooltip}><Tag color="cyan">{locationLabel}</Tag></Tooltip>}
          {app.is_running === true && <Tag color="success">{t("deviceInspector.runningApp")}</Tag>}
          <Tooltip title={signingTooltip}>
            <Tag color={appSigningTagColor(app.signing_kind)}>{t(`deviceInspector.appSigningKinds.${app.signing_kind}`)}</Tag>
          </Tooltip>
          {app.is_app_clip && <Tag color="processing">{t("deviceInspector.appClip")}</Tag>}
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
            onClick={() => onChangeProfileBinding(app.bundle_id, bindingState !== "active")}
          />
        </Tooltip>
        <Tooltip title={t("deviceInspector.copyBundleId")}>
          <Button size="small" icon={<CopyOutlined />} onClick={() => onCopyBundleId(app.bundle_id)} />
        </Tooltip>
        {(app.documents_available || app.is_developer_app) && (
          <Tooltip title={t("deviceInspector.appDocuments")}>
            <Button size="small" icon={<FolderOpenOutlined />} onClick={() => onOpenDocuments(app)} />
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
              onClick={() => activeWdaRunner && wdaRunnerStatus?.managed ? onStopWdaRunner() : onStartWdaRunner(app)}
            />
          </Tooltip>
        )}
        {(!app.is_first_party || app.is_developer_app) && !app.is_app_clip && (
          <Tooltip title={t("deviceInspector.launchWithConsole")}>
            <Button
              size="small"
              icon={<CodeOutlined />}
              disabled={appProcessAction !== null || consoleOpen}
              onClick={() => onOpenConsole(app)}
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
            onClick={() => onLaunch(app)}
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
              onClick={() => onStop(app)}
            />
          </Tooltip>
        )}
        {app.is_removable && !app.is_first_party && !app.is_app_clip && (
          <Tooltip title={t("deviceInspector.uninstallApp")}>
            <Button
              danger
              size="small"
              icon={<DeleteOutlined />}
              disabled={appMutationRunning}
              onClick={() => onUninstall(app)}
            />
          </Tooltip>
        )}
      </div>
    </div>
  );
});
