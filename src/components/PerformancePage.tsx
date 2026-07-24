import { DashboardOutlined, DownloadOutlined, ExperimentOutlined, LeftOutlined, ReloadOutlined, RightOutlined, SearchOutlined, StopOutlined } from "@ant-design/icons";
import { save } from "@tauri-apps/plugin-dialog";
import { Alert, Button, Input, Modal, Segmented, Select, Space, Tag, Tooltip, Typography, message } from "antd";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { sortProcesses, type ProcessSort } from "../processPerformance";
import { bluetoothCaptureFilename, networkCaptureDurations, networkCaptureFilename, networkCaptureRunning } from "../networkCapture";
import { decodeDeviceConditionSelection, deviceConditionSelectionExists, encodeDeviceConditionSelection } from "../deviceConditions";
import { filterRunningProcesses } from "../runningProcesses";
import { showErrorMessage } from "../errorMessage";
import type { PerformanceSnapshot, PerformanceView, RunningProcessList, ServiceHealth, StreamMetrics } from "../types";
import { ErrorAlert, ErrorCopyButton } from "./ErrorPresentation";

type Props = {
  activeUdid: string | null;
  streamMetrics: StreamMetrics;
  renderFps: number;
  view: PerformanceView | null;
  error: string | null;
  deviceName: string;
  request: (path: string, init?: RequestInit) => Promise<Response>;
};

const HISTORY_LIMIT = 120;
const PROCESS_PAGE_SIZE = 50;

function number(value: number | null | undefined, digits = 1) {
  return value == null || !Number.isFinite(value) ? "--" : value.toFixed(digits);
}

function bytes(value: number | null | undefined) {
  if (value == null || !Number.isFinite(value)) return "--";
  if (value >= 1024 ** 3) return `${(value / 1024 ** 3).toFixed(2)} GB`;
  return `${(value / 1024 ** 2).toFixed(1)} MB`;
}

function byteRate(value: number | null | undefined) {
  if (value == null || !Number.isFinite(value)) return "--";
  if (value >= 1024 ** 2) return `${(value / 1024 ** 2).toFixed(2)} MB/s`;
  if (value >= 1024) return `${(value / 1024).toFixed(1)} KB/s`;
  return `${value.toFixed(0)} B/s`;
}

function fileSize(value: number | null | undefined) {
  if (value == null || !Number.isFinite(value)) return "--";
  if (value >= 1024 ** 2) return `${(value / 1024 ** 2).toFixed(1)} MB`;
  if (value >= 1024) return `${(value / 1024).toFixed(1)} KB`;
  return `${value.toFixed(0)} B`;
}

function energyScore(value: number | null | undefined) {
  if (value == null || !Number.isFinite(value)) return "--";
  if (value >= 100) return value.toFixed(1);
  if (value >= 10) return value.toFixed(2);
  return value.toFixed(3);
}

function Sparkline({ values, ceiling }: { values: number[]; ceiling?: number }) {
  const points = useMemo(() => {
    if (values.length === 0) return "";
    const maximum = Math.max(ceiling ?? 0, ...values, 1);
    return values.map((value, index) => {
      const x = values.length === 1 ? 100 : index * 100 / (values.length - 1);
      const y = 34 - Math.min(Math.max(value, 0), maximum) / maximum * 30;
      return `${x.toFixed(2)},${y.toFixed(2)}`;
    }).join(" ");
  }, [ceiling, values]);
  return (
    <svg className="performance-sparkline" viewBox="0 0 100 36" preserveAspectRatio="none" aria-hidden="true">
      <line x1="0" y1="34" x2="100" y2="34" />
      {points && <polyline points={points} />}
    </svg>
  );
}

function ServiceRow({ service }: { service: ServiceHealth }) {
  const { t } = useTranslation();
  const color = service.phase === "ready" ? "success"
    : service.phase === "recovering" || service.phase === "connecting" ? "processing"
      : "warning";
  return (
    <div className="performance-service-row">
      <div>
        <Typography.Text>{t(`performance.services.${service.name}`, { defaultValue: service.name })}</Typography.Text>
        {service.last_error && <Typography.Text type="secondary" ellipsis={{ tooltip: service.last_error }}>{service.last_error}</Typography.Text>}
      </div>
      <span>{service.restarts}</span>
      <Tag color={color}>{t(`performance.phases.${service.phase}`)}</Tag>
    </div>
  );
}

