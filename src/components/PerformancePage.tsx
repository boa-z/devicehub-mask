import { DashboardOutlined, DownloadOutlined, StopOutlined } from "@ant-design/icons";
import { save } from "@tauri-apps/plugin-dialog";
import { Alert, Button, Segmented, Select, Space, Tag, Typography, message } from "antd";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { sortProcesses, type ProcessSort } from "../processPerformance";
import { networkCaptureDurations, networkCaptureFilename, networkCaptureRunning } from "../networkCapture";
import type { PerformanceSnapshot, PerformanceView, ServiceHealth, StreamMetrics } from "../types";

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
  const { t } = useTranslation();
  const [history, setHistory] = useState<PerformanceSnapshot[]>([]);
  const [processSort, setProcessSort] = useState<ProcessSort>("cpu");
  const [captureDuration, setCaptureDuration] = useState<number>(30);
  const [captureBusy, setCaptureBusy] = useState(false);

  useEffect(() => {
    setHistory([]);
  }, [activeUdid]);

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
  const capture = view?.network_capture;
  const captureIsRunning = capture ? networkCaptureRunning(capture) : false;
  const captureStatus = capture
    ? `${t(`performance.captureStates.${capture.state}`)}${capture.stop_reason ? ` · ${t(`performance.captureReasons.${capture.stop_reason}`)}` : ""}`
    : t("performance.captureStates.idle");

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
        body: JSON.stringify({ destination, duration_seconds: captureDuration }),
      });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      void message.success(t("performance.captureStarted"));
    } catch (captureError) {
      void message.error(t("performance.captureStartFailed", { error: String(captureError) }));
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
      void message.error(t("performance.captureStopFailed", { error: String(captureError) }));
    } finally {
      setCaptureBusy(false);
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
      {error && <Alert type="warning" showIcon message={t("performance.loadFailed")} description={error} />}

      <section className="performance-section">
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
      </section>

      <section className="performance-section">
        <Typography.Title level={5}>{t("performance.deviceNetwork")}</Typography.Title>
        <div className="performance-transport-grid">
          <div><span>{t("performance.networkReceive")}</span><strong>{byteRate(sample?.network_rx_bytes_per_second)}</strong></div>
          <div><span>{t("performance.networkSend")}</span><strong>{byteRate(sample?.network_tx_bytes_per_second)}</strong></div>
          <div><span>{t("performance.networkConnections")}</span><strong>{sample?.network_recent_connections ?? "--"}</strong></div>
        </div>
      </section>

      <section className="performance-section">
        <div className="performance-process-header">
          <div>
            <Typography.Title level={5}>{t("performance.packetCapture")}</Typography.Title>
            <Typography.Text type="secondary">{t("performance.packetCaptureHint")}</Typography.Text>
          </div>
          <Space wrap className="performance-capture-controls">
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
          <div><span>{t("performance.capturePackets")}</span><strong>{capture?.packet_count ?? 0}</strong></div>
          <div><span>{t("performance.captureSize")}</span><strong>{fileSize(capture?.bytes_written)}</strong></div>
          <div><span>{t("performance.captureElapsed")}</span><strong>{((capture?.elapsed_ms ?? 0) / 1000).toFixed(1)} s</strong></div>
        </div>
        {capture?.error && <Alert type="warning" showIcon message={t("performance.captureFailed")} description={capture.error} />}
      </section>

      <section className="performance-section">
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

      <section className="performance-section">
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

      <section className="performance-section">
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

      <section className="performance-section">
        <div className="performance-section-title"><Typography.Title level={5}>{t("performance.serviceHealth")}</Typography.Title><span>{t("performance.restarts")}</span></div>
        <div className="performance-service-list">
          {(view?.services ?? []).map((service) => <ServiceRow key={service.name} service={service} />)}
          {activeUdid && view?.services.length === 0 && <Typography.Text type="secondary">{t("performance.waitingServices")}</Typography.Text>}
        </div>
      </section>
    </main>
  );
}
