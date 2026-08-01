import ApiOutlined from "@ant-design/icons/es/icons/ApiOutlined";
import ReloadOutlined from "@ant-design/icons/es/icons/ReloadOutlined";
import SafetyCertificateOutlined from "@ant-design/icons/es/icons/SafetyCertificateOutlined";
import StopOutlined from "@ant-design/icons/es/icons/StopOutlined";
import SyncOutlined from "@ant-design/icons/es/icons/SyncOutlined";
import { Button, Popover, Tag, Tooltip, Typography } from "antd";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { canConnectTransport, connectedPhysicalDeviceCount, groupDevices, isActiveSession } from "../deviceConnections";
import type { Device } from "../types";
import { ErrorCopyButton } from "./ErrorPresentation";

type DeviceConnectionCenterProps = {
  devices: Device[];
  selectedDeviceId: string | null;
  backendReady: boolean;
  pairingDeviceId: string | null;
  startupDevicePriority: string[];
  onStartupDevicePriorityChange: (priority: string[]) => void;
  onConnect: (deviceId: string) => void;
  onReconnect: (deviceId: string) => void;
  onDisconnect: (deviceId: string) => void;
  onPair: (deviceId: string) => void;
  onRefresh: () => void;
};

export function DeviceConnectionCenter({
  devices,
  selectedDeviceId,
  backendReady,
  pairingDeviceId,
  startupDevicePriority,
  onStartupDevicePriorityChange,
  onConnect,
  onReconnect,
  onDisconnect,
  onPair,
  onRefresh,
}: DeviceConnectionCenterProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const groups = useMemo(() => groupDevices(devices, startupDevicePriority), [devices, startupDevicePriority]);
  const connectedCount = connectedPhysicalDeviceCount(devices);
  const selected = devices.find((device) => device.id === selectedDeviceId);

  const content = (
    <div className="device-connection-center">
      <div className="device-center-header">
        <div>
          <Typography.Text strong>{t("device.devices")}</Typography.Text>
          <Typography.Text type="secondary">{t("device.connectedCount", { count: connectedCount })}</Typography.Text>
        </div>
        <div className="device-center-header-actions">
          {startupDevicePriority.length > 0 && (
            <Tooltip title={t("device.priorityClear")}>
              <Button size="small" type="text" aria-label={t("device.priorityClear")} icon={<span aria-hidden>×</span>} onClick={() => onStartupDevicePriorityChange([])} />
            </Tooltip>
          )}
          <Tooltip title={t("device.refresh")}>
            <Button size="small" aria-label={t("device.refresh")} disabled={!backendReady} icon={<ReloadOutlined />} onClick={onRefresh} />
          </Tooltip>
        </div>
      </div>
      {groups.length === 0 ? (
        <div className="device-center-empty">{t("device.noDevices")}</div>
      ) : ([true, false] as const).map((active) => {
        const section = groups.filter((group) => group.active === active);
        if (section.length === 0) return null;
        return (
          <section className="device-center-section" key={String(active)}>
            <div className="device-center-section-title">{t(active ? "device.connectedDevices" : "device.availableDevices")}</div>
            {section.map((group) => (
              <div className="device-group" key={group.udid}>
                <div className="device-group-title">
                  <span>{group.name}</span>
                  {startupDevicePriority.includes(group.udid) && (
                    <Tag color="gold">#{startupDevicePriority.indexOf(group.udid) + 1}</Tag>
                  )}
                  <div className="device-priority-actions">
                    {startupDevicePriority.includes(group.udid) ? (
                      <>
                        <Tooltip title={t("device.priorityUp")}><Button size="small" type="text" aria-label={t("device.priorityUp")} disabled={startupDevicePriority[0] === group.udid} icon={<span aria-hidden>↑</span>} onClick={() => onStartupDevicePriorityChange(movePriority(startupDevicePriority, group.udid, -1))} /></Tooltip>
                        <Tooltip title={t("device.priorityDown")}><Button size="small" type="text" aria-label={t("device.priorityDown")} disabled={startupDevicePriority.at(-1) === group.udid} icon={<span aria-hidden>↓</span>} onClick={() => onStartupDevicePriorityChange(movePriority(startupDevicePriority, group.udid, 1))} /></Tooltip>
                        <Tooltip title={t("device.priorityRemove")}><Button className="is-priority-toggle" size="small" type="text" aria-label={t("device.priorityRemove")} icon={<span aria-hidden>★</span>} onClick={() => onStartupDevicePriorityChange(startupDevicePriority.filter((udid) => udid !== group.udid))} /></Tooltip>
                      </>
                    ) : (
                      <Tooltip title={t("device.priorityAdd")}><Button className="is-priority-toggle" size="small" type="text" aria-label={t("device.priorityAdd")} icon={<span aria-hidden>☆</span>} onClick={() => onStartupDevicePriorityChange([...startupDevicePriority, group.udid])} /></Tooltip>
                    )}
                  </div>
                </div>
                {group.devices.map((device) => {
                  const phase = device.session_phase ?? "discovered";
                  const selectedTarget = device.id === selectedDeviceId;
                  const activeSession = isActiveSession(device);
                  const unpaired = device.connection === "USB" && device.pairing === "unpaired";
                  return (
                    <div className={`device-transport-row${selectedTarget ? " is-selected" : ""}`} key={device.id}>
                      <span className={`device-phase-dot is-${phase}`} />
                      <div className="device-transport-detail">
                        <div>
                          <Typography.Text>{device.connection}</Typography.Text>
                          {selectedTarget && <Tag color="success">{t("device.currentControlTarget")}</Tag>}
                        </div>
                        <Typography.Text type={device.session_error ? "danger" : "secondary"} ellipsis title={device.session_error ?? device.session_status ?? undefined}>
                          {device.session_error ?? (unpaired ? t("device.trustRequired") : t(`device.sessionPhases.${phase}`))}
                        </Typography.Text>
                      </div>
                      <div className="device-transport-actions">
                        {device.session_error && <ErrorCopyButton error={device.session_error} />}
                        {unpaired ? (
                          <Tooltip title={t("device.pairDeviceHint")}>
                            <Button size="small" type="text" aria-label={t("device.pairDevice")} loading={pairingDeviceId === device.id} icon={<SafetyCertificateOutlined />} onClick={() => onPair(device.id)} />
                          </Tooltip>
                        ) : activeSession ? (
                          <>
                            {!selectedTarget && <Tooltip title={t("device.selectControlTarget")}><Button size="small" type="text" aria-label={t("device.selectControlTarget")} icon={<ApiOutlined />} onClick={() => onConnect(device.id)} /></Tooltip>}
                            <Tooltip title={t("device.reconnect")}><Button size="small" type="text" aria-label={t("device.reconnect")} disabled={phase === "disconnecting"} icon={<SyncOutlined />} onClick={() => onReconnect(device.id)} /></Tooltip>
                            <Tooltip title={t("device.disconnect")}><Button size="small" type="text" danger aria-label={t("device.disconnect")} disabled={phase === "disconnecting"} icon={<StopOutlined />} onClick={() => onDisconnect(device.id)} /></Tooltip>
                          </>
                        ) : canConnectTransport(device, devices) ? (
                          <Tooltip title={t("device.connect")}><Button size="small" type="text" aria-label={t("device.connect")} icon={<ApiOutlined />} onClick={() => onConnect(device.id)} /></Tooltip>
                        ) : (
                          <Tooltip title={t("device.disconnectActiveTransportFirst")}><Button size="small" type="text" aria-label={t("device.connect")} disabled icon={<ApiOutlined />} /></Tooltip>
                        )}
                      </div>
                    </div>
                  );
                })}
              </div>
            ))}
          </section>
        );
      })}
    </div>
  );

  return (
    <Popover content={content} trigger="click" placement="bottom" open={open} onOpenChange={setOpen} overlayClassName="device-center-popover">
      <Button className="device-center-trigger" disabled={!backendReady}>
        <span className={`device-phase-dot is-${selected?.session_phase ?? "disconnected"}`} />
        <span className="device-center-trigger-label">{selected?.name ?? t("device.select")}</span>
        {selected && <span className="device-center-trigger-transport">{selected.connection}</span>}
        <span className="device-center-trigger-count">{connectedCount}</span>
      </Button>
    </Popover>
  );
}

function movePriority(priority: string[], udid: string, offset: -1 | 1) {
  const from = priority.indexOf(udid);
  const to = from + offset;
  if (from < 0 || to < 0 || to >= priority.length) return priority;
  const next = [...priority];
  [next[from], next[to]] = [next[to], next[from]];
  return next;
}
