import { DashboardOutlined } from "@ant-design/icons";
import { Alert, Tag, Typography } from "antd";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { PerformanceSnapshot, PerformanceView, ServiceHealth, StreamMetrics } from "../types";

type Props = {
  activeUdid: string | null;
  streamMetrics: StreamMetrics;
  renderFps: number;
  view: PerformanceView | null;
  error: string | null;
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

export function PerformancePage({ activeUdid, streamMetrics, renderFps, view, error }: Props) {
  const { t } = useTranslation();
  const [history, setHistory] = useState<PerformanceSnapshot[]>([]);

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
