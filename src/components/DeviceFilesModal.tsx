import {
  ArrowLeftOutlined,
  DeleteOutlined,
  DownloadOutlined,
  EditOutlined,
  FileAddOutlined,
  FileOutlined,
  FolderAddOutlined,
  FolderOpenOutlined,
  FolderOutlined,
  HomeOutlined,
  ReloadOutlined,
  UploadOutlined,
} from "@ant-design/icons";
import { open as openDialog, save } from "@tauri-apps/plugin-dialog";
import { Alert, Breadcrumb, Button, Dropdown, Empty, Input, Modal, Spin, Tooltip, Typography, message } from "antd";
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

  const mutate = async (operation: string, call: () => Promise<Response>, success: string, refresh = true) => {
    const version = ++requestVersion.current;
    setBusy(operation);
    try {
      const response = await call();
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      if (requestVersion.current === version) void message.success(success);
      if (refresh && requestVersion.current === version) await load();
      return true;
    } catch (mutationError) {
      if (requestVersion.current === version) {
        void message.error(t("deviceInspector.deviceFileOperationFailed", { error: String(mutationError) }));
      }
      return false;
    } finally {
      if (requestVersion.current === version) setBusy(null);
    }
  };

  const importPath = async (directory: boolean) => {
    const source = await openDialog({ multiple: false, directory });
    if (!source || Array.isArray(source)) return;
    await mutate(
      "import",
      () => request("/api/device/files/import", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ directory: path, source }),
      }),
      t(directory ? "deviceInspector.deviceDirectoryImported" : "deviceInspector.deviceFileImported"),
    );
  };

  const exportPath = async (entry: DeviceFileEntry) => {
    if (entry.kind === "other") return;
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
      const result = await response.json() as { bytes_written: number; files_written: number };
      if (requestVersion.current === version) {
        void message.success(t("deviceInspector.deviceFileExported", {
          size: formatFileSize(result.bytes_written),
          count: result.files_written,
        }));
      }
    } catch (exportError) {
      if (requestVersion.current === version) {
        void message.error(t("deviceInspector.deviceFileExportFailed", { error: String(exportError) }));
      }
    } finally {
      if (requestVersion.current === version) setBusy(null);
    }
  };

  const createDirectory = () => {
    let name = "";
    Modal.confirm({
      title: t("deviceInspector.createDeviceDirectory"),
      content: <Input autoFocus maxLength={255} placeholder={t("deviceInspector.deviceFileName")} onChange={(event) => { name = event.target.value; }} />,
      okText: t("common.create"),
      cancelText: t("common.cancel"),
      async onOk() {
        if (!name.trim()) throw new Error(t("deviceInspector.deviceFileNameRequired"));
        const succeeded = await mutate(
          "mkdir",
          () => request("/api/device/files/directory", {
            method: "PUT",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ directory: path, name: name.trim() }),
          }),
          t("deviceInspector.deviceDirectoryCreated"),
        );
        if (!succeeded) throw new Error(t("deviceInspector.deviceFileOperationRetry"));
      },
    });
  };

  const rename = (entry: DeviceFileEntry) => {
    let name = entry.name;
    Modal.confirm({
      title: t("deviceInspector.renameDeviceFile"),
      content: <Input autoFocus defaultValue={entry.name} maxLength={255} onChange={(event) => { name = event.target.value; }} />,
      okText: t("common.rename"),
      cancelText: t("common.cancel"),
      async onOk() {
        if (!name.trim()) throw new Error(t("deviceInspector.deviceFileNameRequired"));
        const succeeded = await mutate(
          `rename:${entry.path}`,
          () => request("/api/device/files/rename", {
            method: "PUT",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ path: entry.path, name: name.trim() }),
          }),
          t("deviceInspector.deviceFileRenamed"),
        );
        if (!succeeded) throw new Error(t("deviceInspector.deviceFileOperationRetry"));
      },
    });
  };

  const remove = (entry: DeviceFileEntry) => {
    Modal.confirm({
      title: t("deviceInspector.deleteDeviceFile"),
      content: t(entry.kind === "directory"
        ? "deviceInspector.deleteDeviceDirectoryConfirm"
        : "deviceInspector.deleteDeviceFileConfirm", { name: entry.name }),
      okText: t("common.delete"),
      cancelText: t("common.cancel"),
      okButtonProps: { danger: true },
      async onOk() {
        const query = new URLSearchParams({ path: entry.path });
        const succeeded = await mutate(
          `delete:${entry.path}`,
          () => request(`/api/device/files?${query}`, { method: "DELETE" }),
          t("deviceInspector.deviceFileDeleted"),
        );
        if (!succeeded) throw new Error(t("deviceInspector.deviceFileOperationRetry"));
      },
    });
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
        <Tooltip title={t("deviceInspector.createDeviceDirectory")}>
          <Button icon={<FolderAddOutlined />} aria-label={t("deviceInspector.createDeviceDirectory")} disabled={busy !== null} onClick={createDirectory} />
        </Tooltip>
        <Dropdown
          disabled={busy !== null}
          menu={{
            items: [
              { key: "file", icon: <FileAddOutlined />, label: t("deviceInspector.importDeviceFile") },
              { key: "directory", icon: <FolderAddOutlined />, label: t("deviceInspector.importDeviceDirectory") },
            ],
            onClick: ({ key }) => void importPath(key === "directory"),
          }}
        >
          <Button icon={<UploadOutlined />} aria-label={t("deviceInspector.importDevicePath")} disabled={busy !== null} />
        </Dropdown>
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
                {entry.kind !== "other" && (
                  <Tooltip title={t("deviceInspector.exportDevicePath")}><Button size="small" icon={<DownloadOutlined />} aria-label={t("deviceInspector.exportDevicePath")} loading={busy === `export:${entry.path}`} disabled={busy !== null && busy !== `export:${entry.path}`} onClick={() => void exportPath(entry)} /></Tooltip>
                )}
                <Tooltip title={t("deviceInspector.renameDeviceFile")}><Button size="small" icon={<EditOutlined />} aria-label={t("deviceInspector.renameDeviceFile")} disabled={busy !== null || entry.kind === "other"} onClick={() => rename(entry)} /></Tooltip>
                <Tooltip title={t("deviceInspector.deleteDeviceFile")}><Button size="small" danger icon={<DeleteOutlined />} aria-label={t("deviceInspector.deleteDeviceFile")} disabled={busy !== null || entry.kind === "other"} onClick={() => remove(entry)} /></Tooltip>
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
