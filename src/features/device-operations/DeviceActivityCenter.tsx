import ReloadOutlined from "@ant-design/icons/es/icons/ReloadOutlined";
import SyncOutlined from "@ant-design/icons/es/icons/SyncOutlined";
import { Badge, Button, Empty, Popover, Tag, Tooltip, Typography } from "antd";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { AppPage } from "../../components/AppNavigation";
import { ErrorCopyButton } from "../../components/ErrorPresentation";
import type { BackendRequest } from "../../shared/backend/client";
import {
  isActiveOperation,
  operationWorkspace,
  type ManagedOperation,
  type ManagedOperationPhase,
} from "./deviceOperations";
import { useManagedOperations } from "./useManagedOperations";
import "./deviceActivityCenter.css";

type Props = {
  deviceId: string | null;
  deviceName: string | null;
  enabled: boolean;
  request: BackendRequest;
  onNavigate: (page: AppPage) => void;
};

export function DeviceActivityCenter({ deviceId, deviceName, enabled, request, onNavigate }: Props) {
  const { t, i18n } = useTranslation();
  const [open, setOpen] = useState(false);
  const [cancelConfirmationId, setCancelConfirmationId] = useState<number | null>(null);
  const {
    operations,
    error,
    actionError,
    cancelOperation,
    clearActionError,
    refresh,
  } = useManagedOperations(request, deviceId, enabled, open);
  const activeCount = operations.filter(isActiveOperation).length;
  const failedCount = operations.filter((operation) => operation.phase === "failed").length;
  const visibleOperations = useMemo(() => operations.slice(0, 12), [operations]);

  const content = (
    <div className="device-activity-center">
      <header>
        <div>
          <Typography.Text strong>{t("operations.title")}</Typography.Text>
          <Typography.Text type="secondary">{deviceName ?? t("device.select")}</Typography.Text>
        </div>
        <Tooltip title={t("device.refresh")}>
          <Button type="text" size="small" aria-label={t("device.refresh")} icon={<ReloadOutlined />} onClick={refresh} />
        </Tooltip>
      </header>
      <div className="device-activity-summary">
        <span><strong>{activeCount}</strong>{t("operations.active")}</span>
        <span><strong>{failedCount}</strong>{t("operations.failed")}</span>
      </div>
      {actionError && (
        <div className="device-activity-action-error">
          <Typography.Text type="danger" ellipsis title={actionError}>{t("operations.cancelFailed")}</Typography.Text>
          <ErrorCopyButton error={actionError} />
          <Button type="text" size="small" onClick={clearActionError}>{t("operations.dismiss")}</Button>
        </div>
      )}
      {error ? (
        <div className="device-activity-error">
          <Typography.Text type="danger" ellipsis title={error}>{t("operations.loadFailed")}</Typography.Text>
          <ErrorCopyButton error={error} />
        </div>
      ) : visibleOperations.length === 0 ? (
        <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={t("operations.empty")} />
      ) : (
        <div className="device-activity-list">
          {visibleOperations.map((operation) => (
            <OperationRow
              key={operation.id}
              operation={operation}
              locale={i18n.language}
              confirmingCancel={cancelConfirmationId === operation.id}
              onRequestCancel={() => setCancelConfirmationId(operation.id)}
              onDismissCancel={() => setCancelConfirmationId(null)}
              onCancel={() => {
                setCancelConfirmationId(null);
                void cancelOperation(operation.id);
              }}
              onNavigate={() => {
                setOpen(false);
                onNavigate(operationWorkspace(operation.kind));
              }}
            />
          ))}
        </div>
      )}
    </div>
  );

  return (
    <Popover
      content={content}
      trigger="click"
      placement="bottomRight"
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) setCancelConfirmationId(null);
      }}
      overlayClassName="device-activity-popover"
    >
      <Tooltip title={t("operations.title")}>
        <Badge count={activeCount} dot={activeCount === 0 && failedCount > 0} color={failedCount > 0 && activeCount === 0 ? "#d89614" : undefined}>
          <Button aria-label={t("operations.title")} disabled={!deviceId || !enabled} icon={<SyncOutlined spin={activeCount > 0} />} />
        </Badge>
      </Tooltip>
    </Popover>
  );
}

function OperationRow({ operation, locale, confirmingCancel, onRequestCancel, onDismissCancel, onCancel, onNavigate }: {
  operation: ManagedOperation;
  locale: string;
  confirmingCancel: boolean;
  onRequestCancel: () => void;
  onDismissCancel: () => void;
  onCancel: () => void;
  onNavigate: () => void;
}) {
  const { t } = useTranslation();
  const active = isActiveOperation(operation);
  const progress = operation.progress_percent === null ? undefined : Math.round(operation.progress_percent);
  return (
    <div className={`device-activity-row is-${operation.phase}`}>
      <div className="device-activity-row-heading">
        <Typography.Text strong ellipsis title={t(`operations.kinds.${operation.kind}`)}>
          {t(`operations.kinds.${operation.kind}`)}
        </Typography.Text>
        <Tag color={phaseColor(operation.phase)}>{t(`operations.phases.${operation.phase}`)}</Tag>
      </div>
      {(operation.label || operation.stage) && (
        <Typography.Text type="secondary" ellipsis title={operation.label ?? operation.stage ?? undefined}>
          {operation.label ?? t(`operations.stages.${operation.stage}`, { defaultValue: operation.stage })}
        </Typography.Text>
      )}
      {active && (
        <div className={`device-activity-progress${progress === undefined ? " is-indeterminate" : ""}`} role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={progress}>
          <span style={progress === undefined ? undefined : { width: `${progress}%` }} />
          {progress !== undefined && <Typography.Text>{progress}%</Typography.Text>}
        </div>
      )}
      {operation.error && (
        <div className="device-activity-row-error">
          <Typography.Text type="danger" ellipsis title={operation.error.message}>{operation.error.message}</Typography.Text>
          <ErrorCopyButton error={operation.error.message} />
        </div>
      )}
      {confirmingCancel && (
        <div className="device-activity-cancel-confirmation">
          <Typography.Text>{t("operations.cancelConfirmDescription")}</Typography.Text>
          <div>
            <Button type="text" size="small" onClick={onDismissCancel}>{t("operations.keepTask")}</Button>
            <Button danger size="small" onClick={onCancel}>{t("operations.cancel")}</Button>
          </div>
        </div>
      )}
      <div className="device-activity-row-footer">
        <Typography.Text type="secondary">
          {new Intl.DateTimeFormat(locale, { hour: "2-digit", minute: "2-digit", second: "2-digit" }).format(operation.updated_at_ms)}
        </Typography.Text>
        <div className="device-activity-row-actions">
          {operation.phase === "running" && operation.cancellable && !confirmingCancel && (
            <Button danger type="text" size="small" onClick={onRequestCancel}>
              {t("operations.cancel")}
            </Button>
          )}
          <Button type="link" size="small" onClick={onNavigate}>
            {t("operations.openTool")}
          </Button>
        </div>
      </div>
    </div>
  );
}

function phaseColor(phase: ManagedOperationPhase) {
  switch (phase) {
    case "running": return "processing";
    case "cancelling": return "warning";
    case "succeeded": return "success";
    case "failed": return "error";
    default: return "default";
  }
}
