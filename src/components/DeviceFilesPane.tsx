import {
  ArrowLeftOutlined,
  CheckOutlined,
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
  SortAscendingOutlined,
  SortDescendingOutlined,
  StopOutlined,
  UploadOutlined,
} from "@ant-design/icons";
import { open as openDialog, save } from "@tauri-apps/plugin-dialog";
import { Alert, Breadcrumb, Button, Dropdown, Empty, Input, Modal, Progress, Spin, Tooltip, Typography, message } from "antd";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { normalizeAfcPath, sortAfcEntries } from "../afcBrowser";
import type { AfcSortDirection, AfcSortField } from "../afcBrowser";
import { formatFileSize } from "../deviceInspector";
import type { DeviceFileActivity, DeviceFileEntry, DeviceFileList } from "../types";

type Request = (path: string, init?: RequestInit) => Promise<Response>;

type Props = {
  active: boolean;
  deviceId: string | null;
  refreshToken: number;
  request: Request;
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

export function DeviceFilesPane({ active, deviceId, refreshToken, request }: Props) {
  const { t, i18n } = useTranslation();
  const [path, setPath] = useState("/");
  const [listing, setListing] = useState<DeviceFileList | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [editingPath, setEditingPath] = useState(false);
  const [pathDraft, setPathDraft] = useState("/");
  const [sortField, setSortField] = useState<AfcSortField>("name");
  const [sortDirection, setSortDirection] = useState<AfcSortDirection>("ascending");
  const [activity, setActivity] = useState<DeviceFileActivity | null>(null);
  const [cancelPending, setCancelPending] = useState(false);
  const [cancelRequested, setCancelRequested] = useState(false);
  const cancelRequestedRef = useRef(false);
  const requestVersion = useRef(0);

  const load = useCallback(async () => {
    if (!active || !deviceId) return;
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
  }, [active, deviceId, path, request]);

  useEffect(() => {
    requestVersion.current += 1;
    setPath("/");
    setPathDraft("/");
    setEditingPath(false);
    setListing(null);
    setError(null);
    setBusy(null);
    setActivity(null);
    setCancelPending(false);
    setCancelRequested(false);
    cancelRequestedRef.current = false;
  }, [deviceId]);

  const transferBusy = busy === "import" || busy?.startsWith("export:") === true;
  useEffect(() => {
    if (!deviceId || !transferBusy) {
      setActivity(null);
      return;
    }
    let cancelled = false;
    const poll = async () => {
      try {
        const next = await readJson<DeviceFileActivity>(await request("/api/device/files/activity"));
        if (!cancelled) setActivity(next);
      } catch {
        // The transfer request reports the authoritative error.
      }
    };
    void poll();
    const interval = window.setInterval(() => void poll(), 250);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, [deviceId, request, transferBusy]);

  useEffect(() => {
    void load();
  }, [load, refreshToken]);

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

  const visibleEntries = useMemo(
    () => sortAfcEntries(
      listing?.entries ?? [],
      sortField,
      sortDirection,
      i18n.resolvedLanguage ?? i18n.language,
    ),
    [i18n.language, i18n.resolvedLanguage, listing?.entries, sortDirection, sortField],
  );

  const startEditingPath = () => {
    setPathDraft(path);
    setEditingPath(true);
  };

  const cancelEditingPath = () => {
    setPathDraft(path);
    setEditingPath(false);
  };

  const commitPath = () => {
    const normalized = normalizeAfcPath(pathDraft);
    if (!normalized) {
      void message.error(t("deviceInspector.deviceFilePathInvalid"));
      return;
    }
    setPath(normalized);
    setPathDraft(normalized);
    setEditingPath(false);
  };

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
        if (operation === "import" && (cancelRequestedRef.current || String(mutationError).includes("device file transfer cancelled"))) {
          void message.info(t("deviceInspector.deviceFileTransferCancelled"));
        } else {
          void message.error(t("deviceInspector.deviceFileOperationFailed", { error: String(mutationError) }));
        }
      }
      return false;
    } finally {
      if (requestVersion.current === version) {
        setBusy(null);
        if (operation === "import") {
          setCancelPending(false);
          setCancelRequested(false);
          cancelRequestedRef.current = false;
        }
      }
    }
  };

  const importPath = async (directory: boolean) => {
    const source = await openDialog({ multiple: false, directory });
    if (!source || Array.isArray(source)) return;
    setCancelPending(false);
    setCancelRequested(false);
    cancelRequestedRef.current = false;
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
    setCancelPending(false);
    setCancelRequested(false);
    cancelRequestedRef.current = false;
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
        if (cancelRequestedRef.current || String(exportError).includes("device file transfer cancelled")) {
          void message.info(t("deviceInspector.deviceFileTransferCancelled"));
        } else {
          void message.error(t("deviceInspector.deviceFileExportFailed", { error: String(exportError) }));
        }
      }
    } finally {
      if (requestVersion.current === version) {
        setBusy(null);
        setCancelPending(false);
        setCancelRequested(false);
        cancelRequestedRef.current = false;
      }
    }
  };

  const cancelTransfer = async () => {
    if (!transferBusy || cancelPending || cancelRequested) return;
    const version = requestVersion.current;
    setCancelPending(true);
    try {
      const response = await request("/api/device/files/activity", { method: "DELETE" });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      if (requestVersion.current === version) {
        cancelRequestedRef.current = true;
        setCancelRequested(true);
      }
    } catch (cancelError) {
      if (requestVersion.current === version) {
        void message.error(t("deviceInspector.deviceFileCancelFailed", { error: String(cancelError) }));
      }
    } finally {
      if (requestVersion.current === version) setCancelPending(false);
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
    <div className="device-files-pane" hidden={!active}>
      <div className="device-files-pane-heading">
        <Typography.Text strong>{t("deviceInspector.deviceFilesTitle")}</Typography.Text>
        <Typography.Text type="secondary">{t("deviceInspector.deviceFilesHint")}</Typography.Text>
      </div>
      <div className="app-documents-toolbar device-files-toolbar">
        <Tooltip title={t("common.back")}>
          <Button icon={<ArrowLeftOutlined />} aria-label={t("common.back")} disabled={path === "/" || busy !== null} onClick={() => setPath(parentPath(path))} />
        </Tooltip>
        {editingPath ? (
          <Input
            autoFocus
            className="device-files-path-input"
            value={pathDraft}
            aria-label={t("deviceInspector.deviceFilePath")}
            disabled={busy !== null}
            onChange={(event) => setPathDraft(event.target.value)}
            onPressEnter={commitPath}
            onKeyDown={(event) => {
              if (event.key === "Escape") cancelEditingPath();
            }}
          />
        ) : <Breadcrumb items={breadcrumbs} />}
        <Tooltip title={t(editingPath ? "deviceInspector.openDeviceFilePath" : "deviceInspector.editDeviceFilePath")}>
          <Button
            icon={editingPath ? <CheckOutlined /> : <EditOutlined />}
            aria-label={t(editingPath ? "deviceInspector.openDeviceFilePath" : "deviceInspector.editDeviceFilePath")}
            disabled={busy !== null}
            onClick={editingPath ? commitPath : startEditingPath}
          />
        </Tooltip>
        <Dropdown
          disabled={busy !== null || visibleEntries.length === 0}
          menu={{
            items: [
              { key: "field:name", icon: sortField === "name" ? <CheckOutlined /> : null, label: t("deviceInspector.sortByName") },
              { key: "field:size", icon: sortField === "size" ? <CheckOutlined /> : null, label: t("deviceInspector.sortBySize") },
              { key: "field:modified", icon: sortField === "modified" ? <CheckOutlined /> : null, label: t("deviceInspector.sortByModified") },
              { type: "divider" },
              { key: "direction:ascending", icon: sortDirection === "ascending" ? <CheckOutlined /> : null, label: t("deviceInspector.sortAscending") },
              { key: "direction:descending", icon: sortDirection === "descending" ? <CheckOutlined /> : null, label: t("deviceInspector.sortDescending") },
            ],
            onClick: ({ key }) => {
              if (key.startsWith("field:")) setSortField(key.slice(6) as AfcSortField);
              else setSortDirection(key.slice(10) as AfcSortDirection);
            },
          }}
        >
          <Button
            icon={sortDirection === "ascending" ? <SortAscendingOutlined /> : <SortDescendingOutlined />}
            aria-label={t("deviceInspector.sortDeviceFiles")}
          />
        </Dropdown>
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
      {transferBusy && (
        <div className="app-document-transfer">
          <div className="app-document-transfer-heading">
            <Typography.Text>
              {t(cancelRequested
                ? "deviceInspector.deviceFileTransferCancelling"
                : busy === "import"
                  ? "deviceInspector.deviceFileTransferImporting"
                  : "deviceInspector.deviceFileTransferExporting")}
            </Typography.Text>
            <div className="app-document-transfer-actions">
              {activity?.state === "running" || activity?.state === "cancelled" ? (
                <Typography.Text type="secondary">
                  {t("deviceInspector.deviceFileTransferProgress", {
                    size: formatFileSize(activity.bytes_transferred),
                    files: activity.files_transferred,
                    directories: activity.directories_transferred,
                  })}
                </Typography.Text>
              ) : <Spin size="small" />}
              <Tooltip title={t(cancelRequested
                ? "deviceInspector.deviceFileTransferCancelling"
                : "deviceInspector.cancelDeviceFileTransfer")}>
                <Button
                  size="small"
                  danger
                  icon={<StopOutlined />}
                  aria-label={t("deviceInspector.cancelDeviceFileTransfer")}
                  loading={cancelPending}
                  disabled={cancelRequested}
                  onClick={() => void cancelTransfer()}
                />
              </Tooltip>
            </div>
          </div>
          {activity?.state === "running" && activity.bytes_total !== null && (
            <Progress
              size="small"
              status="active"
              percent={activity.bytes_total === 0
                ? 100
                : Math.min(100, Math.floor(activity.bytes_transferred * 100 / activity.bytes_total))}
            />
          )}
        </div>
      )}
      {error ? (
        <Alert type="error" showIcon message={t("deviceInspector.deviceFilesUnavailable")} description={error} />
      ) : busy === "list" && !listing ? (
        <div className="app-documents-loading"><Spin /></div>
      ) : visibleEntries.length ? (
        <div className="app-document-list" aria-busy={busy !== null}>
          {visibleEntries.map((entry) => (
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
    </div>
  );
}
