import {
  AppstoreOutlined,
  CheckOutlined,
  DatabaseOutlined,
  FilterOutlined,
  InfoCircleOutlined,
  SearchOutlined,
  SortAscendingOutlined,
  SortDescendingOutlined,
  ThunderboltOutlined,
} from "@ant-design/icons";
import { Alert, Button, Dropdown, Empty, Input, Progress, Spin, Tooltip, Typography } from "antd";
import { memo, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  APP_RENDER_BATCH_SIZE,
  filterDeviceApps,
  nextAppRenderLimit,
  sortDeviceApps,
} from "../../../deviceInspector";
import type { DeviceAppSort } from "../../../deviceInspector";
import type { AppOperation, DeviceApp, HomeScreenLayout, WdaRunnerStatus } from "../../../types";
import { DeviceAppRow } from "./DeviceAppRow";

type Request = (path: string, init?: RequestInit) => Promise<Response>;

type Props = {
  apps: DeviceApp[];
  request: Request;
  query: string;
  appSort: DeviceAppSort;
  showSystemApps: boolean;
  showAppClips: boolean;
  loading: boolean;
  appScopesLoading: boolean;
  appOperation: AppOperation | null;
  homeScreenLayout: HomeScreenLayout | null;
  homeScreenLoading: boolean;
  homeScreenError: string | null;
  activeProfile: string;
  appProfileBindings: Record<string, string>;
  bindingConflicts: string[];
  bindingApp: string | null;
  appProcessAction: { bundleId: string; kind: "launch" | "stop" } | null;
  appMutationRunning: boolean;
  consoleOpen: boolean;
  wdaRunnerStatus: WdaRunnerStatus | null;
  wdaRunnerAction: string | null;
  onQueryChange: (query: string) => void;
  onSortChange: (sort: DeviceAppSort) => void;
  onToggleScope: (scope: "system" | "clips") => void;
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

export const AppsPane = memo(function AppsPane({
  apps,
  request,
  query,
  appSort,
  showSystemApps,
  showAppClips,
  loading,
  appScopesLoading,
  appOperation,
  homeScreenLayout,
  homeScreenLoading,
  homeScreenError,
  activeProfile,
  appProfileBindings,
  bindingConflicts,
  bindingApp,
  appProcessAction,
  appMutationRunning,
  consoleOpen,
  wdaRunnerStatus,
  wdaRunnerAction,
  onQueryChange,
  onSortChange,
  onToggleScope,
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
  const { t, i18n } = useTranslation();
  const [renderLimit, setRenderLimit] = useState(APP_RENDER_BATCH_SIZE);
  const listContainer = useRef<HTMLDivElement>(null);
  const listSentinel = useRef<HTMLDivElement>(null);
  const visibleApps = useMemo(
    () => sortDeviceApps(
      filterDeviceApps(apps, query),
      appSort,
      i18n.resolvedLanguage ?? i18n.language,
    ),
    [appSort, apps, i18n.language, i18n.resolvedLanguage, query],
  );
  const renderedApps = useMemo(
    () => visibleApps.slice(0, renderLimit),
    [renderLimit, visibleApps],
  );
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

  useEffect(() => {
    setRenderLimit(Math.min(APP_RENDER_BATCH_SIZE, visibleApps.length));
    listContainer.current?.scrollTo({ top: 0 });
  }, [appSort, apps, query, showAppClips, showSystemApps, visibleApps.length]);

  useEffect(() => {
    const root = listContainer.current;
    const sentinel = listSentinel.current;
    if (!root || !sentinel || renderLimit >= visibleApps.length) return;
    if (typeof IntersectionObserver === "undefined") {
      setRenderLimit(visibleApps.length);
      return;
    }
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) {
        setRenderLimit((current) => nextAppRenderLimit(current, visibleApps.length));
      }
    }, { root, rootMargin: "400px 0px" });
    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [renderLimit, visibleApps.length]);

  return (
    <div className="device-apps-pane">
      <div className="device-app-toolbar">
        <Input
          allowClear
          value={query}
          prefix={<SearchOutlined />}
          placeholder={t("deviceInspector.searchApps")}
          onChange={(event) => onQueryChange(event.target.value)}
        />
        <Dropdown
          menu={{
            items: [
              { key: "name", icon: appSort === "name" ? <CheckOutlined /> : <SortAscendingOutlined />, label: t("deviceInspector.sortAppsByName") },
              { key: "storage", icon: appSort === "storage" ? <CheckOutlined /> : <DatabaseOutlined />, label: t("deviceInspector.sortAppsByStorage") },
            ],
            onClick: ({ key }) => onSortChange(key as DeviceAppSort),
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
            onClick: ({ key }) => onToggleScope(key as "system" | "clips"),
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
      <div className="device-app-list" ref={listContainer}>
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
            consoleOpen={consoleOpen}
            wdaRunnerStatus={wdaRunnerStatus}
            wdaRunnerAction={wdaRunnerAction}
            onChangeProfileBinding={onChangeProfileBinding}
            onCopyBundleId={onCopyBundleId}
            onOpenDocuments={onOpenDocuments}
            onStartWdaRunner={onStartWdaRunner}
            onStopWdaRunner={onStopWdaRunner}
            onOpenConsole={onOpenConsole}
            onLaunch={onLaunch}
            onStop={onStop}
            onUninstall={onUninstall}
          />
        ))}
        {renderedApps.length < visibleApps.length && <div ref={listSentinel} className="device-app-list-sentinel" />}
        {visibleApps.length === 0 && <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={t("deviceInspector.noApps")} />}
      </div>
    </div>
  );
});
