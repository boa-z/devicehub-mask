import { useTranslation } from "react-i18next";
import type { PerformanceHudItem } from "../performanceHudPreferences";
import type { PerformanceView, StreamMetrics } from "../types";

type Props = {
  items: PerformanceHudItem[];
  view: PerformanceView | null;
  streamMetrics: StreamMetrics;
  renderFps: number;
};

function number(value: number | null | undefined, digits = 1) {
  return value == null || !Number.isFinite(value) ? "--" : value.toFixed(digits);
}

function bytes(value: number | null | undefined) {
  if (value == null || !Number.isFinite(value)) return "--";
  if (value >= 1024 ** 3) return `${(value / 1024 ** 3).toFixed(2)} GB`;
  return `${(value / 1024 ** 2).toFixed(1)} MB`;
}

export function PerformanceHud({ items, view, streamMetrics, renderFps }: Props) {
  const { t } = useTranslation();
  const sample = view?.sample;
  const values: Record<PerformanceHudItem, string> = {
    system_cpu: `${number(sample?.system_cpu_percent)}%`,
    graphics_fps: number(sample?.graphics_fps),
    process_count: sample?.process_count == null ? "--" : String(sample.process_count),
    gpu_memory: bytes(sample?.gpu_in_use_bytes),
    source_fps: number(streamMetrics.source_fps),
    decoded_fps: number(streamMetrics.decoded_fps),
    presented_fps: number(renderFps),
    bandwidth: `${number(streamMetrics.megabits_per_second, 2)} Mbps`,
    jpeg_encode: `${number(streamMetrics.jpeg_encode_ms, 2)} ms`,
    frame_age: `${number(streamMetrics.frame_age_ms, 2)} ms`,
  };

  if (items.length === 0) return null;
  return (
    <aside className="performance-hud" aria-label={t("performance.hud.label")}>
      {items.map((item) => (
        <div key={item}>
          <span>{t(`performance.hud.items.${item}`)}</span>
          <strong>{values[item]}</strong>
        </div>
      ))}
    </aside>
  );
}
