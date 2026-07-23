use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::reload;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry};

const XPC_PARTIAL_FRAME_FILTER: &str = "idevice::xpc::format=error";
const DEFAULT_FILTER: &str =
    "devicehub_mask=info,tower_http=warn,idevice=warn,idevice::xpc::format=error";
const DEBUG_FILTER: &str =
    "devicehub_mask=debug,tower_http=info,idevice=info,idevice::xpc::format=error";

type FilterHandle = reload::Handle<EnvFilter, Registry>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogMode {
    Normal,
    Debug,
    Custom,
}

#[derive(Debug, Serialize)]
pub struct DiagnosticsStatus {
    pub debug_enabled: bool,
    pub custom_filter: bool,
    pub filter: String,
    pub log_directory: String,
    pub file_logging: bool,
    pub run_id: String,
    pub dropped_log_lines: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrontendLogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Deserialize)]
pub struct FrontendLogEvent {
    pub level: FrontendLogLevel,
    pub component: String,
    pub operation: String,
    pub message: String,
}

pub struct Diagnostics {
    filter_handle: FilterHandle,
    filter: Mutex<String>,
    mode: Mutex<LogMode>,
    log_directory: PathBuf,
    file_logging: bool,
    run_id: String,
    dropped_log_lines: Option<tracing_appender::non_blocking::ErrorCounter>,
    _file_guard: Option<WorkerGuard>,
}

impl Diagnostics {
    pub fn init(log_directory: PathBuf) -> Result<Self, String> {
        let configured_filter = std::env::var("DEVICEHUB_LOG")
            .or_else(|_| std::env::var("RUST_LOG"))
            .ok();
        let (filter_text, mode, rejected_filter) = match configured_filter {
            Some(value) => {
                let value = suppress_xpc_partial_frame_noise(&value);
                if parse_filter(&value).is_ok() {
                    (value, LogMode::Custom, false)
                } else {
                    eprintln!("DeviceHub Mask ignored an invalid DEVICEHUB_LOG/RUST_LOG filter");
                    (DEFAULT_FILTER.to_owned(), LogMode::Normal, true)
                }
            }
            None => (DEFAULT_FILTER.to_owned(), LogMode::Normal, false),
        };
        let filter = parse_filter(&filter_text)?;
        let (filter_layer, filter_handle) = reload::Layer::new(filter);

        let (file_layer, file_guard, dropped_log_lines, file_logging) =
            match create_file_writer(&log_directory) {
                Ok((writer, guard)) => {
                    let error_counter = writer.error_counter();
                    (
                        Some(
                            fmt::layer()
                                .json()
                                .with_ansi(false)
                                .with_current_span(false)
                                .with_span_list(false)
                                .with_thread_ids(true)
                                .with_thread_names(true)
                                .with_writer(writer),
                        ),
                        Some(guard),
                        Some(error_counter),
                        true,
                    )
                }
                Err(error) => {
                    eprintln!("DeviceHub Mask could not initialize file logging: {error}");
                    (None, None, None, false)
                }
            };

        tracing_subscriber::registry()
            .with(filter_layer)
            .with(
                fmt::layer()
                    .compact()
                    .with_thread_ids(true)
                    .with_thread_names(true),
            )
            .with(file_layer)
            .try_init()
            .map_err(|error| format!("cannot install tracing subscriber: {error}"))?;

        let run_id = uuid::Uuid::new_v4().simple().to_string();
        let diagnostics = Self {
            filter_handle,
            filter: Mutex::new(filter_text),
            mode: Mutex::new(mode),
            log_directory,
            file_logging,
            run_id,
            dropped_log_lines,
            _file_guard: file_guard,
        };
        diagnostics.install_panic_hook();
        diagnostics.log_startup();
        if rejected_filter {
            tracing::warn!("ignored invalid environment log filter; using defaults");
        }
        Ok(diagnostics)
    }

    pub fn status(&self) -> DiagnosticsStatus {
        let mode = *self.mode.lock().unwrap();
        DiagnosticsStatus {
            debug_enabled: mode == LogMode::Debug,
            custom_filter: mode == LogMode::Custom,
            filter: self.filter.lock().unwrap().clone(),
            log_directory: self.log_directory.to_string_lossy().into_owned(),
            file_logging: self.file_logging,
            run_id: self.run_id.clone(),
            dropped_log_lines: self
                .dropped_log_lines
                .as_ref()
                .map_or(0, |counter| counter.dropped_lines()),
        }
    }

    pub fn set_debug_enabled(&self, enabled: bool) -> Result<DiagnosticsStatus, String> {
        let filter_text = if enabled {
            DEBUG_FILTER
        } else {
            DEFAULT_FILTER
        };
        let filter = EnvFilter::builder()
            .with_default_directive(LevelFilter::WARN.into())
            .parse(filter_text)
            .map_err(|error| format!("invalid built-in log filter: {error}"))?;
        self.filter_handle
            .reload(filter)
            .map_err(|error| format!("cannot reload log filter: {error}"))?;
        *self.filter.lock().unwrap() = filter_text.to_owned();
        *self.mode.lock().unwrap() = if enabled {
            LogMode::Debug
        } else {
            LogMode::Normal
        };
        tracing::info!(
            debug_enabled = enabled,
            filter = filter_text,
            "log filter changed"
        );
        Ok(self.status())
    }

