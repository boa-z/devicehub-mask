import {
  CaretRightOutlined,
  ClearOutlined,
  CopyOutlined,
  FileTextOutlined,
  PauseOutlined,
  SearchOutlined,
} from "@ant-design/icons";
import { Alert, Button, Empty, Input, Select, Switch, Tag, Tooltip, Typography, message } from "antd";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { deviceLogContext, filterDeviceLogs, formatDeviceLogLine, type DeviceLogLevelFilter } from "../deviceLogs";
import type { DeviceLogEntry, DeviceLogsView } from "../types";

type Request = (path: string, init?: RequestInit) => Promise<Response>;

type Props = {
  activeUdid: string | null;
  request: Request;
};

const CLIENT_ENTRY_LIMIT = 2_000;

export function DeviceLogsPage({ activeUdid, request }: Props) {
  const { t, i18n } = useTranslation();
  const [entries, setEntries] = useState<DeviceLogEntry[]>([]);
  const [query, setQuery] = useState("");
  const [level, setLevel] = useState<DeviceLogLevelFilter>("all");
  const [paused, setPaused] = useState(false);
  const [autoScroll, setAutoScroll] = useState(true);
  const [view, setView] = useState<DeviceLogsView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [gapDetected, setGapDetected] = useState(false);
  const cursor = useRef<number | null>(null);
  const pollingGeneration = useRef(0);
  const viewport = useRef<HTMLDivElement>(null);
  const timeFormatter = useMemo(() => new Intl.DateTimeFormat(
    i18n.resolvedLanguage ?? i18n.language,
    {
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
      fractionalSecondDigits: 3,
    },
  ), [i18n.language, i18n.resolvedLanguage]);

  useEffect(() => {
    pollingGeneration.current += 1;
    setEntries([]);
    setView(null);
    setError(null);
    setGapDetected(false);
    cursor.current = null;
  }, [activeUdid]);

  useEffect(() => {
    if (!activeUdid || paused) return;
    let disposed = false;
    let loading = false;
    const refresh = async () => {
      if (loading) return;
      loading = true;
      const generation = pollingGeneration.current;
      try {
        const received: DeviceLogEntry[] = [];
        let latestView: DeviceLogsView | null = null;
        let cursorLagged = false;
        for (let page = 0; page < 4; page += 1) {
          const after = cursor.current;
          const path = after === null
            ? "/api/device/logs?limit=500"
            : `/api/device/logs?after=${after}&limit=500`;
          const response = await request(path);
          if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
          const next = await response.json() as DeviceLogsView;
          if (disposed || generation !== pollingGeneration.current) return;
          latestView = next;
          cursorLagged ||= next.cursor_lagged;
          if (next.entries.length > 0) {
            cursor.current = next.entries.at(-1)?.sequence ?? cursor.current;
            received.push(...next.entries);
          } else if (cursor.current === null) {
            cursor.current = next.latest_sequence;
          }
          if (!next.has_more) break;
        }
        if (!disposed && generation === pollingGeneration.current) {
          setView(latestView);
          setError(null);
          setGapDetected((current) => current || cursorLagged);
          if (received.length > 0) {
            setEntries((current) => [...current, ...received].slice(-CLIENT_ENTRY_LIMIT));
          }
        }
      } catch (refreshError) {
        if (!disposed) setError(String(refreshError));
      } finally {
        loading = false;
      }
    };
    void refresh();
    const timer = window.setInterval(() => void refresh(), 500);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [activeUdid, paused, request]);

  const visibleEntries = useMemo(
    () => filterDeviceLogs(entries, query, level),
    [entries, level, query],
  );

  useEffect(() => {
    if (!autoScroll || paused) return;
    const element = viewport.current;
    if (element) element.scrollTop = element.scrollHeight;
  }, [autoScroll, paused, visibleEntries]);

  const clear = async () => {
    try {
      const response = await request("/api/device/logs", { method: "DELETE" });
      if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
      pollingGeneration.current += 1;
      setEntries([]);
      setGapDetected(false);
      cursor.current = null;
    } catch (clearError) {
      void message.error(t("deviceLogs.clearFailed", { error: String(clearError) }));
    }
  };

  const copyVisible = async () => {
    try {
      await navigator.clipboard.writeText(visibleEntries.map((entry) =>
        formatDeviceLogLine(entry, timeFormatter.format(new Date(entry.received_at_ms)))).join("\n"));
      void message.success(t("deviceLogs.copied", { count: visibleEntries.length }));
    } catch (copyError) {
      void message.error(t("deviceLogs.copyFailed", { error: String(copyError) }));
    }
  };

  const phase = view?.service?.phase;
  const statusColor = phase === "ready" ? "success"
    : phase === "connecting" || phase === "recovering" ? "processing"
      : "default";
  const statusLabel = paused ? t("deviceLogs.paused")
    : phase ? t(`performance.phases.${phase}`)
      : activeUdid ? t("deviceLogs.waiting") : t("deviceLogs.disconnected");

  return (
    <main className="device-logs-page">
      <header>
        <div>
          <Typography.Title level={3}><FileTextOutlined />{t("deviceLogs.title")}</Typography.Title>
          <Typography.Text type="secondary">{t("deviceLogs.subtitle")}</Typography.Text>
        </div>
        <div className="device-logs-status">
          {view?.source && <Tag>{t(`deviceLogs.sources.${view.source}`)}</Tag>}
          <Tag color={paused ? "warning" : statusColor}>{statusLabel}</Tag>
        </div>
      </header>

      {!activeUdid && <Alert type="info" showIcon message={t("deviceLogs.connectDevice")} />}
      {error && <Alert type="warning" showIcon message={t("deviceLogs.loadFailed")} description={error} />}
      {view?.service?.last_error && <Alert type="warning" showIcon message={t("deviceLogs.serviceUnavailable")} description={view.service.last_error} />}
      {gapDetected && <Alert type="warning" showIcon closable message={t("deviceLogs.truncated")} onClose={() => setGapDetected(false)} />}

      <section className="device-logs-console">
        <div className="device-logs-toolbar">
          <Input
            allowClear
            value={query}
            prefix={<SearchOutlined />}
            placeholder={t("deviceLogs.search")}
            onChange={(event) => setQuery(event.target.value)}
          />
          <Select<DeviceLogLevelFilter>
            value={level}
            aria-label={t("deviceLogs.levelFilter")}
            options={(["all", "notice", "info", "debug", "error", "fault"] as const).map((value) => ({
              value,
              label: t(`deviceLogs.levels.${value}`),
            }))}
            onChange={setLevel}
          />
          <Tooltip title={t(paused ? "deviceLogs.resume" : "deviceLogs.pause")}>
            <Button
              aria-label={t(paused ? "deviceLogs.resume" : "deviceLogs.pause")}
              icon={paused ? <CaretRightOutlined /> : <PauseOutlined />}
              disabled={!activeUdid}
              onClick={() => setPaused((current) => !current)}
            />
          </Tooltip>
          <Tooltip title={t("deviceLogs.copyVisible")}>
            <Button aria-label={t("deviceLogs.copyVisible")} icon={<CopyOutlined />} disabled={visibleEntries.length === 0} onClick={() => void copyVisible()} />
          </Tooltip>
          <Tooltip title={t("deviceLogs.clear")}>
            <Button aria-label={t("deviceLogs.clear")} icon={<ClearOutlined />} disabled={entries.length === 0} onClick={() => void clear()} />
          </Tooltip>
          <label className="device-logs-autoscroll">
            <span>{t("deviceLogs.autoScroll")}</span>
            <Switch size="small" checked={autoScroll} onChange={setAutoScroll} />
          </label>
          <span className="device-logs-count">{t("deviceLogs.count", { count: visibleEntries.length })}</span>
        </div>
        <div className="device-logs-viewport" ref={viewport}>
          {visibleEntries.map((entry) => (
            <div className="device-log-row" key={entry.sequence}>
              <time>{timeFormatter.format(new Date(entry.received_at_ms))}</time>
              <span className={`device-log-level is-${entry.level ?? "raw"}`}>
                {entry.level ? t(`deviceLogs.levels.${entry.level}`) : t("deviceLogs.levels.raw")}
              </span>
              <span className="device-log-context" title={deviceLogContext(entry)}>
                {deviceLogContext(entry) || "-"}
              </span>
              <pre>{entry.message}</pre>
            </div>
          ))}
          {visibleEntries.length === 0 && <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={t("deviceLogs.empty")} />}
        </div>
      </section>
    </main>
  );
}
