import type { AppPage } from "../../components/AppNavigation";
import type { BackendRequest } from "../../shared/backend/client";
import { readBackendJson, requireBackendSuccess } from "../../shared/backend/response";

export type ManagedOperationKind =
  | "app_uninstall"
  | "app_document_export"
  | "app_document_import"
  | "device_file_export"
  | "device_file_import"
  | "device_backup"
  | "sysdiagnose"
  | "log_archive"
  | "network_capture"
  | "bluetooth_capture"
  | "developer_image_mount"
  | "developer_image_unmount"
  | "wda_runner";

export type ManagedOperationPhase = "running" | "cancelling" | "succeeded" | "cancelled" | "failed";

export type ManagedOperationError = {
  code: string;
  message: string;
  retryable: boolean;
  suggested_action: string | null;
};

export type ManagedOperation = {
  id: number;
  kind: ManagedOperationKind;
  phase: ManagedOperationPhase;
  stage: string | null;
  label: string | null;
  progress_percent: number | null;
  cancellable: boolean;
  started_at_ms: number;
  updated_at_ms: number;
  error: ManagedOperationError | null;
};

const operationKinds = new Set<ManagedOperationKind>([
  "app_uninstall",
  "app_document_export",
  "app_document_import",
  "device_file_export",
  "device_file_import",
  "device_backup",
  "sysdiagnose",
  "log_archive",
  "network_capture",
  "bluetooth_capture",
  "developer_image_mount",
  "developer_image_unmount",
  "wda_runner",
]);

const operationPhases = new Set<ManagedOperationPhase>([
  "running",
  "cancelling",
  "succeeded",
  "cancelled",
  "failed",
]);

export function isActiveOperation(operation: ManagedOperation) {
  return operation.phase === "running" || operation.phase === "cancelling";
}

export function operationWorkspace(kind: ManagedOperationKind): AppPage {
  switch (kind) {
    case "app_document_export":
    case "app_document_import":
    case "device_file_export":
    case "device_file_import":
      return "afc";
    case "log_archive":
      return "logs";
    case "sysdiagnose":
    case "network_capture":
    case "bluetooth_capture":
      return "performance";
    default:
      return "device";
  }
}

export function operationPollDelay(operations: readonly ManagedOperation[], centerOpen: boolean) {
  if (operations.some(isActiveOperation)) return 750;
  return centerOpen ? 2_500 : 5_000;
}

export async function fetchManagedOperations(request: BackendRequest, signal?: AbortSignal) {
  const response = await request("/api/device/operations", { signal });
  return parseManagedOperations(await readBackendJson<unknown>(response));
}

export async function cancelManagedOperation(request: BackendRequest, operationId: number, signal?: AbortSignal) {
  const response = await request(`/api/device/operations/${operationId}`, {
    method: "DELETE",
    signal,
  });
  await requireBackendSuccess(response);
}

export function parseManagedOperations(value: unknown): ManagedOperation[] {
  if (!Array.isArray(value) || !value.every(isManagedOperation)) {
    throw new Error("device operation response is invalid");
  }
  return value;
}

function isManagedOperation(value: unknown): value is ManagedOperation {
  if (!value || typeof value !== "object") return false;
  const operation = value as Partial<ManagedOperation>;
  return isNonNegativeSafeInteger(operation.id) && operation.id > 0
    && operationKinds.has(operation.kind as ManagedOperationKind)
    && operationPhases.has(operation.phase as ManagedOperationPhase)
    && (operation.stage === null || typeof operation.stage === "string")
    && (operation.label === null || typeof operation.label === "string")
    && (operation.progress_percent === null
      || (typeof operation.progress_percent === "number"
        && Number.isFinite(operation.progress_percent)
        && operation.progress_percent >= 0
        && operation.progress_percent <= 100))
    && typeof operation.cancellable === "boolean"
    && isNonNegativeSafeInteger(operation.started_at_ms)
    && isNonNegativeSafeInteger(operation.updated_at_ms)
    && (operation.error === null || isManagedOperationError(operation.error));
}

function isNonNegativeSafeInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

function isManagedOperationError(value: unknown): value is ManagedOperationError {
  if (!value || typeof value !== "object") return false;
  const error = value as Partial<ManagedOperationError>;
  return typeof error.code === "string"
    && typeof error.message === "string"
    && typeof error.retryable === "boolean"
    && (error.suggested_action === null || typeof error.suggested_action === "string");
}
