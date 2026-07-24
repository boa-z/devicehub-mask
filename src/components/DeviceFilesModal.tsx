import {
  ArrowLeftOutlined,
  DownloadOutlined,
  FileOutlined,
  FolderOpenOutlined,
  FolderOutlined,
  HomeOutlined,
  ReloadOutlined,
} from "@ant-design/icons";
import { save } from "@tauri-apps/plugin-dialog";
import { Alert, Breadcrumb, Button, Empty, Modal, Spin, Tooltip, Typography, message } from "antd";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { formatFileSize } from "../deviceInspector";
import type { DeviceFileEntry, DeviceFileList } from "../types";

type Request = (path: string, init?: RequestInit) => Promise<Response>;

type Props = {
  open: boolean;
  request: Request;
  onClose: () => void;
};

async function readJson<T>(response: Response): Promise<T> {
  if (!response.ok) throw new Error((await response.text()) || `${response.status} ${response.statusText}`);
  return response.json() as Promise<T>;
}

function parentPath(path: string) {
  const parts = path.split("/").filter(Boolean);
  parts.pop();
  return parts.length ? `/${parts.join("/")}` : "/";
}

export function DeviceFilesModal({ open, request, onClose }: Props) {
  const { t, i18n } = useTranslation();
  const [path, setPath] = useState("/");
  const [listing, setListing] = useState<DeviceFileList | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const requestVersion = useRef(0);

  const load = useCallback(async () => {
    if (!open) return;
    const version = ++requestVersion.current;
    setBusy("list");
    setError(null);
    try {
      const query = new URLSearchParams({ path });
      const nextListing = await readJson<DeviceFileList>(await request(`/api/device/files?${query}`));
      if (requestVersion.current === version) setListing(nextListing);
    } catch (loadError) {
      if (requestVersion.current === version) {
        setListing(null);
        setError(String(loadError));
      }
    } finally {
      if (requestVersion.current === version) setBusy(null);
    }
  }, [open, path, request]);

  useEffect(() => {
    if (!open) {
      requestVersion.current += 1;
      setPath("/");
      setListing(null);
      setError(null);
      setBusy(null);
    }
  }, [open]);

  useEffect(() => {
    void load();
  }, [load]);

  const breadcrumbs = useMemo(() => {
    const parts = path.split("/").filter(Boolean);
    return [
      {
        title: <Button type="text" size="small" icon={<HomeOutlined />} aria-label={t("deviceInspector.deviceFilesRoot")} disabled={busy !== null} onClick={() => setPath("/")} />,
      },
      ...parts.map((part, index) => ({
        title: <Button type="text" size="small" disabled={busy !== null} onClick={() => setPath(`/${parts.slice(0, index + 1).join("/")}`)}>{part}</Button>,
      })),
    ];
  }, [busy, path, t]);

  const exportFile = async (entry: DeviceFileEntry) => {
    if (entry.kind !== "file") return;
    const destination = await save({ defaultPath: entry.name });
    if (!destination) return;
    const version = ++requestVersion.current;
    setBusy(`export:${entry.path}`);
    try {
      const response = await request("/api/device/files/export", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ path: entry.path, destination }),
      });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      const result = await response.json() as { bytes_written: number };
      if (requestVersion.current === version) {
        void message.success(t("deviceInspector.deviceFileExported", { size: formatFileSize(result.bytes_written) }));
      }
    } catch (exportError) {
      if (requestVersion.current === version) {
        void message.error(t("deviceInspector.deviceFileExportFailed", { error: String(exportError) }));
      }
    } finally {
      if (requestVersion.current === version) setBusy(null);
    }
  };

  return (
    <Modal
      className="app-documents-modal"
      open={open}
      width={760}
      title={t("deviceInspector.deviceFilesTitle")}
      footer={null}
      destroyOnHidden
      closable={busy === null}
      keyboard={busy === null}
      maskClosable={busy === null}
      onCancel={() => { if (busy === null) onClose(); }}
    >
      <div className="app-documents-toolbar device-files-toolbar">
        <Tooltip title={t("common.back")}>
          <Button icon={<ArrowLeftOutlined />} aria-label={t("common.back")} disabled={path === "/" || busy !== null} onClick={() => setPath(parentPath(path))} />
        </Tooltip>
        <Breadcrumb items={breadcrumbs} />
        <Tooltip title={t("deviceInspector.refreshDeviceFiles")}>
          <Button icon={<ReloadOutlined />} aria-label={t("deviceInspector.refreshDeviceFiles")} disabled={busy !== null} onClick={() => void load()} />
        </Tooltip>
      </div>
      {error ? (
        <Alert type="error" showIcon message={t("deviceInspector.deviceFilesUnavailable")} description={error} />
      ) : busy === "list" && !listing ? (
        <div className="app-documents-loading"><Spin /></div>
      ) : listing?.entries.length ? (
        <div className="app-document-list" aria-busy={busy !== null}>
          {listing.entries.map((entry) => (
            <div className="app-document-row" key={entry.path}>
              <span className="app-document-kind">{entry.kind === "directory" ? <FolderOutlined /> : <FileOutlined />}</span>
              <button className="app-document-name" disabled={entry.kind !== "directory" || busy !== null} onClick={() => setPath(entry.path)}>
                <Typography.Text ellipsis={{ tooltip: entry.name }}>{entry.name}</Typography.Text>
              </button>
              <Typography.Text type="secondary" className="app-document-size">{entry.kind === "file" ? formatFileSize(entry.size_bytes) : ""}</Typography.Text>
              <Typography.Text type="secondary" className="app-document-date">
                {new Date(entry.modified).toLocaleString(i18n.resolvedLanguage ?? i18n.language)}
              </Typography.Text>
              <div className="app-document-actions">
                {entry.kind === "directory" && (
                  <Tooltip title={t("deviceInspector.openDeviceDirectory")}><Button size="small" icon={<FolderOpenOutlined />} aria-label={t("deviceInspector.openDeviceDirectory")} disabled={busy !== null} onClick={() => setPath(entry.path)} /></Tooltip>
                )}
                {entry.kind === "file" && (
                  <Tooltip title={t("deviceInspector.exportDeviceFile")}><Button size="small" icon={<DownloadOutlined />} aria-label={t("deviceInspector.exportDeviceFile")} loading={busy === `export:${entry.path}`} disabled={busy !== null && busy !== `export:${entry.path}`} onClick={() => void exportFile(entry)} /></Tooltip>
                )}
              </div>
            </div>
          ))}
        </div>
      ) : (
        <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={t("deviceInspector.noDeviceFiles")} />
      )}
      {listing?.truncated && <Alert type="warning" showIcon message={t("deviceInspector.deviceFilesTruncated")} />}
    </Modal>
  );
}
