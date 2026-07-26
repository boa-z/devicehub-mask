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

pub(crate) use console::serve_app_console;
pub use console::{AppConsoleCommand, AppConsoleLine, AppConsolePhase, AppConsoleSnapshot};
pub use icons::AppIconCommand;
pub(crate) use icons::serve_app_icons;
pub use lifecycle::AppLifecycleCommand;
pub(crate) use lifecycle::serve_app_lifecycle;
pub use manager::{APP_CONTROL_REQUEST_TIMEOUT, APP_LIST_REQUEST_TIMEOUT};
pub(crate) use manager::{AppClientSet, AppManagement, AppServiceTransport};
pub use processes::RunningProcessCommand;
pub(crate) use processes::serve_running_processes;
pub use wda_automation::WdaAutomationCommand;
pub(crate) use wda_automation::serve_wda_automation;
pub use wda_runner::WdaRunnerCommand;
pub(crate) use wda_runner::serve_wda_runner;

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
