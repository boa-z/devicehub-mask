//! Commands accepted by the device application runtime.

use devicehub_core::DeviceApp;
use tokio::sync::oneshot;

mod console;
mod icons;
mod lifecycle;
mod manager;
mod processes;
mod wda_automation;
mod wda_runner;

pub use console::{
    AppConsoleCommand, AppConsoleLine, AppConsolePhase, AppConsoleSnapshot, serve_app_console,
};
pub use icons::{AppIconCommand, serve_app_icons};
pub use lifecycle::{AppLifecycleCommand, serve_app_lifecycle};
pub use manager::{
    APP_CONTROL_REQUEST_TIMEOUT, APP_LIST_REQUEST_TIMEOUT, AppClientSet, AppManagement,
    AppServiceTransport,
};
pub use processes::{RunningProcessCommand, serve_running_processes};
pub use wda_automation::{
    DEFAULT_SOURCE_CHARS, MAX_ATTRIBUTE_BYTES, MAX_ATTRIBUTE_CHARACTERS,
    MAX_BACKGROUND_DURATION_MS, MAX_ELEMENTS, MAX_HOLD_DURATION_MS, MAX_SELECTOR_BYTES,
    MAX_SOURCE_CHARS, MAX_TEXT_BYTES, MAX_TEXT_CHARACTERS, MAX_WAIT_TIMEOUT_MS,
    MIN_BACKGROUND_DURATION_MS, MIN_HOLD_DURATION_MS, WdaAutomationCommand, WdaBoundedText,
    WdaDeviceState, WdaElement, WdaElementDetails, WdaElementWaitResult, WdaElementWaitState,
    WdaOrientation, WdaRect, WdaSize, WdaStatus, WdaUiTree, WdaUnlockResult, parse_wait_state,
    serve_wda_automation, validate_background_duration, validate_hold_duration,
    validate_scroll_direction, validate_selector, validate_text, validate_wait_timeout,
};
pub use wda_runner::{
    WdaRunnerCommand, WdaRunnerPhase, WdaRunnerStatus, serve_wda_runner, validate_runner_bundle_id,
};

#[derive(Debug)]
pub enum AppCommand {
    List {
        include_system: bool,
        include_app_clips: bool,
        reply: oneshot::Sender<Result<Vec<DeviceApp>, String>>,
    },
    Launch {
        bundle_id: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Stop {
        bundle_id: String,
        reply: oneshot::Sender<Result<bool, String>>,
    },
    Uninstall {
        bundle_id: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
}
