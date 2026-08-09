import ApiOutlined from "@ant-design/icons/es/icons/ApiOutlined";
import SafetyCertificateOutlined from "@ant-design/icons/es/icons/SafetyCertificateOutlined";
import StopOutlined from "@ant-design/icons/es/icons/StopOutlined";
import SyncOutlined from "@ant-design/icons/es/icons/SyncOutlined";
import { Button, Tag, Tooltip, Typography } from "antd";
import { useTranslation } from "react-i18next";
import { ErrorCopyButton } from "../../components/ErrorPresentation";
import { canConnectTransport, isActiveSession } from "../device-session/deviceConnections";
import type { Device } from "../../types";
import type { DashboardDeviceGroup } from "./deviceDashboardModel";

type Props = {
  group: DashboardDeviceGroup;
  allDevices: Device[];
  selectedDeviceId: string | null;
  pairingDeviceId: string | null;
  busyActions: ReadonlySet<string>;
  updatedLabel: string | null;
  onAction: (key: string, operation: () => Promise<unknown>) => void;
  onOpenControl: (deviceId: string) => Promise<unknown>;
  onConnect: (deviceId: string) => Promise<unknown>;
  onReconnect: (deviceId: string) => Promise<unknown>;
  onDisconnect: (deviceId: string) => Promise<unknown>;
  onPair: (deviceId: string) => Promise<unknown>;
};

const resourceKeys = ["video", "audio", "performance", "device_logs"] as const;

export function DeviceDashboardItem({
  group,
  allDevices,
  selectedDeviceId,
  pairingDeviceId,
  busyActions,
  updatedLabel,
  onAction,
  onOpenControl,
  onConnect,
  onReconnect,
  onDisconnect,
  onPair,
}: Props) {
  const { t } = useTranslation();
  const isSelectedGroup = group.devices.some((device) => device.id === selectedDeviceId);

  return (
    <article className={`device-dashboard-item${isSelectedGroup ? " is-selected" : ""}`}>
      <header>
        <div className="device-dashboard-identity">
          <span className={`device-phase-dot is-${group.phase}`} />
          <div>
            <Typography.Title level={4}>{group.name}</Typography.Title>
            <Typography.Text type="secondary">{group.udid}</Typography.Text>
          </div>
        </div>
        <div className="device-dashboard-state">
          {isSelectedGroup && <Tag color="success">{t("device.currentControlTarget")}</Tag>}
          <Tag>{t(`device.sessionPhases.${group.phase}`)}</Tag>
        </div>
      </header>

      <div className="device-dashboard-resources" aria-label={t("dashboard.resourceDemand")}>
        {resourceKeys.map((resource) => (
          <span className={group.resources[resource] ? "is-active" : ""} key={resource}>
            <span aria-hidden />
            {t(`dashboard.resources.${resource}`)}
          </span>
        ))}
      </div>

      <div className="device-dashboard-transports">
        {group.devices.map((device) => {
          const phase = device.session_phase ?? "discovered";
          const active = isActiveSession(device);
          const unpaired = device.connection === "USB" && device.pairing === "unpaired";
          const selected = device.id === selectedDeviceId;
          const actionKey = (action: string) => `${action}:${device.id}`;
          const isBusy = (action: string) => busyActions.has(actionKey(action));
          return (
            <div className={`device-dashboard-transport${selected ? " is-selected" : ""}`} key={device.id}>
              <span className={`device-phase-dot is-${phase}`} />
              <div className="device-dashboard-transport-copy">
                <div>
                  <strong>{device.connection}</strong>
                  <span>{unpaired ? t("device.trustRequired") : t(`device.sessionPhases.${phase}`)}</span>
                </div>
                {(device.session_error || device.session_status) && (
                  <Typography.Text type={device.session_error ? "danger" : "secondary"} ellipsis title={device.session_error ?? device.session_status ?? undefined}>
                    {device.session_error ?? device.session_status}
                  </Typography.Text>
                )}
              </div>
              <div className="device-dashboard-actions">
                {device.session_error && <ErrorCopyButton error={device.session_error} />}
                {unpaired ? (
                  <Button
                    size="small"
                    icon={<SafetyCertificateOutlined />}
                    loading={pairingDeviceId === device.id || isBusy("pair")}
                    onClick={() => onAction(actionKey("pair"), () => onPair(device.id))}
                  >
                    {t("device.pairDevice")}
                  </Button>
                ) : active ? (
                  <>
                    <Button
                      size="small"
                      type="primary"
                      icon={<ApiOutlined />}
                      loading={isBusy("open")}
                      onClick={() => onAction(actionKey("open"), () => onOpenControl(device.id))}
                    >
                      {t("dashboard.openControl")}
                    </Button>
                    <Tooltip title={t("device.reconnect")}>
                      <Button
                        size="small"
                        type="text"
                        aria-label={t("device.reconnect")}
                        disabled={phase === "disconnecting"}
                        loading={isBusy("reconnect")}
                        icon={<SyncOutlined />}
                        onClick={() => onAction(actionKey("reconnect"), () => onReconnect(device.id))}
                      />
                    </Tooltip>
                    <Tooltip title={t("device.disconnect")}>
                      <Button
                        size="small"
                        type="text"
                        danger
                        aria-label={t("device.disconnect")}
                        disabled={phase === "disconnecting"}
                        loading={isBusy("disconnect")}
                        icon={<StopOutlined />}
                        onClick={() => onAction(actionKey("disconnect"), () => onDisconnect(device.id))}
                      />
                    </Tooltip>
                  </>
                ) : canConnectTransport(device, allDevices) ? (
                  <Button
                    size="small"
                    type="primary"
                    icon={<ApiOutlined />}
                    loading={isBusy("connect")}
                    onClick={() => onAction(actionKey("connect"), () => onConnect(device.id))}
                  >
                    {t("device.connect")}
                  </Button>
                ) : (
                  <Tooltip title={t("device.disconnectActiveTransportFirst")}>
                    <Button size="small" disabled icon={<ApiOutlined />}>{t("device.connect")}</Button>
                  </Tooltip>
                )}
              </div>
            </div>
          );
        })}
      </div>

      <footer>
        <span>{t("dashboard.transportCount", { count: group.devices.length })}</span>
        <span>{updatedLabel ? t("dashboard.updated", { value: updatedLabel }) : t("dashboard.notConnectedYet")}</span>
      </footer>
    </article>
  );
}