export function PerformancePage({ activeUdid, streamMetrics, renderFps, view, error, deviceName, request }: Props) {
  const { t, i18n } = useTranslation();
  const [history, setHistory] = useState<PerformanceSnapshot[]>([]);
  const [processSort, setProcessSort] = useState<ProcessSort>("cpu");
  const [captureDuration, setCaptureDuration] = useState<number>(30);
  const [captureProcessId, setCaptureProcessId] = useState<number | null>(null);
  const [captureBusy, setCaptureBusy] = useState(false);
  const [bluetoothDuration, setBluetoothDuration] = useState<number>(30);
  const [bluetoothBusy, setBluetoothBusy] = useState(false);
  const [conditionSelection, setConditionSelection] = useState<string | null>(null);
  const [conditionBusy, setConditionBusy] = useState(false);
  const [processInventory, setProcessInventory] = useState<RunningProcessList | null>(null);
  const [processInventoryLoading, setProcessInventoryLoading] = useState(false);
  const [processInventoryError, setProcessInventoryError] = useState<string | null>(null);
  const [processQuery, setProcessQuery] = useState("");
  const [processPage, setProcessPage] = useState(1);
  const processRequestSequence = useRef(0);
  const condition = view?.device_conditions;
  const appActivity = useMemo(
    () => [...(view?.app_activity ?? [])].reverse().slice(0, 20),
    [view?.app_activity],
  );

  const loadProcessInventory = useCallback(async () => {
    if (!activeUdid) return;
    const sequence = ++processRequestSequence.current;
    setProcessInventoryLoading(true);
    setProcessInventoryError(null);
    try {
      const response = await request("/api/performance/processes");
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      const inventory = await response.json() as RunningProcessList;
      if (sequence === processRequestSequence.current) {
        setProcessInventory(inventory);
        setProcessPage(1);
      }
    } catch (inventoryError) {
      if (sequence === processRequestSequence.current) setProcessInventoryError(String(inventoryError));
    } finally {
      if (sequence === processRequestSequence.current) setProcessInventoryLoading(false);
    }
  }, [activeUdid, request]);

  useEffect(() => {
    setHistory([]);
    setConditionSelection(null);
    setProcessInventory(null);
    setProcessInventoryError(null);
    setProcessQuery("");
    setProcessPage(1);
    setCaptureProcessId(null);
  }, [activeUdid]);

  useEffect(() => {
    if (activeUdid) void loadProcessInventory();
    return () => {
      processRequestSequence.current += 1;
    };
  }, [activeUdid, loadProcessInventory]);

  useEffect(() => {
    const groups = condition?.groups ?? [];
    setConditionSelection((current) => {
      if (current && deviceConditionSelectionExists(groups, current)) return current;
      if (condition?.active) {
        const active = encodeDeviceConditionSelection({
          groupIdentifier: condition.active.group_identifier,
          profileIdentifier: condition.active.profile_identifier,
        });
        if (deviceConditionSelectionExists(groups, active)) return active;
      }
      const firstGroup = groups.find((group) => group.profiles.length > 0);
      const firstProfile = firstGroup?.profiles[0];
      return firstGroup && firstProfile ? encodeDeviceConditionSelection({
        groupIdentifier: firstGroup.identifier,
        profileIdentifier: firstProfile.identifier,
      }) : null;
    });
  }, [condition]);

  useEffect(() => {
    const sample = view?.sample;
    if (!sample || sample.captured_at_ms <= 0) return;
    setHistory((current) => {
      if (current.at(-1)?.captured_at_ms === sample.captured_at_ms) return current;
      return [...current, sample].slice(-HISTORY_LIMIT);
    });
  }, [view]);

  const sample = view?.sample;
  const cpuHistory = history.flatMap((item) => item.system_cpu_percent == null ? [] : [item.system_cpu_percent]);
  const fpsHistory = history.flatMap((item) => item.graphics_fps == null ? [] : [item.graphics_fps]);
  const processes = useMemo(
    () => sortProcesses(sample?.top_processes ?? [], processSort),
    [processSort, sample?.top_processes],
  );
  const visibleProcessInventory = useMemo(
    () => filterRunningProcesses(processInventory?.processes ?? [], processQuery),
    [processInventory?.processes, processQuery],
  );
  const pagedProcessInventory = useMemo(
    () => visibleProcessInventory.slice((processPage - 1) * PROCESS_PAGE_SIZE, processPage * PROCESS_PAGE_SIZE),
    [processPage, visibleProcessInventory],
  );
  const processPageCount = Math.max(1, Math.ceil(visibleProcessInventory.length / PROCESS_PAGE_SIZE));
  const capture = view?.network_capture;
  const captureIsRunning = capture ? networkCaptureRunning(capture) : false;
  useEffect(() => {
    if (captureProcessId == null || !processInventory || captureIsRunning) return;
    if (!processInventory.processes.some((process) => process.pid === captureProcessId)) {
      setCaptureProcessId(null);
    }
  }, [captureIsRunning, captureProcessId, processInventory]);
  const captureStatus = capture
    ? `${t(`performance.captureStates.${capture.state}`)}${capture.stop_reason ? ` · ${t(`performance.captureReasons.${capture.stop_reason}`)}` : ""}`
    : t("performance.captureStates.idle");
  const captureProcessOptions = useMemo(() => [
    { value: "all" as const, label: t("performance.captureAllProcesses") },
    ...(processInventory?.processes ?? []).map((process) => ({
      value: process.pid,
      label: `${process.app_name ?? process.name} · PID ${process.pid}`,
    })),
  ], [processInventory?.processes, t]);
  const capturedProcessEntry = capture?.process_id == null
    ? undefined
    : processInventory?.processes.find((process) => process.pid === capture.process_id);
  const capturedProcess = capture?.process_id == null
    ? t("performance.captureAllProcesses")
    : capturedProcessEntry?.app_name
      ?? capturedProcessEntry?.name
      ?? t("performance.capturePid", { pid: capture.process_id });
  const bluetoothCapture = view?.bluetooth_capture;
  const bluetoothCaptureIsRunning = bluetoothCapture ? networkCaptureRunning(bluetoothCapture) : false;
  const bluetoothCaptureStatus = bluetoothCapture
    ? `${t(`performance.captureStates.${bluetoothCapture.state}`)}${bluetoothCapture.stop_reason ? ` · ${t(`performance.captureReasons.${bluetoothCapture.stop_reason}`)}` : ""}`
    : t("performance.captureStates.idle");
  const conditionOptions = (condition?.groups ?? []).map((group) => ({
    label: group.identifier,
    options: group.profiles.map((profile) => ({
      value: encodeDeviceConditionSelection({
        groupIdentifier: group.identifier,
        profileIdentifier: profile.identifier,
      }),
      label: (
        <div className="performance-condition-option" title={profile.description || profile.identifier}>
          <strong>{profile.identifier}</strong>
          {profile.description && profile.description !== profile.identifier && <span>{profile.description}</span>}
        </div>
      ),
      title: profile.identifier,
    })),
  }));
  const activeConditionValue = condition?.active ? encodeDeviceConditionSelection({
    groupIdentifier: condition.active.group_identifier,
    profileIdentifier: condition.active.profile_identifier,
  }) : null;

  const startCapture = async () => {
    const destination = await save({
      defaultPath: networkCaptureFilename(deviceName),
      filters: [{ name: "PCAP", extensions: ["pcap"] }],
    });
    if (!destination) return;
    setCaptureBusy(true);
    try {
      const response = await request("/api/performance/network-capture", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ destination, duration_seconds: captureDuration, process_id: captureProcessId }),
      });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      void message.success(t("performance.captureStarted"));
    } catch (captureError) {
      void showErrorMessage(t("performance.captureStartFailed", { error: String(captureError) }));
    } finally {
      setCaptureBusy(false);
    }
  };

  const stopCapture = async () => {
    setCaptureBusy(true);
    try {
      const response = await request("/api/performance/network-capture", { method: "DELETE" });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      void message.success(t("performance.captureSaved"));
    } catch (captureError) {
      void showErrorMessage(t("performance.captureStopFailed", { error: String(captureError) }));
    } finally {
      setCaptureBusy(false);
    }
  };

  const startBluetoothCapture = async () => {
    const destination = await save({
      defaultPath: bluetoothCaptureFilename(deviceName),
      filters: [{ name: "Bluetooth HCI PCAP", extensions: ["pcap"] }],
    });
    if (!destination) return;
    setBluetoothBusy(true);
    try {
      const response = await request("/api/performance/bluetooth-capture", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ destination, duration_seconds: bluetoothDuration }),
      });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      void message.success(t("performance.bluetoothCaptureStarted"));
    } catch (captureError) {
      void showErrorMessage(t("performance.bluetoothCaptureStartFailed", { error: String(captureError) }));
    } finally {
      setBluetoothBusy(false);
    }
  };

  const stopBluetoothCapture = async () => {
    setBluetoothBusy(true);
    try {
      const response = await request("/api/performance/bluetooth-capture", { method: "DELETE" });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      void message.success(t("performance.bluetoothCaptureSaved"));
    } catch (captureError) {
      void showErrorMessage(t("performance.bluetoothCaptureStopFailed", { error: String(captureError) }));
    } finally {
      setBluetoothBusy(false);
    }
  };

  const applyCondition = () => {
    if (!conditionSelection || conditionBusy) return;
    const selected = decodeDeviceConditionSelection(conditionSelection);
    if (!selected) return;
    Modal.confirm({
      title: t("performance.conditionConfirmTitle"),
      content: t("performance.conditionConfirmBody"),
      okText: t("performance.applyCondition"),
      okType: "danger",
      cancelText: t("common.cancel"),
      onOk: async () => {
        setConditionBusy(true);
        try {
          const response = await request("/api/performance/device-condition", {
            method: "PUT",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({
              group_identifier: selected.groupIdentifier,
              profile_identifier: selected.profileIdentifier,
            }),
          });
          if (!response.ok) throw new Error((await response.text()) || response.statusText);
          void message.success(t("performance.conditionApplied"));
        } catch (conditionError) {
          void showErrorMessage(t("performance.conditionApplyFailed", { error: String(conditionError) }));
        } finally {
          setConditionBusy(false);
        }
      },
    });
  };

  const clearCondition = async () => {
    if (conditionBusy) return;
    setConditionBusy(true);
    try {
      const response = await request("/api/performance/device-condition", { method: "DELETE" });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      void message.success(t("performance.conditionCleared"));
    } catch (conditionError) {
      void showErrorMessage(t("performance.conditionClearFailed", { error: String(conditionError) }));
    } finally {
      setConditionBusy(false);
    }
  };

  return (
    <main className="performance-page">
      <header>
        <div>
          <Typography.Title level={3}><DashboardOutlined />{t("performance.title")}</Typography.Title>
          <Typography.Text type="secondary">{t("performance.subtitle")}</Typography.Text>
        </div>
        <Tag color={activeUdid ? "success" : "default"}>{activeUdid ? t("performance.sampling") : t("performance.disconnected")}</Tag>
      </header>

      {!activeUdid && <Alert type="info" showIcon message={t("performance.connectDevice")} />}
      {error && <ErrorAlert type="warning" title={t("performance.loadFailed")} error={error} />}

      <div className="performance-dashboard">
      <section className="performance-section performance-section-device">
        <Typography.Title level={5}>{t("performance.deviceMetrics")}</Typography.Title>
        <div className="performance-metric-grid">
          <div className="performance-metric">
            <span>{t("performance.cpu")}</span><strong>{number(sample?.system_cpu_percent)}%</strong>
            <Sparkline values={cpuHistory} ceiling={100} />
          </div>
          <div className="performance-metric">
            <span>{t("performance.graphicsFps")}</span><strong>{number(sample?.graphics_fps)}</strong>
            <Sparkline values={fpsHistory} ceiling={120} />
          </div>
          <div className="performance-metric"><span>{t("performance.processes")}</span><strong>{sample?.process_count ?? "--"}</strong></div>
          <div className="performance-metric"><span>{t("performance.gpuMemory")}</span><strong>{bytes(sample?.gpu_in_use_bytes)}</strong><small>{t("performance.allocated", { value: bytes(sample?.gpu_allocated_bytes) })}</small></div>
        </div>
        <div className="performance-transport-grid performance-hardware-grid">
          <div><span>{t("performance.logicalCpuCores")}</span><strong>{sample?.logical_cpu_count ?? "--"}</strong></div>
          <div><span>{t("performance.physicalCpuCores")}</span><strong>{sample?.physical_cpu_count ?? "--"}</strong></div>
          <div><span>{t("performance.physicalMemory")}</span><strong>{bytes(sample?.physical_memory_bytes)}</strong></div>
        </div>
      </section>

      <section className="performance-section performance-section-network">
        <Typography.Title level={5}>{t("performance.deviceNetwork")}</Typography.Title>
        <div className="performance-transport-grid">
          <div><span>{t("performance.networkReceive")}</span><strong>{byteRate(sample?.network_rx_bytes_per_second)}</strong></div>
          <div><span>{t("performance.networkSend")}</span><strong>{byteRate(sample?.network_tx_bytes_per_second)}</strong></div>
          <div><span>{t("performance.networkConnections")}</span><strong>{sample?.network_recent_connections ?? "--"}</strong></div>
        </div>
        <div className="performance-network-interfaces">
          <div>
            <span>{t("performance.networkInterfaces")}</span>
            <small>{t("performance.networkInterfacesHint")}</small>
          </div>
          <div className="performance-network-interface-list">
            {(sample?.network_interfaces ?? []).map((networkInterface) => (
              <Tooltip key={networkInterface.name} title={networkInterface.description}>
                <Tag>{networkInterface.name} · {t(`performance.networkInterfaceKinds.${networkInterface.kind}`)}</Tag>
              </Tooltip>
            ))}
            {!sample?.network_interfaces_available && <Typography.Text type="secondary">--</Typography.Text>}
            {sample?.network_interfaces_available && sample.network_interfaces.length === 0 && <Typography.Text type="secondary">{t("performance.noNetworkInterfaces")}</Typography.Text>}
            {sample?.network_interfaces_truncated && <Tag>{t("performance.networkInterfacesTruncated")}</Tag>}
          </div>
        </div>
      </section>

      <section className="performance-section performance-section-wide">
        <div className="performance-condition-header">
          <div>
            <Typography.Title level={5}>{t("performance.deviceConditions")}</Typography.Title>
            <Typography.Text type="secondary">{t("performance.deviceConditionsHint")}</Typography.Text>
          </div>
        </div>
        <div className="performance-condition-controls">
          <Select
            aria-label={t("performance.conditionProfile")}
            value={conditionSelection}
            placeholder={t("performance.selectCondition")}
            options={conditionOptions}
            optionLabelProp="title"
            popupClassName="performance-condition-popup"
            virtual={false}
            disabled={!activeUdid || !condition?.available || conditionBusy || conditionOptions.length === 0}
            onChange={setConditionSelection}
          />
          <Button
            danger
            type="primary"
            icon={<ExperimentOutlined />}
            disabled={!condition?.available || !conditionSelection || conditionSelection === activeConditionValue}
            loading={conditionBusy}
            onClick={applyCondition}
          >
            {t("performance.applyCondition")}
          </Button>
          <Button
            icon={<StopOutlined />}
            disabled={!condition?.available || !condition.active}
            loading={conditionBusy}
            onClick={() => void clearCondition()}
          >
            {t("performance.clearCondition")}
          </Button>
        </div>
        {condition?.active && <Alert
          type="warning"
          showIcon
          message={t("performance.conditionActive")}
          description={condition.active.description || condition.active.profile_identifier}
        />}
        {condition?.cleanup_pending && !condition.error && <Alert
          type="error"
          showIcon
          message={t("performance.conditionCleanupPending")}
          description={t("performance.conditionCleanupPendingHint")}
        />}
        {activeUdid && condition && !condition.available && !condition.cleanup_pending && <Alert
          type="info"
          showIcon
          message={t("performance.conditionsUnavailable")}
          description={condition.error ?? t("performance.conditionsUnavailableHint")}
          action={condition.error ? <ErrorCopyButton error={condition.error} /> : undefined}
        />}
        {condition?.error && condition.cleanup_pending && (
          <ErrorAlert title={t("performance.conditionCleanupPending")} error={condition.error} />
        )}
      </section>

      <section className="performance-section performance-section-wide">
        <div className="performance-process-header">
          <div>
            <Typography.Title level={5}>{t("performance.appActivity")}</Typography.Title>
            <Typography.Text type="secondary">{t("performance.appActivityHint")}</Typography.Text>
          </div>
        </div>
        <div className="performance-process-table-wrap">
          <table className="performance-process-table performance-activity-table">
            <colgroup><col /><col /><col /><col /></colgroup>
            <thead><tr>
              <th>{t("performance.eventTime")}</th>
              <th>{t("performance.activityApp")}</th>
              <th>{t("performance.activityState")}</th>
              <th>{t("performance.pid")}</th>
            </tr></thead>
            <tbody>
              {appActivity.map((event) => <tr key={event.sequence}>
                <td>{new Date(event.received_at_ms).toLocaleTimeString(i18n.resolvedLanguage ?? i18n.language)}</td>
                <td><span title={event.exec_name ?? event.app_name ?? undefined}>{event.app_name ?? event.exec_name ?? t("performance.unknownApp")}</span></td>
                <td><span title={event.notification_type}>{event.state_description ?? event.notification_type}</span></td>
                <td>{event.pid ?? "-"}</td>
              </tr>)}
              {appActivity.length === 0 && <tr className="performance-process-empty"><td colSpan={4}>{t("performance.waitingAppActivity")}</td></tr>}
            </tbody>
          </table>
        </div>
      </section>

      <section className="performance-section performance-section-capture">
        <div className="performance-process-header">
          <div>
            <Typography.Title level={5}>{t("performance.packetCapture")}</Typography.Title>
            <Typography.Text type="secondary">{t("performance.packetCaptureHint")}</Typography.Text>
          </div>
          <Space wrap className="performance-capture-controls">
            <Select<number | "all">
              className="performance-capture-process-select"
              aria-label={t("performance.captureProcess")}
              value={captureProcessId ?? "all"}
              disabled={!activeUdid || captureIsRunning || captureBusy}
              loading={processInventoryLoading}
              showSearch
              optionFilterProp="label"
              options={captureProcessOptions}
              onChange={(value) => setCaptureProcessId(value === "all" ? null : value)}
            />
            <Select<number>
              aria-label={t("performance.captureDuration")}
              value={captureDuration}
              disabled={!activeUdid || captureIsRunning || captureBusy}
              options={networkCaptureDurations.map((seconds) => ({
                value: seconds,
                label: t("performance.captureSeconds", { count: seconds }),
              }))}
              onChange={setCaptureDuration}
            />
            {captureIsRunning ? (
              <Button danger icon={<StopOutlined />} loading={captureBusy} onClick={() => void stopCapture()}>
                {t("performance.stopCapture")}
              </Button>
            ) : (
              <Button type="primary" icon={<DownloadOutlined />} disabled={!activeUdid} loading={captureBusy} onClick={() => void startCapture()}>
                {t("performance.startCapture")}
              </Button>
            )}
          </Space>
        </div>
        <div className="performance-transport-grid performance-capture-grid">
          <div><span>{t("performance.captureStatus")}</span><strong>{captureStatus}</strong></div>
          <div><span>{t("performance.captureProcess")}</span><strong title={capturedProcess}>{capturedProcess}</strong></div>
          <div><span>{t("performance.capturePackets")}</span><strong>{capture?.packet_count ?? 0}</strong></div>
          <div><span>{t("performance.captureFilteredPackets")}</span><strong>{capture?.filtered_packet_count ?? 0}</strong></div>
          <div><span>{t("performance.captureSize")}</span><strong>{fileSize(capture?.bytes_written)}</strong></div>
          <div><span>{t("performance.captureElapsed")}</span><strong>{((capture?.elapsed_ms ?? 0) / 1000).toFixed(1)} s</strong></div>
        </div>
        {capture?.error && <ErrorAlert type="warning" title={t("performance.captureFailed")} error={capture.error} />}
      </section>

      <section className="performance-section performance-section-capture">
        <div className="performance-process-header">
          <div>
            <Typography.Title level={5}>{t("performance.bluetoothCapture")}</Typography.Title>
            <Typography.Text type="secondary">{t("performance.bluetoothCaptureHint")}</Typography.Text>
          </div>
          <Space wrap className="performance-capture-controls">
            <Select<number>
              aria-label={t("performance.captureDuration")}
              value={bluetoothDuration}
              disabled={!activeUdid || bluetoothCaptureIsRunning || bluetoothBusy}
              options={networkCaptureDurations.map((seconds) => ({
                value: seconds,
                label: t("performance.captureSeconds", { count: seconds }),
              }))}
              onChange={setBluetoothDuration}
            />
            {bluetoothCaptureIsRunning ? (
              <Button danger icon={<StopOutlined />} loading={bluetoothBusy} onClick={() => void stopBluetoothCapture()}>
                {t("performance.stopBluetoothCapture")}
              </Button>
            ) : (
              <Button type="primary" icon={<DownloadOutlined />} disabled={!activeUdid} loading={bluetoothBusy} onClick={() => void startBluetoothCapture()}>
                {t("performance.startBluetoothCapture")}
              </Button>
            )}
          </Space>
        </div>
        <div className="performance-transport-grid performance-capture-grid">
          <div><span>{t("performance.captureStatus")}</span><strong>{bluetoothCaptureStatus}</strong></div>
          <div><span>{t("performance.capturePackets")}</span><strong>{bluetoothCapture?.packet_count ?? 0}</strong></div>
          <div><span>{t("performance.captureSize")}</span><strong>{fileSize(bluetoothCapture?.bytes_written)}</strong></div>
          <div><span>{t("performance.captureElapsed")}</span><strong>{((bluetoothCapture?.elapsed_ms ?? 0) / 1000).toFixed(1)} s</strong></div>
        </div>
        {bluetoothCapture?.error && <ErrorAlert type="warning" title={t("performance.bluetoothCaptureFailed")} error={bluetoothCapture.error} />}
      </section>

      <section className="performance-section performance-section-process">
        <div className="performance-process-header">
          <div>
            <Typography.Title level={5}>{t("performance.runningProcesses")}</Typography.Title>
            <Typography.Text type="secondary">{t("performance.runningProcessesHint")}</Typography.Text>
          </div>
          <Space wrap className="performance-process-inventory-controls">
            <Input
              allowClear
              aria-label={t("performance.searchProcesses")}
              prefix={<SearchOutlined />}
              placeholder={t("performance.searchProcesses")}
              value={processQuery}
              onChange={(event) => {
                setProcessQuery(event.target.value);
                setProcessPage(1);
              }}
            />
            <Tooltip title={t("performance.refreshProcesses")}>
              <Button
                aria-label={t("performance.refreshProcesses")}
                icon={<ReloadOutlined />}
                disabled={!activeUdid}
                loading={processInventoryLoading}
                onClick={() => void loadProcessInventory()}
              />
            </Tooltip>
          </Space>
        </div>
        {processInventoryError && <ErrorAlert type="warning" title={t("performance.processInventoryUnavailable")} error={processInventoryError} />}
        {processInventory?.truncated && <Alert type="info" showIcon message={t("performance.processInventoryTruncated")} />}
        <div className="performance-process-table-wrap">
          <table className="performance-process-table performance-inventory-table">
            <colgroup><col /><col /><col /></colgroup>
            <thead><tr>
              <th>{t("performance.processName")}</th>
              <th>{t("performance.pid")}</th>
              <th>{t("performance.processType")}</th>
            </tr></thead>
            <tbody>
              {pagedProcessInventory.map((process) => <tr key={process.pid}>
                <td>
                  <span title={process.app_name ? `${process.app_name} · ${process.name}` : process.name}>{process.app_name ?? process.name}</span>
                  {process.app_name && <small>{process.name}</small>}
                </td>
                <td>{process.pid}</td>
                <td><Tag color={process.is_application ? "success" : "default"}>{t(process.is_application ? "performance.applicationProcess" : "performance.systemProcess")}</Tag></td>
              </tr>)}
              {!processInventoryLoading && visibleProcessInventory.length === 0 && <tr className="performance-process-empty"><td colSpan={3}>{t(processQuery ? "performance.noMatchingProcesses" : "performance.noRunningProcesses")}</td></tr>}
              {processInventoryLoading && !processInventory && <tr className="performance-process-empty"><td colSpan={3}>{t("performance.loadingProcesses")}</td></tr>}
            </tbody>
          </table>
        </div>
        {visibleProcessInventory.length > PROCESS_PAGE_SIZE && <div className="performance-process-pagination">
          <Tooltip title={t("performance.previousProcessPage")}>
            <Button
              aria-label={t("performance.previousProcessPage")}
              icon={<LeftOutlined />}
              disabled={processPage <= 1}
              onClick={() => setProcessPage((current) => Math.max(1, current - 1))}
            />
          </Tooltip>
          <span>{t("performance.processPage", { current: processPage, total: processPageCount })}</span>
          <Tooltip title={t("performance.nextProcessPage")}>
            <Button
              aria-label={t("performance.nextProcessPage")}
              icon={<RightOutlined />}
              disabled={processPage >= processPageCount}
              onClick={() => setProcessPage((current) => Math.min(processPageCount, current + 1))}
            />
          </Tooltip>
        </div>}
      </section>

      <section className="performance-section performance-section-process">
        <div className="performance-process-header">
          <div>
            <Typography.Title level={5}>{t("performance.topProcesses")}</Typography.Title>
            <Typography.Text type="secondary">
              {sample?.logical_cpu_count
                ? t("performance.processCpuNormalized", { count: sample.logical_cpu_count })
                : t("performance.processCpuNormalizedUnknown")}
            </Typography.Text>
          </div>
          <Segmented<ProcessSort>
            aria-label={t("performance.processSort")}
            value={processSort}
            options={[
              { value: "cpu", label: t("performance.sortCpu") },
              { value: "memory", label: t("performance.sortMemory") },
            ]}
            onChange={setProcessSort}
          />
        </div>
        <div className="performance-process-table-wrap">
          <table className="performance-process-table">
            <colgroup><col /><col /><col /><col /></colgroup>
            <thead><tr>
              <th>{t("performance.processName")}</th>
              <th>{t("performance.pid")}</th>
              <th>{t("performance.processCpu")}</th>
              <th>{t("performance.physicalFootprint")}</th>
            </tr></thead>
            <tbody>
              {processes.map((process) => <tr key={process.pid}>
                <td><span title={process.name}>{process.name}</span></td>
                <td>{process.pid}</td>
                <td>{process.cpu_percent == null ? "--" : `${number(process.cpu_percent)}%`}</td>
                <td>{bytes(process.memory_bytes)}</td>
              </tr>)}
              {processes.length === 0 && <tr className="performance-process-empty"><td colSpan={4}>{t("performance.waitingProcesses")}</td></tr>}
            </tbody>
          </table>
        </div>
      </section>

      <section className="performance-section performance-section-process">
        <div className="performance-process-header">
          <div>
            <Typography.Title level={5}>{t("performance.processEnergy")}</Typography.Title>
            <Typography.Text type="secondary">{t("performance.processEnergyHint")}</Typography.Text>
          </div>
        </div>
        <div className="performance-process-table-wrap">
          <table className="performance-process-table performance-energy-table">
            <colgroup><col /><col /><col /><col /><col /><col /></colgroup>
            <thead><tr>
              <th>{t("performance.processName")}</th>
              <th>{t("performance.pid")}</th>
              <th>{t("performance.energyTotal")}</th>
              <th>{t("performance.energyCpu")}</th>
              <th>{t("performance.energyGpu")}</th>
              <th>{t("performance.energyNetwork")}</th>
            </tr></thead>
            <tbody>
              {(sample?.energy_processes ?? []).map((process) => <tr key={process.pid}>
                <td><span title={process.name}>{process.name}</span></td>
                <td>{process.pid}</td>
                <td>{energyScore(process.total_score)}</td>
                <td>{energyScore(process.cpu_score)}</td>
                <td>{energyScore(process.gpu_score)}</td>
                <td>{energyScore(process.networking_score)}</td>
              </tr>)}
              {(sample?.energy_processes?.length ?? 0) === 0 && <tr className="performance-process-empty"><td colSpan={6}>{t("performance.waitingEnergy")}</td></tr>}
            </tbody>
          </table>
        </div>
      </section>

      <section className="performance-section performance-section-support">
        <Typography.Title level={5}>{t("performance.transportMetrics")}</Typography.Title>
        <div className="performance-transport-grid">
          <div><span>{t("performance.sourceFps")}</span><strong>{number(streamMetrics.source_fps)}</strong></div>
          <div><span>{t("performance.decodedFps")}</span><strong>{number(streamMetrics.decoded_fps)}</strong></div>
          <div><span>{t("performance.presentedFps")}</span><strong>{number(renderFps)}</strong></div>
          <div><span>{t("performance.bandwidth")}</span><strong>{number(streamMetrics.megabits_per_second, 2)} Mbps</strong></div>
          <div><span>{t("performance.jpegEncode")}</span><strong>{number(streamMetrics.jpeg_encode_ms, 2)} ms</strong></div>
          <div><span>{t("performance.frameAge")}</span><strong>{number(streamMetrics.frame_age_ms, 2)} ms</strong></div>
        </div>
      </section>

      <section className="performance-section performance-section-support">
        <div className="performance-section-title"><Typography.Title level={5}>{t("performance.serviceHealth")}</Typography.Title><span>{t("performance.restarts")}</span></div>
        <div className="performance-service-list">
          {(view?.services ?? []).map((service) => <ServiceRow key={service.name} service={service} />)}
          {activeUdid && view?.services.length === 0 && <Typography.Text type="secondary">{t("performance.waitingServices")}</Typography.Text>}
        </div>
      </section>
      </div>
    </main>
  );
}
