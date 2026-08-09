import ReloadOutlined from "@ant-design/icons/es/icons/ReloadOutlined";
import { Button, Empty, Typography } from "antd";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { Device } from "../../types";
import { connectedPhysicalDeviceCount } from "../device-session/deviceConnections";
import { DeviceDashboardItem } from "./DeviceDashboardItem";
import { buildDashboardGroups, relativeUpdateTime } from "./deviceDashboardModel";
import "./deviceDashboard.css";

type Props = {
  devices: Device[];
  selectedDeviceId: string | null;
  backendReady: boolean;
  pairingDeviceId: string | null;
  startupDevicePriority: string[];
  onOpenControl: (deviceId: string) => Promise<unknown>;
  onConnect: (deviceId: string) => Promise<unknown>;
  onReconnect: (deviceId: string) => Promise<unknown>;
  onDisconnect: (deviceId: string) => Promise<unknown>;
  onPair: (deviceId: string) => Promise<unknown>;
  onRefresh: () => Promise<unknown>;
};

export function DeviceDashboardPage({
  devices,
  selectedDeviceId,
  backendReady,
  pairingDeviceId,
  startupDevicePriority,
  onOpenControl,
  onConnect,
  onReconnect,
  onDisconnect,
  onPair,
  onRefresh,
}: Props) {
  const { t, i18n } = useTranslation();
  const [busyActions, setBusyActions] = useState<ReadonlySet<string>>(() => new Set());
  const groups = useMemo(
    () => buildDashboardGroups(devices, selectedDeviceId, startupDevicePriority),
    [devices, selectedDeviceId, startupDevicePriority],
  );
  const connectedCount = connectedPhysicalDeviceCount(devices);
  const activeResourceCount = groups.reduce((count, group) => count
    + Object.values(group.resources).filter(Boolean).length, 0);

  const runAction = (key: string, operation: () => Promise<unknown>) => {
    if (busyActions.has(key)) return;
    setBusyActions((current) => new Set(current).add(key));
    void operation().finally(() => setBusyActions((current) => {
      const next = new Set(current);
      next.delete(key);
      return next;
    }));
  };

  return (
    <main className="device-dashboard-page">
      <header>
        <div>
          <Typography.Title level={2}>{t("dashboard.title")}</Typography.Title>
          <Typography.Text type="secondary">{t("dashboard.subtitle")}</Typography.Text>
        </div>
        <Button
          icon={<ReloadOutlined />}
          disabled={!backendReady}
          loading={busyActions.has("refresh")}
          onClick={() => runAction("refresh", onRefresh)}
        >
          {t("device.refresh")}
        </Button>
      </header>

      <section className="device-dashboard-summary" aria-label={t("dashboard.summary")}>
        <div><strong>{groups.length}</strong><span>{t("dashboard.discovered")}</span></div>
        <div><strong>{connectedCount}</strong><span>{t("dashboard.activeSessions")}</span></div>
        <div><strong>{activeResourceCount}</strong><span>{t("dashboard.activeDemands")}</span></div>
      </section>

      {!backendReady ? (
        <Empty description={t("dashboard.backendUnavailable")} />
      ) : groups.length === 0 ? (
        <Empty description={t("device.noDevices")} />
      ) : (
        <section className="device-dashboard-grid" aria-label={t("dashboard.devices")}>
          {groups.map((group) => (
            <DeviceDashboardItem
              key={group.udid}
              group={group}
              allDevices={devices}
              selectedDeviceId={selectedDeviceId}
              pairingDeviceId={pairingDeviceId}
              busyActions={busyActions}
              updatedLabel={relativeUpdateTime(group.latestUpdateMs, Date.now(), i18n.language)}
              onAction={runAction}
              onOpenControl={onOpenControl}
              onConnect={onConnect}
              onReconnect={onReconnect}
              onDisconnect={onDisconnect}
              onPair={onPair}
            />
          ))}
        </section>
      )}
    </main>
  );
}
