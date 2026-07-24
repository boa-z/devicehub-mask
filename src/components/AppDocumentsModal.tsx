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
  StopOutlined,
  UploadOutlined,
} from "@ant-design/icons";
import { open, save } from "@tauri-apps/plugin-dialog";
import { Alert, Breadcrumb, Button, Dropdown, Empty, Input, Modal, Progress, Segmented, Spin, Tooltip, Typography, message } from "antd";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { formatFileSize } from "../deviceInspector";
import type { AppDocumentActivity, AppDocumentEntry, AppDocumentList, DeviceApp } from "../types";

type Request = (path: string, init?: RequestInit) => Promise<Response>;
export type AppStorageScope = "documents" | "container";

type Props = {
  app: DeviceApp | null;
  request: Request;
  onClose?: () => void;
  active?: boolean;
  embedded?: boolean;
  fixedScope?: AppStorageScope;
  onTransferStateChange?: (active: boolean) => void;
};

async function readJson<T>(response: Response): Promise<T> {
  if (!response.ok) throw new Error((await response.text()) || `${response.status} ${response.statusText}`);
  return response.json() as Promise<T>;
}

function endpoint(bundleId: string, suffix = "") {
  return `/api/device/apps/${encodeURIComponent(bundleId)}/storage${suffix}`;
}

function parentPath(path: string) {
  const parts = path.split("/").filter(Boolean);
  parts.pop();
  return parts.length ? `/${parts.join("/")}` : "/";
}

