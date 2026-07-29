//! Commands accepted by the device application runtime.

use devicehub_core::DeviceApp;
use futures_util::TryStreamExt;
use idevice::core_device::{AppListEntry, AppServiceClient};
use idevice::{IdeviceError, ReadWrite};
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

async fn collect_app_stream<R: ReadWrite>(
    client: &mut AppServiceClient<R>,
    app_clips: bool,
    removable_apps: bool,
    hidden_apps: bool,
    internal_apps: bool,
    default_apps: bool,
) -> Result<Vec<AppListEntry>, IdeviceError> {
    client
        .stream_apps(
            app_clips,
            removable_apps,
            hidden_apps,
            internal_apps,
            default_apps,
        )
        .try_collect()
        .await
}

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
