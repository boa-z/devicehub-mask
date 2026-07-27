import { invoke } from "@tauri-apps/api/core";
import { browserHostJson, browserHostRequest, runningInDesktopHost } from "./hostApi";

export type FrontendLogLevel = "debug" | "info" | "warn" | "error";

export type DiagnosticsStatus = {
  debug_enabled: boolean;
  custom_filter: boolean;
  filter: string;
  log_directory: string;
  file_logging: boolean;
  run_id: string;
  dropped_log_lines: number;
};

const frontendLogWindowMs = 5_000;
const recentEvents = new Map<string, { sentAt: number; suppressed: number }>();

function describeError(value: unknown) {
  if (value instanceof Error) {
    return value.stack || `${value.name}: ${value.message}`;
  }
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

export function logFrontend(
  level: FrontendLogLevel,
  component: string,
  operation: string,
  value: unknown,
) {
  const key = `${level}:${component}:${operation}`;
  const now = performance.now();
  const recent = recentEvents.get(key);
  if (recent && now - recent.sentAt < frontendLogWindowMs) {
    recent.suppressed += 1;
    return;
  }
  const suffix = recent?.suppressed ? ` (suppressed ${recent.suppressed} repeated events)` : "";
  recentEvents.set(key, { sentAt: now, suppressed: 0 });
  const message = `${describeError(value).replace(/[\r\n]+/g, " ") || "Unknown error"}${suffix}`.slice(0, 2_048);
  const event = { level, component, operation, message };
  const sent = runningInDesktopHost()
    ? invoke("frontend_log", { event })
    : browserHostRequest("/api/host/frontend-log", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(event),
      });
  void sent.catch(() => undefined);
}

export function installGlobalDiagnostics() {
  window.addEventListener("error", (event) => {
    logFrontend("error", "window", "uncaught_error", event.error ?? event.message);
  });
  window.addEventListener("unhandledrejection", (event) => {
    logFrontend("error", "window", "unhandled_rejection", event.reason);
  });
  logFrontend("info", "application", "frontend_started", navigator.userAgent);
}

export function readDiagnosticsStatus() {
  return runningInDesktopHost()
    ? invoke<DiagnosticsStatus>("diagnostics_status")
    : browserHostJson<DiagnosticsStatus>("/api/host/diagnostics");
}

export function setDebugLogging(enabled: boolean) {
  return runningInDesktopHost()
    ? invoke<DiagnosticsStatus>("set_debug_logging", { enabled })
    : browserHostJson<DiagnosticsStatus>("/api/host/diagnostics", {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ enabled }),
      });
}

export function openLogDirectory() {
  return runningInDesktopHost()
    ? invoke<void>("open_log_directory")
    : Promise.reject(new Error("Headless logs are written to the service process output"));
}
