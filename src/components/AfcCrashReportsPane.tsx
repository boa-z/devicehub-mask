import { DownloadOutlined, FileTextOutlined, ReloadOutlined, SearchOutlined } from "@ant-design/icons";
import { save } from "@tauri-apps/plugin-dialog";
import { Alert, Button, Empty, Input, Spin, Tooltip, Typography, message } from "antd";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { filterCrashReports, formatFileSize, formatReportDate } from "../deviceInspector";
import type { DeviceCrashReport, DeviceCrashReportList } from "../types";

type Request = (path: string, init?: RequestInit) => Promise<Response>;

type Props = {
  active: boolean;
  deviceId: string | null;
  request: Request;
  onTransferStateChange?: (active: boolean) => void;
};

async function readJson<T>(response: Response): Promise<T> {
  if (!response.ok) throw new Error((await response.text()) || `${response.status} ${response.statusText}`);
  return response.json() as Promise<T>;
}

export function AfcCrashReportsPane({ active, deviceId, request, onTransferStateChange }: Props) {
  const { t, i18n } = useTranslation();
  const [reports, setReports] = useState<DeviceCrashReport[]>([]);
  const [truncated, setTruncated] = useState(false);
  const [query, setQuery] = useState("");
  const [loading, setLoading] = useState(false);
  const [exporting, setExporting] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const version = useRef(0);

  useEffect(() => {
    onTransferStateChange?.(exporting !== null);
    return () => onTransferStateChange?.(false);
  }, [exporting, onTransferStateChange]);

  const load = useCallback(async () => {
    if (!active || !deviceId) return;
    const requestVersion = ++version.current;
    setLoading(true);
    setError(null);
    try {
      const result = await readJson<DeviceCrashReportList>(await request("/api/device/crash-reports"));
      if (version.current === requestVersion) {
        setReports(result.reports);
        setTruncated(result.truncated);
      }
    } catch (loadError) {
      if (version.current === requestVersion) setError(String(loadError));
    } finally {
      if (version.current === requestVersion) setLoading(false);
    }
  }, [active, deviceId, request]);

  useEffect(() => {
    version.current += 1;
    setReports([]);
    setTruncated(false);
    setQuery("");
    setError(null);
    setLoading(false);
    setExporting(null);
  }, [deviceId]);

  useEffect(() => {
    void load();
  }, [load]);

  const visibleReports = useMemo(() => filterCrashReports(reports, query), [query, reports]);

  const exportReport = async (report: DeviceCrashReport) => {
    const destination = await save({ defaultPath: report.name });
    if (!destination) return;
    setExporting(report.path);
    try {
      const response = await request("/api/device/crash-reports/export", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ path: report.path, destination }),
      });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      void message.success(t("afc.crashExported"));
    } catch (exportError) {
      void message.error(t("afc.crashExportFailed", { error: String(exportError) }));
    } finally {
      setExporting(null);
    }
  };

  return (
    <div className="afc-crash-pane">
      <div className="afc-crash-toolbar">
        <Input
          allowClear
          prefix={<SearchOutlined />}
          value={query}
          placeholder={t("afc.searchCrashReports")}
          disabled={loading}
          onChange={(event) => setQuery(event.target.value)}
        />
        <Tooltip title={t("afc.refreshCrashReports")}>
          <Button icon={<ReloadOutlined />} aria-label={t("afc.refreshCrashReports")} disabled={loading || exporting !== null} onClick={() => void load()} />
        </Tooltip>
      </div>
      {error ? (
        <Alert type="error" showIcon message={t("afc.crashReportsUnavailable")} description={error} />
      ) : loading && reports.length === 0 ? (
        <div className="app-documents-loading"><Spin /></div>
      ) : visibleReports.length > 0 ? (
        <div className="app-document-list" aria-busy={loading || exporting !== null}>
          {visibleReports.map((report) => (
            <div className="afc-crash-row" key={report.path}>
              <span className="app-document-kind"><FileTextOutlined /></span>
              <div className="afc-crash-name">
                <Typography.Text ellipsis={{ tooltip: report.name }}>{report.name}</Typography.Text>
                <Typography.Text type="secondary" ellipsis={{ tooltip: report.path }}>{report.path}</Typography.Text>
              </div>
              <Typography.Text type="secondary" className="app-document-size">{formatFileSize(report.size_bytes)}</Typography.Text>
              <Typography.Text type="secondary" className="app-document-date">
                {formatReportDate(report.modified, i18n.resolvedLanguage ?? i18n.language)}
              </Typography.Text>
              <div className="app-document-actions">
                <Tooltip title={t("afc.exportCrashReport")}>
                  <Button
                    size="small"
                    icon={<DownloadOutlined />}
                    aria-label={t("afc.exportCrashReport")}
                    loading={exporting === report.path}
                    disabled={exporting !== null && exporting !== report.path}
                    onClick={() => void exportReport(report)}
                  />
                </Tooltip>
              </div>
            </div>
          ))}
        </div>
      ) : (
        <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={t("afc.noCrashReports")} />
      )}
      {truncated && <Alert type="warning" showIcon message={t("afc.crashReportsTruncated")} />}
    </div>
  );
}
