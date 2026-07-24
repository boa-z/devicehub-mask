import { ClearOutlined, ReloadOutlined, StopOutlined } from "@ant-design/icons";
import { Button, Input, Modal, Spin, Tag, Tooltip, Typography } from "antd";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { mergeConsoleLines } from "../appConsole";
import type { AppConsoleLine, AppConsoleSnapshot, DeviceApp } from "../types";
import { ErrorAlert } from "./ErrorPresentation";

type Request = (path: string, init?: RequestInit) => Promise<Response>;

type Props = {
  app: DeviceApp | null;
  request: Request;
  onClose: () => void;
};

async function readSnapshot(response: Response): Promise<AppConsoleSnapshot> {
  if (!response.ok) throw new Error((await response.text()) || response.statusText);
  return response.json() as Promise<AppConsoleSnapshot>;
}

export function AppConsoleModal({ app, request, onClose }: Props) {
  const { t } = useTranslation();
  const [snapshot, setSnapshot] = useState<AppConsoleSnapshot | null>(null);
  const [lines, setLines] = useState<AppConsoleLine[]>([]);
  const [filter, setFilter] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const cursor = useRef(0);
  const generation = useRef(0);
  const startedBundle = useRef<string | null>(null);
  const output = useRef<HTMLDivElement>(null);

  const applySnapshot = useCallback((next: AppConsoleSnapshot, replace = false) => {
    setSnapshot(next);
    setLines((current) => replace ? next.lines : mergeConsoleLines(current, next));
    cursor.current = Math.max(0, next.next_sequence - 1);
    setError(next.last_error);
  }, []);

  const start = useCallback(async () => {
    if (!app) return;
    const requestGeneration = ++generation.current;
    setBusy(true);
    setError(null);
    setLines([]);
    cursor.current = 0;
    try {
      const next = await readSnapshot(await request(
        `/api/device/apps/${encodeURIComponent(app.bundle_id)}/console`,
        { method: "PUT" },
      ));
      if (requestGeneration !== generation.current) return;
      applySnapshot(next, true);
    } catch (startError) {
      if (requestGeneration === generation.current) setError(String(startError));
    } finally {
      if (requestGeneration === generation.current) setBusy(false);
    }
  }, [app, applySnapshot, request]);

  useEffect(() => {
    if (!app) {
      generation.current += 1;
      startedBundle.current = null;
      setSnapshot(null);
      setLines([]);
      setError(null);
      setBusy(false);
      return;
    }
    if (startedBundle.current === app.bundle_id) return;
    startedBundle.current = app.bundle_id;
    void start();
  }, [app, start]);

  useEffect(() => {
    if (!app || snapshot?.phase !== "running") return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const poll = async () => {
      try {
        const next = await readSnapshot(await request(`/api/device/app-console?after=${cursor.current}`));
        if (cancelled) return;
        applySnapshot(next);
        if (next.phase === "running") timer = setTimeout(poll, 400);
      } catch (pollError) {
        if (!cancelled) setError(String(pollError));
      }
    };
    timer = setTimeout(poll, 200);
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [app, applySnapshot, request, snapshot?.phase]);

  useEffect(() => {
    const element = output.current;
    if (element) element.scrollTop = element.scrollHeight;
  }, [lines]);

  const stop = async () => {
    const requestGeneration = generation.current;
    setBusy(true);
    try {
      const next = await readSnapshot(await request("/api/device/app-console", { method: "DELETE" }));
      if (requestGeneration === generation.current) applySnapshot(next);
    } catch (stopError) {
      if (requestGeneration === generation.current) setError(String(stopError));
    } finally {
      if (requestGeneration === generation.current) setBusy(false);
    }
  };

  const close = () => {
    generation.current += 1;
    if (app) {
      void request("/api/device/app-console?clear=true", { method: "DELETE" });
    }
    startedBundle.current = null;
    onClose();
  };

  const visibleLines = useMemo(() => {
    const query = filter.toLocaleLowerCase();
    return query
      ? lines.filter((line) => line.text.toLocaleLowerCase().includes(query))
      : lines;
  }, [filter, lines]);
  const phase = snapshot?.phase ?? (busy ? "starting" : "stopped");

  return (
    <Modal
      className="app-console-modal"
      title={app ? t("deviceInspector.appConsoleTitle", { name: app.name }) : ""}
      open={app !== null}
      width={860}
      footer={null}
      onCancel={close}
    >
      <div className="app-console-toolbar">
        <Tag color={phase === "running" ? "success" : phase === "failed" ? "error" : "default"}>
          {t(`deviceInspector.appConsoleStates.${phase}`)}
        </Tag>
        <Input
          allowClear
          value={filter}
          placeholder={t("deviceInspector.filterAppConsole")}
          onChange={(event) => setFilter(event.target.value)}
        />
        <Tooltip title={t("deviceInspector.clearAppConsole")}>
          <Button aria-label={t("deviceInspector.clearAppConsole")} icon={<ClearOutlined />} onClick={() => setLines([])} />
        </Tooltip>
        <Tooltip title={t("deviceInspector.relaunchAppConsole")}>
          <Button aria-label={t("deviceInspector.relaunchAppConsole")} icon={<ReloadOutlined />} loading={busy && phase !== "running"} disabled={busy} onClick={() => void start()} />
        </Tooltip>
        <Tooltip title={t("deviceInspector.stopAppConsole")}>
          <Button danger aria-label={t("deviceInspector.stopAppConsole")} icon={<StopOutlined />} loading={busy && phase === "running"} disabled={busy || phase !== "running"} onClick={() => void stop()} />
        </Tooltip>
      </div>
      {error && <ErrorAlert title={t("deviceInspector.appConsoleFailed")} error={error} />}
      <div className="app-console-output" ref={output} role="log" aria-live="polite">
        {busy && lines.length === 0 ? <Spin size="small" /> : visibleLines.map((line) => (
          <div className="app-console-line" key={line.sequence}>{line.text || " "}</div>
        ))}
        {!busy && visibleLines.length === 0 && <Typography.Text type="secondary">{t("deviceInspector.noAppConsoleOutput")}</Typography.Text>}
      </div>
      <div className="app-console-summary">
        <Typography.Text type="secondary">
          {t("deviceInspector.appConsoleSummary", {
            lines: snapshot?.total_lines ?? 0,
            bytes: snapshot?.total_bytes ?? 0,
            dropped: snapshot?.dropped_lines ?? 0,
          })}
        </Typography.Text>
      </div>
    </Modal>
  );
}
