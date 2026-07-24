import {
  AppstoreOutlined,
  FileTextOutlined,
  FolderOpenOutlined,
  HddOutlined,
  ReloadOutlined,
} from "@ant-design/icons";
import { Alert, Button, Empty, Segmented, Select, Spin, Tag, Tooltip, Typography } from "antd";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { availableAfcApps } from "../afcBrowser";
import type { DeviceApp } from "../types";
import { AppDocumentsModal, type AppStorageScope } from "./AppDocumentsModal";
import { AfcCrashReportsPane } from "./AfcCrashReportsPane";
import { DeviceFilesPane } from "./DeviceFilesPane";

type Request = (path: string, init?: RequestInit) => Promise<Response>;
type AfcWorkspaceScope = "public" | AppStorageScope | "crash-reports";

type Props = {
  active: boolean;
  activeUdid: string | null;
  request: Request;
};

async function readJson<T>(response: Response): Promise<T> {
  if (!response.ok) throw new Error((await response.text()) || `${response.status} ${response.statusText}`);
  return response.json() as Promise<T>;
}

export function AfcPage({ active, activeUdid, request }: Props) {
  const { t, i18n } = useTranslation();
  const [scope, setScope] = useState<AfcWorkspaceScope>("public");
  const [apps, setApps] = useState<DeviceApp[] | null>(null);
  const [appsLoading, setAppsLoading] = useState(false);
  const [appsError, setAppsError] = useState<string | null>(null);
  const [selectedBundleId, setSelectedBundleId] = useState<string | null>(null);
  const [publicTransferActive, setPublicTransferActive] = useState(false);
  const [appTransferActive, setAppTransferActive] = useState(false);
  const [crashExportActive, setCrashExportActive] = useState(false);
  const appRequestVersion = useRef(0);
  const appScope = scope === "documents" || scope === "container";
  const workspaceTransferActive = publicTransferActive || appTransferActive || crashExportActive;

  const loadApps = async () => {
    if (!active || !activeUdid || !appScope) return;
    const version = ++appRequestVersion.current;
    setAppsLoading(true);
    setAppsError(null);
    try {
      const result = await readJson<DeviceApp[]>(await request("/api/device/apps"));
      if (appRequestVersion.current === version) setApps(result);
    } catch (error) {
      if (appRequestVersion.current === version) {
        setApps(null);
        setAppsError(String(error));
      }
    } finally {
      if (appRequestVersion.current === version) setAppsLoading(false);
    }
  };

  useEffect(() => {
    appRequestVersion.current += 1;
    setApps(null);
    setAppsError(null);
    setAppsLoading(false);
    setSelectedBundleId(null);
  }, [activeUdid]);

  useEffect(() => {
    if (active && activeUdid && appScope && apps === null && !appsLoading && !appsError) {
      void loadApps();
    }
  });

  const availableApps = useMemo(() => {
    if (!appScope) return [];
    return availableAfcApps(apps ?? [], scope, i18n.resolvedLanguage ?? i18n.language);
  }, [appScope, apps, i18n.language, i18n.resolvedLanguage, scope]);

  useEffect(() => {
    if (!appScope || availableApps.length === 0) {
      setSelectedBundleId(null);
      return;
    }
    if (!availableApps.some((app) => app.bundle_id === selectedBundleId)) {
      setSelectedBundleId(availableApps[0].bundle_id);
    }
  }, [appScope, availableApps, selectedBundleId]);

  const selectedApp = availableApps.find((app) => app.bundle_id === selectedBundleId) ?? null;

  return (
    <main className="afc-page" hidden={!active}>
      <header>
        <div>
          <Typography.Title level={3}><FolderOpenOutlined /> {t("afc.title")}</Typography.Title>
          <Typography.Text type="secondary">{t("afc.subtitle")}</Typography.Text>
        </div>
        <Tag color={activeUdid ? "success" : "default"}>
          {t(activeUdid ? "afc.connected" : "afc.disconnected")}
        </Tag>
      </header>

      {activeUdid ? (
        <section className="afc-browser" aria-label={t("afc.title")}>
          <div className="afc-scope-toolbar">
            <Segmented<AfcWorkspaceScope>
              block
              value={scope}
              disabled={workspaceTransferActive}
              options={[
                { value: "public", label: t("afc.scopes.public"), icon: <HddOutlined /> },
                { value: "documents", label: t("afc.scopes.documents"), icon: <FolderOpenOutlined /> },
                { value: "container", label: t("afc.scopes.container"), icon: <AppstoreOutlined /> },
                { value: "crash-reports", label: t("afc.scopes.crashReports"), icon: <FileTextOutlined /> },
              ]}
              onChange={setScope}
            />
            {appScope && (
              <div className="afc-app-picker">
                <Select
                  showSearch
                  value={selectedBundleId}
                  loading={appsLoading}
                  disabled={appTransferActive || appsLoading || appsError !== null || availableApps.length === 0}
                  placeholder={t("afc.selectApp")}
                  optionFilterProp="search"
                  options={availableApps.map((app) => ({
                    value: app.bundle_id,
                    label: app.name,
                    search: `${app.name} ${app.bundle_id}`,
                    title: `${app.name} (${app.bundle_id})`,
                  }))}
                  onChange={setSelectedBundleId}
                />
                <Typography.Text type="secondary" ellipsis={{ tooltip: selectedApp?.bundle_id }}>
                  {selectedApp?.bundle_id ?? t("afc.appCount", { count: availableApps.length })}
                </Typography.Text>
                <Tooltip title={t("afc.refreshApps")}>
                  <Button icon={<ReloadOutlined />} aria-label={t("afc.refreshApps")} loading={appsLoading} disabled={appTransferActive} onClick={() => void loadApps()} />
                </Tooltip>
              </div>
            )}
          </div>

          {scope === "public" ? (
            <DeviceFilesPane active={active} deviceId={activeUdid} refreshToken={0} request={request} onTransferStateChange={setPublicTransferActive} />
          ) : scope === "crash-reports" ? (
            <AfcCrashReportsPane active={active} deviceId={activeUdid} request={request} onTransferStateChange={setCrashExportActive} />
          ) : appsError ? (
            <Alert type="error" showIcon message={t("afc.appsUnavailable")} description={appsError} />
          ) : appsLoading && apps === null ? (
            <div className="app-documents-loading"><Spin /></div>
          ) : selectedApp ? (
            <AppDocumentsModal
              key={`${scope}:${selectedApp.bundle_id}`}
              active={active}
              embedded
              fixedScope={scope}
              app={selectedApp}
              request={request}
              onTransferStateChange={setAppTransferActive}
            />
          ) : (
            <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={t(scope === "documents" ? "afc.noDocumentApps" : "afc.noContainerApps")} />
          )}
        </section>
      ) : (
        <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={t("afc.connectDevice")} />
      )}
    </main>
  );
}