export function AppDocumentsModal({ app, request, onClose = () => undefined, active = true, embedded = false, fixedScope, onTransferStateChange }: Props) {
  const { t, i18n } = useTranslation();
  const [path, setPath] = useState("/");
  const [scope, setScope] = useState<AppStorageScope>(fixedScope ?? "documents");
  const [listing, setListing] = useState<AppDocumentList | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [activity, setActivity] = useState<AppDocumentActivity | null>(null);
  const [cancelPending, setCancelPending] = useState(false);
  const [cancelRequested, setCancelRequested] = useState(false);
  const cancelRequestedRef = useRef(false);
  const activeBundleIdRef = useRef(app?.bundle_id);

  const load = useCallback(async () => {
    if (!active || !app
      || (!app.documents_available && scope === "documents")
      || (!app.is_developer_app && scope === "container")) return;
    setBusy("list");
    setError(null);
    try {
      const query = new URLSearchParams({ path, scope });
      setListing(await readJson<AppDocumentList>(await request(`${endpoint(app.bundle_id)}?${query}`)));
    } catch (loadError) {
      setListing(null);
      setError(String(loadError));
    } finally {
      setBusy(null);
    }
  }, [active, app, path, request, scope]);

  useEffect(() => {
    activeBundleIdRef.current = app?.bundle_id;
    setPath("/");
    setScope(fixedScope ?? (app?.documents_available === false ? "container" : "documents"));
    setListing(null);
    setError(null);
    setCancelPending(false);
    setCancelRequested(false);
    cancelRequestedRef.current = false;
  }, [app?.bundle_id, app?.documents_available, fixedScope]);

  const transferBusy = busy === "upload" || busy?.startsWith("export:") === true;
  useEffect(() => {
    onTransferStateChange?.(transferBusy);
    return () => onTransferStateChange?.(false);
  }, [onTransferStateChange, transferBusy]);

  useEffect(() => {
    const bundleId = app?.bundle_id;
    if (!active || !bundleId || !transferBusy) {
      setActivity(null);
      return;
    }
    let cancelled = false;
    const poll = async () => {
      try {
        const next = await readJson<AppDocumentActivity>(
          await request(endpoint(bundleId, "/activity")),
        );
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
  }, [active, app?.bundle_id, request, transferBusy]);

  const changeScope = (nextScope: AppStorageScope) => {
    if (busy !== null || nextScope === scope) return;
    setScope(nextScope);
    setPath("/");
    setListing(null);
    setError(null);
  };

  useEffect(() => {
    void load();
  }, [load]);

  const breadcrumbs = useMemo(() => {
    const parts = path.split("/").filter(Boolean);
    return [
      {
        title: <Button type="text" size="small" icon={<HomeOutlined />} aria-label={t("deviceInspector.documentsRoot")} onClick={() => setPath("/")} />,
      },
      ...parts.map((part, index) => ({
        title: <Button type="text" size="small" onClick={() => setPath(`/${parts.slice(0, index + 1).join("/")}`)}>{part}</Button>,
      })),
    ];
  }, [path, t]);

  const mutate = async (operation: string, call: () => Promise<Response>, success: string, refresh = true) => {
    setBusy(operation);
    try {
      const response = await call();
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      void message.success(success);
      if (refresh) await load();
      return true;
    } catch (mutationError) {
      if (operation === "upload" && (cancelRequestedRef.current || String(mutationError).includes("application storage transfer cancelled"))) {
        void message.info(t("deviceInspector.documentTransferCancelled"));
      } else {
        void message.error(t("deviceInspector.documentOperationFailed", { error: String(mutationError) }));
      }
      return false;
    } finally {
      setBusy(null);
      if (operation === "upload") {
        setCancelPending(false);
        setCancelRequested(false);
        cancelRequestedRef.current = false;
      }
    }
  };

  const upload = async (directory: boolean) => {
    if (!app) return;
    const source = await open({ multiple: false, directory });
    if (!source || Array.isArray(source)) return;
    setCancelPending(false);
    setCancelRequested(false);
    cancelRequestedRef.current = false;
    await mutate(
      "upload",
      () => request(endpoint(app.bundle_id, "/import"), {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ directory: path, source, scope }),
      }),
      t(directory ? "deviceInspector.documentDirectoryUploaded" : "deviceInspector.documentUploaded"),
    );
  };

  const download = async (entry: AppDocumentEntry) => {
    if (!app || entry.kind === "other") return;
    const destination = await save({ defaultPath: entry.name });
    if (!destination) return;
    setCancelPending(false);
    setCancelRequested(false);
    cancelRequestedRef.current = false;
    setBusy(`export:${entry.path}`);
    try {
      const response = await request(endpoint(app.bundle_id, "/export"), {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ path: entry.path, destination, scope }),
      });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      const result = await response.json() as { bytes_written: number; files_written: number };
      void message.success(t("deviceInspector.documentExported", {
        size: formatFileSize(result.bytes_written),
        count: result.files_written,
      }));
    } catch (exportError) {
      if (cancelRequestedRef.current || String(exportError).includes("application storage transfer cancelled")) {
        void message.info(t("deviceInspector.documentTransferCancelled"));
      } else {
        void message.error(t("deviceInspector.documentOperationFailed", { error: String(exportError) }));
      }
    } finally {
      setBusy(null);
      setCancelPending(false);
      setCancelRequested(false);
      cancelRequestedRef.current = false;
    }
  };

  const cancelTransfer = async () => {
    const bundleId = app?.bundle_id;
    if (!bundleId || !transferBusy || cancelPending || cancelRequested) return;
    setCancelPending(true);
    try {
      const response = await request(endpoint(bundleId, "/activity"), { method: "DELETE" });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);
      if (activeBundleIdRef.current !== bundleId) return;
      cancelRequestedRef.current = true;
      setCancelRequested(true);
    } catch (cancelError) {
      if (activeBundleIdRef.current === bundleId) {
        void message.error(t("deviceInspector.documentCancelFailed", { error: String(cancelError) }));
      }
    } finally {
      if (activeBundleIdRef.current === bundleId) setCancelPending(false);
    }
  };

  const createDirectory = () => {
    if (!app) return;
    let name = "";
    Modal.confirm({
      title: t("deviceInspector.createDocumentDirectory"),
      content: <Input autoFocus maxLength={255} placeholder={t("deviceInspector.documentName")} onChange={(event) => { name = event.target.value; }} />,
      okText: t("common.create"),
      cancelText: t("common.cancel"),
      async onOk() {
        if (!name.trim()) throw new Error(t("deviceInspector.documentNameRequired"));
        const succeeded = await mutate(
          "mkdir",
          () => request(endpoint(app.bundle_id, "/directory"), {
            method: "PUT",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ directory: path, name: name.trim(), scope }),
          }),
          t("deviceInspector.documentDirectoryCreated"),
        );
        if (!succeeded) throw new Error(t("deviceInspector.documentOperationRetry"));
      },
    });
  };

  const rename = (entry: AppDocumentEntry) => {
    if (!app) return;
    let name = entry.name;
    Modal.confirm({
      title: t("deviceInspector.renameDocument"),
      content: <Input autoFocus defaultValue={entry.name} maxLength={255} onChange={(event) => { name = event.target.value; }} />,
      okText: t("common.rename"),
      cancelText: t("common.cancel"),
      async onOk() {
        if (!name.trim()) throw new Error(t("deviceInspector.documentNameRequired"));
        const succeeded = await mutate(
          `rename:${entry.path}`,
          () => request(endpoint(app.bundle_id, "/rename"), {
            method: "PUT",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ path: entry.path, name: name.trim(), scope }),
          }),
          t("deviceInspector.documentRenamed"),
        );
        if (!succeeded) throw new Error(t("deviceInspector.documentOperationRetry"));
      },
    });
  };

  const remove = (entry: AppDocumentEntry) => {
    if (!app) return;
    Modal.confirm({
      title: t("deviceInspector.deleteDocument"),
      content: t(entry.kind === "directory"
        ? "deviceInspector.deleteDocumentDirectoryConfirm"
        : "deviceInspector.deleteDocumentConfirm", { name: entry.name }),
      okText: t("common.delete"),
      cancelText: t("common.cancel"),
      okButtonProps: { danger: true },
      async onOk() {
        const query = new URLSearchParams({
          path: entry.path,
          scope,
          recursive: String(entry.kind === "directory"),
        });
        const succeeded = await mutate(
          `delete:${entry.path}`,
          () => request(`${endpoint(app.bundle_id)}?${query}`, { method: "DELETE" }),
          t("deviceInspector.documentDeleted"),
        );
        if (!succeeded) throw new Error(t("deviceInspector.documentOperationRetry"));
      },
    });
  };

  const content = (
    <>
      {!fixedScope && <Segmented
        block
        className="app-storage-scope"
        value={scope}
        disabled={busy !== null}
        options={[
          { value: "documents", label: t("deviceInspector.appStorageDocuments"), disabled: app?.documents_available === false },
          { value: "container", label: t("deviceInspector.appStorageContainer"), disabled: app?.is_developer_app === false },
        ]}
        onChange={(value) => changeScope(value as AppStorageScope)}
      />}
      {scope === "container" && (
        <Alert type="warning" showIcon message={t("deviceInspector.appContainerWarning")} />
      )}
      <div className="app-documents-toolbar">
        <Tooltip title={t("common.back")}>
          <Button icon={<ArrowLeftOutlined />} disabled={path === "/" || busy !== null} onClick={() => setPath(parentPath(path))} />
        </Tooltip>
        <Breadcrumb items={breadcrumbs} />
        <Tooltip title={t("deviceInspector.createDocumentDirectory")}>
          <Button icon={<FolderAddOutlined />} disabled={busy !== null} onClick={createDirectory} />
        </Tooltip>
        <Dropdown
          disabled={busy !== null}
          menu={{
            items: [
              { key: "file", icon: <FileAddOutlined />, label: t("deviceInspector.uploadDocument") },
              { key: "directory", icon: <FolderAddOutlined />, label: t("deviceInspector.uploadDocumentDirectory") },
            ],
            onClick: ({ key }) => void upload(key === "directory"),
          }}
        >
          <Button icon={<UploadOutlined />} aria-label={t("deviceInspector.uploadDocumentPath")} disabled={busy !== null} />
        </Dropdown>
        <Tooltip title={t("deviceInspector.refreshDocuments")}>
          <Button icon={<ReloadOutlined />} disabled={busy !== null} onClick={() => void load()} />
        </Tooltip>
      </div>
      {transferBusy && (
        <div className="app-document-transfer">
          <div className="app-document-transfer-heading">
            <Typography.Text>
              {t(cancelRequested
                ? "deviceInspector.documentTransferCancelling"
                : busy === "upload"
                  ? "deviceInspector.documentTransferUploading"
                  : "deviceInspector.documentTransferDownloading")}
            </Typography.Text>
            <div className="app-document-transfer-actions">
              {activity?.state === "running" || activity?.state === "cancelled" ? (
                <Typography.Text type="secondary">
                  {t("deviceInspector.documentTransferProgress", {
                    size: formatFileSize(activity.bytes_transferred),
                    files: activity.files_transferred,
                    directories: activity.directories_transferred,
                  })}
                </Typography.Text>
              ) : <Spin size="small" />}
              <Tooltip title={t(cancelRequested
                ? "deviceInspector.documentTransferCancelling"
                : "deviceInspector.cancelDocumentTransfer")}>
                <Button
                  size="small"
                  danger
                  icon={<StopOutlined />}
                  aria-label={t("deviceInspector.cancelDocumentTransfer")}
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
        <Alert type="error" showIcon message={t("deviceInspector.documentsUnavailable")} description={error} />
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
                {new Date(entry.modified).toLocaleString(i18n.language)}
              </Typography.Text>
              <div className="app-document-actions">
                {entry.kind === "directory" && (
                  <Tooltip title={t("deviceInspector.openDocumentDirectory")}><Button size="small" icon={<FolderOpenOutlined />} disabled={busy !== null} onClick={() => setPath(entry.path)} /></Tooltip>
                )}
                {entry.kind !== "other" && (
                  <Tooltip title={t("deviceInspector.exportDocument")}><Button size="small" icon={<DownloadOutlined />} loading={busy === `export:${entry.path}`} disabled={busy !== null && busy !== `export:${entry.path}`} onClick={() => void download(entry)} /></Tooltip>
                )}
                <Tooltip title={t("deviceInspector.renameDocument")}><Button size="small" icon={<EditOutlined />} disabled={busy !== null || entry.kind === "other"} onClick={() => rename(entry)} /></Tooltip>
                <Tooltip title={t("deviceInspector.deleteDocument")}><Button size="small" danger icon={<DeleteOutlined />} disabled={busy !== null || entry.kind === "other"} onClick={() => remove(entry)} /></Tooltip>
              </div>
            </div>
          ))}
        </div>
      ) : (
        <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={t("deviceInspector.noDocuments")} />
      )}
      {listing?.truncated && <Alert type="warning" showIcon message={t("deviceInspector.documentsTruncated")} />}
    </>
  );

  if (embedded) {
    return <div className="app-storage-browser">{content}</div>;
  }

  return (
    <Modal
      className="app-documents-modal"
      open={app !== null}
      width={760}
      title={app ? t("deviceInspector.appStorageTitle", { name: app.name }) : ""}
      footer={null}
      destroyOnHidden
      closable={busy === null}
      keyboard={busy === null}
      maskClosable={busy === null}
      onCancel={() => { if (busy === null) onClose(); }}
    >
      {content}
    </Modal>
  );
}
