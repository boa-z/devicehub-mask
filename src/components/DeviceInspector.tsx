import {
  AppstoreOutlined,
  CopyOutlined,
  InfoCircleOutlined,
  PlayCircleOutlined,
  ReloadOutlined,
  SafetyCertificateOutlined,
  SearchOutlined,
} from "@ant-design/icons";
import { Alert, Button, Empty, Input, Segmented, Spin, Tag, Tooltip, Typography, message } from "antd";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { filterDeviceApps, filterProvisioningProfiles, formatCapacity, formatProfileDate } from "../deviceInspector";
import type { ProfileStatusFilter } from "../deviceInspector";
import type { DeviceApp, DeviceDetails, ProvisioningProfile } from "../types";

type InspectorTab = "info" | "apps" | "profiles";
type Request = (path: string, init?: RequestInit) => Promise<Response>;

type Props = {
  activeUdid: string | null;
  request: Request;
};

async function readJson<T>(response: Response): Promise<T> {
  if (!response.ok) {
    throw new Error((await response.text()) || `${response.status} ${response.statusText}`);
  }
  return response.json() as Promise<T>;
}

export function DeviceInspector({ activeUdid, request }: Props) {
  const { t, i18n } = useTranslation();
  const [tab, setTab] = useState<InspectorTab>("info");
  const [details, setDetails] = useState<DeviceDetails | null>(null);
  const [apps, setApps] = useState<DeviceApp[]>([]);
  const [profiles, setProfiles] = useState<ProvisioningProfile[]>([]);
  const [query, setQuery] = useState("");
  const [profileStatus, setProfileStatus] = useState<ProfileStatusFilter>("all");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [launching, setLaunching] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!activeUdid) return;
    setLoading(true);
    setError(null);
    try {
      if (tab === "info") {
        setDetails(await readJson<DeviceDetails>(await request("/api/device/details")));
      } else if (tab === "apps") {
        setApps(await readJson<DeviceApp[]>(await request("/api/device/apps")));
      } else {
        setProfiles(await readJson<ProvisioningProfile[]>(await request("/api/device/provisioning-profiles")));
      }
    } catch (loadError) {
      setError(String(loadError));
    } finally {
      setLoading(false);
    }
  }, [activeUdid, request, tab]);

  useEffect(() => {
    setDetails(null);
    setApps([]);
    setProfiles([]);
    setError(null);
  }, [activeUdid]);

  useEffect(() => {
    void load();
  }, [load]);

  const visibleApps = useMemo(() => filterDeviceApps(apps, query), [apps, query]);
  const visibleProfiles = useMemo(
    () => filterProvisioningProfiles(profiles, query, profileStatus),
    [profileStatus, profiles, query],
  );

  const launch = async (app: DeviceApp) => {
    setLaunching(app.bundle_id);
    try {
      const response = await request(`/api/device/apps/${encodeURIComponent(app.bundle_id)}/launch`, { method: "PUT" });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      void message.success(t("deviceInspector.appLaunched", { name: app.name }));
    } catch (launchError) {
      void message.error(t("deviceInspector.appLaunchFailed", { error: String(launchError) }));
    } finally {
      setLaunching(null);
    }
  };

  const copyBundleId = async (bundleId: string) => {
    await navigator.clipboard.writeText(bundleId);
    void message.success(t("deviceInspector.bundleIdCopied"));
  };

  const infoRows = details ? [
    [t("deviceInspector.name"), details.name],
    [t("deviceInspector.os"), `iOS ${details.product_version}${details.build_version ? ` (${details.build_version})` : ""}`],
    [t("deviceInspector.udid"), details.udid],
    [t("deviceInspector.capacity"), formatCapacity(details.total_disk_capacity)],
    [t("deviceInspector.productType"), details.product_type],
    [t("deviceInspector.hardwareModel"), details.hardware_model ?? "-"],
    [t("deviceInspector.serialNumber"), details.serial_number ?? "-"],
    [t("deviceInspector.ecid"), details.ecid?.toString() ?? "-"],
  ] : [];

  return (
    <aside className="device-inspector">
      <div className="device-inspector-header">
        <Segmented<InspectorTab>
          block
          value={tab}
          options={[
            { value: "info", label: t("deviceInspector.info"), icon: <InfoCircleOutlined /> },
            { value: "apps", label: t("deviceInspector.apps"), icon: <AppstoreOutlined /> },
            { value: "profiles", label: t("deviceInspector.profiles"), icon: <SafetyCertificateOutlined /> },
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
      ) : loading && (tab === "info" ? !details : tab === "apps" ? apps.length === 0 : profiles.length === 0) ? (
        <div className="device-inspector-loading"><Spin /></div>
      ) : tab === "info" ? (
        <div className="device-info-list">
          {infoRows.map(([label, value]) => (
            <div className="device-info-row" key={label}>
              <Typography.Text>{label}</Typography.Text>
              <Typography.Text type="secondary" ellipsis={{ tooltip: value }}>{value}</Typography.Text>
            </div>
          ))}
        </div>
      ) : tab === "apps" ? (
        <div className="device-apps-pane">
          <Input
            allowClear
            value={query}
            prefix={<SearchOutlined />}
            placeholder={t("deviceInspector.searchApps")}
            onChange={(event) => setQuery(event.target.value)}
          />
          <div className="device-app-count">{t("deviceInspector.appCount", { count: visibleApps.length })}</div>
          <div className="device-app-list">
            {visibleApps.map((app) => (
              <div className="device-app-row" key={app.bundle_id}>
                <div className="device-app-icon" aria-hidden="true">{app.name.slice(0, 1).toLocaleUpperCase()}</div>
                <div className="device-app-meta">
                  <Typography.Text strong ellipsis={{ tooltip: app.name }}>{app.name}</Typography.Text>
                  <Typography.Text type="secondary" ellipsis={{ tooltip: app.bundle_id }}>{app.bundle_id}</Typography.Text>
                  <div className="device-app-tags">
                    {app.version && <Tag>{app.version}</Tag>}
                    {app.is_developer_app && <Tag color="blue">{t("deviceInspector.developerApp")}</Tag>}
                  </div>
                </div>
                <div className="device-app-actions">
                  <Tooltip title={t("deviceInspector.copyBundleId")}>
                    <Button size="small" icon={<CopyOutlined />} onClick={() => void copyBundleId(app.bundle_id)} />
                  </Tooltip>
                  <Tooltip title={t("deviceInspector.launchApp")}>
                    <Button
                      size="small"
                      type="primary"
                      icon={<PlayCircleOutlined />}
                      loading={launching === app.bundle_id}
                      onClick={() => void launch(app)}
                    />
                  </Tooltip>
                </div>
              </div>
            ))}
            {visibleApps.length === 0 && <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={t("deviceInspector.noApps")} />}
          </div>
        </div>
      ) : (
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
      )}
    </aside>
  );
}