    pub fn open_log_directory(&self) -> Result<(), String> {
        tauri_plugin_opener::open_path(&self.log_directory, None::<&str>)
            .map_err(|error| format!("cannot open log directory: {error}"))
    }

    fn install_panic_hook(&self) {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let location = info.location();
            tracing::error!(
                panic = %info,
                file = location.map(|value| value.file()),
                line = location.map(|value| value.line()),
                column = location.map(|value| value.column()),
                "unhandled panic"
            );
            previous(info);
        }));
    }

    fn log_startup(&self) {
        tracing::info!(
            run_id = %self.run_id,
            version = env!("CARGO_PKG_VERSION"),
            profile = if cfg!(debug_assertions) { "debug" } else { "release" },
            os = std::env::consts::OS,
            arch = std::env::consts::ARCH,
            file_logging = self.file_logging,
            log_directory = %self.log_directory.display(),
            filter = %self.filter.lock().unwrap(),
            "application diagnostics initialized"
        );
    }
}

fn parse_filter(value: &str) -> Result<EnvFilter, String> {
    EnvFilter::builder()
        .with_default_directive(LevelFilter::WARN.into())
        .parse(value)
        .map_err(|error| format!("invalid log filter: {error}"))
}

fn suppress_xpc_partial_frame_noise(value: &str) -> String {
    if value
        .split(',')
        .any(|directive| directive.trim().starts_with("idevice::xpc::format="))
    {
        value.to_owned()
    } else if value.trim().is_empty() {
        XPC_PARTIAL_FRAME_FILTER.to_owned()
    } else {
        format!("{value},{XPC_PARTIAL_FRAME_FILTER}")
    }
}

fn create_file_writer(
    log_directory: &Path,
) -> Result<(tracing_appender::non_blocking::NonBlocking, WorkerGuard), String> {
    std::fs::create_dir_all(log_directory)
        .map_err(|error| format!("cannot create {}: {error}", log_directory.display()))?;
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("devicehub-mask")
        .filename_suffix("jsonl")
        .max_log_files(7)
        .build(log_directory)
        .map_err(|error| format!("cannot create rolling log appender: {error}"))?;
    Ok(
        tracing_appender::non_blocking::NonBlockingBuilder::default()
            .buffered_lines_limit(8_192)
            .lossy(true)
            .thread_name("devicehub-log-writer")
            .finish(appender),
    )
}

pub fn record_frontend_event(event: FrontendLogEvent) -> Result<(), String> {
    let component = bounded_field("component", event.component, 64)?;
    let operation = bounded_field("operation", event.operation, 64)?;
    let message = bounded_field("message", event.message, 2_048)?;
    match event.level {
        FrontendLogLevel::Debug => {
            tracing::debug!(target: "devicehub_mask::frontend", %component, %operation, %message, "frontend event")
        }
        FrontendLogLevel::Info => {
            tracing::info!(target: "devicehub_mask::frontend", %component, %operation, %message, "frontend event")
        }
        FrontendLogLevel::Warn => {
            tracing::warn!(target: "devicehub_mask::frontend", %component, %operation, %message, "frontend event")
        }
        FrontendLogLevel::Error => {
            tracing::error!(target: "devicehub_mask::frontend", %component, %operation, %message, "frontend event")
        }
    }
    Ok(())
}

pub fn device_id_fingerprint(udid: &str) -> String {
    let hash = udid.as_bytes().iter().fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(*byte)).wrapping_mul(0x0100_0193)
    });
    format!("{hash:08x}")
}

fn bounded_field(name: &str, value: String, max_chars: usize) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{name} must not be empty"));
    }
    if trimmed.chars().count() > max_chars {
        return Err(format!("{name} exceeds {max_chars} characters"));
    }
    Ok(trimmed.replace(['\r', '\n'], " "))
}

#[cfg(test)]
mod tests {
    use super::{bounded_field, device_id_fingerprint, suppress_xpc_partial_frame_noise};

    #[test]
    fn suppresses_partial_xpc_frame_noise_without_overriding_explicit_filters() {
        assert_eq!(
            suppress_xpc_partial_frame_noise("warn"),
            "warn,idevice::xpc::format=error"
        );
        assert_eq!(
            suppress_xpc_partial_frame_noise("warn,idevice::xpc::format=trace"),
            "warn,idevice::xpc::format=trace"
        );
    }

    #[test]
    fn frontend_fields_are_single_line() {
        assert_eq!(
            bounded_field("message", " first\nsecond ".into(), 20).unwrap(),
            "first second"
        );
    }

    #[test]
    fn frontend_fields_are_bounded() {
        assert!(bounded_field("component", "".into(), 5).is_err());
        assert!(bounded_field("component", "123456".into(), 5).is_err());
    }

    #[test]
    fn device_fingerprint_is_stable_and_redacted() {
        let udid = "00008030-001905C02106402E";
        let fingerprint = device_id_fingerprint(udid);
        assert_eq!(fingerprint, device_id_fingerprint(udid));
        assert_eq!(fingerprint, "76476ed5");
        assert_eq!(fingerprint.len(), 8);
        assert!(!fingerprint.contains(udid));
    }
}
