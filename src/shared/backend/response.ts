export type BackendErrorDetail = {
  code: string | null;
  message: string;
  retryable: boolean;
  suggestedAction: string | null;
};

export class BackendResponseError extends Error {
  readonly status: number;
  readonly code: string | null;
  readonly retryable: boolean;
  readonly suggestedAction: string | null;

  constructor(status: number, detail: BackendErrorDetail) {
    super(detail.message);
    this.status = status;
    this.code = detail.code;
    this.retryable = detail.retryable;
    this.suggestedAction = detail.suggestedAction;
  }
}

export async function requireBackendSuccess(response: Response) {
  if (response.ok) return response;
  throw new BackendResponseError(response.status, await readErrorDetail(response));
}

export async function readBackendJson<T>(response: Response): Promise<T> {
  await requireBackendSuccess(response);
  try {
    return await response.json() as T;
  } catch {
    throw new BackendResponseError(response.status, {
      code: "invalid_response",
      message: "DeviceHub returned an invalid JSON response",
      retryable: true,
      suggestedAction: "retry",
    });
  }
}

async function readErrorDetail(response: Response): Promise<BackendErrorDetail> {
  const body = (await response.text()).trim();
  if (body) {
    try {
      const value = JSON.parse(body) as unknown;
      if (isApiErrorBody(value)) {
        return {
          code: value.error.code,
          message: value.error.message,
          retryable: value.error.retryable,
          suggestedAction: value.error.suggested_action,
        };
      }
    } catch {
      // Some endpoints still return a bounded plain-text error.
    }
  }
  return {
    code: null,
    message: body || `${response.status} ${response.statusText}`.trim(),
    retryable: response.status >= 500,
    suggestedAction: response.status >= 500 ? "retry" : null,
  };
}

function isApiErrorBody(value: unknown): value is {
  error: { code: string; message: string; retryable: boolean; suggested_action: string | null };
} {
  if (!value || typeof value !== "object" || !("error" in value)) return false;
  const error = (value as { error?: unknown }).error;
  if (!error || typeof error !== "object") return false;
  const detail = error as Record<string, unknown>;
  return typeof detail.code === "string"
    && typeof detail.message === "string"
    && typeof detail.retryable === "boolean"
    && (detail.suggested_action === null || typeof detail.suggested_action === "string");
}
