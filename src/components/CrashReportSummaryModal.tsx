import { Alert, Button, Descriptions, Modal, Spin } from "antd";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { DeviceCrashReportSummary } from "../types";

type Request = (path: string, init?: RequestInit) => Promise<Response>;

type Props = {
  open: boolean;
  devicePath: string | null;
  reportName: string | null;
  request: Request;
  onClose: () => void;
};

async function readJson<T>(response: Response): Promise<T> {
  if (!response.ok) throw new Error((await response.text()) || `${response.status} ${response.statusText}`);
  return response.json() as Promise<T>;
}

export function CrashReportSummaryModal({ open, devicePath, reportName, request, onClose }: Props) {
  const { t } = useTranslation();
  const [summary, setSummary] = useState<DeviceCrashReportSummary | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setSummary(null);
    setError(null);
    if (!open || !devicePath) {
      setLoading(false);
      return;
    }
    const controller = new AbortController();
    setLoading(true);
    const query = new URLSearchParams({ device_path: devicePath });
    void request(`/api/device/crash-reports/summary?${query.toString()}`, { signal: controller.signal })
      .then((response) => readJson<DeviceCrashReportSummary>(response))
      .then((result) => {
        if (!controller.signal.aborted) setSummary(result);
      }).catch((loadError) => {
        if (!controller.signal.aborted) setError(String(loadError));
      }).finally(() => {
        if (!controller.signal.aborted) setLoading(false);
      });
    return () => controller.abort();
  }, [devicePath, open, request]);

  const version = summary?.app_version
    ? summary.build_version ? `${summary.app_version} (${summary.build_version})` : summary.app_version
    : summary?.build_version ?? null;
  const items = summary ? [
    ["kind", t(`crashSummary.kinds.${summary.kind}`)],
    ["format", t(`crashSummary.formats.${summary.format}`)],
    ["process", summary.process_name],
    ["bundle", summary.bundle_id],
    ["version", version],
    ["os", summary.os_version],
    ["timestamp", summary.timestamp],
    ["bugType", summary.bug_type],
    ["exception", summary.exception_type],
    ["signal", summary.exception_signal],
    ["terminationNamespace", summary.termination_namespace],
    ["terminationCode", summary.termination_code],
    ["faultingThread", summary.faulting_thread === null ? null : String(summary.faulting_thread)],
  ].filter((item): item is [string, string] => item[1] !== null) : [];

  return (
    <Modal
      open={open}
      title={reportName ? t("crashSummary.titleWithName", { name: reportName }) : t("crashSummary.title")}
      onCancel={onClose}
      footer={<Button onClick={onClose}>{t("common.close")}</Button>}
      width={560}
    >
      {loading ? (
        <div className="crash-summary-loading"><Spin /></div>
      ) : error ? (
        <Alert type="error" showIcon message={t("crashSummary.loadFailed")} description={error} />
      ) : summary ? (
        <div className="crash-summary-content">
          {!summary.details_parsed && <Alert type="info" showIcon message={t("crashSummary.partialDetails")} />}
          {summary.source_truncated && <Alert type="warning" showIcon message={t("crashSummary.sourceTruncated")} />}
          <Descriptions
            size="small"
            bordered
            column={1}
            items={items.map(([key, value]) => ({ key, label: t(`crashSummary.fields.${key}`), children: value }))}
          />
          {items.length === 0 && <Alert type="info" showIcon message={t("crashSummary.noDetails")} />}
        </div>
      ) : null}
    </Modal>
  );
}
