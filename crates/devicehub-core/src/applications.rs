//! Host-independent application inventory and operation models.

use serde::Serialize;

mod wda;

pub use wda::{
    WDA_DEFAULT_SOURCE_CHARS, WDA_MAX_ATTRIBUTE_BYTES, WDA_MAX_ATTRIBUTE_CHARACTERS,
    WDA_MAX_BACKGROUND_DURATION_MS, WDA_MAX_ELEMENTS, WDA_MAX_HOLD_DURATION_MS,
    WDA_MAX_SELECTOR_BYTES, WDA_MAX_SOURCE_CHARS, WDA_MAX_TEXT_BYTES, WDA_MAX_TEXT_CHARACTERS,
    WDA_MAX_WAIT_TIMEOUT_MS, WDA_MIN_BACKGROUND_DURATION_MS, WDA_MIN_HOLD_DURATION_MS,
    WdaBoundedText, WdaDeviceState, WdaElement, WdaElementDetails, WdaElementWaitResult,
    WdaElementWaitState, WdaOrientation, WdaRect, WdaRunnerPhase, WdaRunnerStatus, WdaSize,
    WdaStatus, WdaUiTree, WdaUnlockResult, parse_wda_wait_state, validate_wda_background_duration,
    validate_wda_hold_duration, validate_wda_runner_bundle_id, validate_wda_scroll_direction,
    validate_wda_selector, validate_wda_text, validate_wda_wait_timeout,
};

/// Returns true only when the executable is a direct child of the selected
/// application bundle.
pub fn process_executable_belongs_to_app(app_path: &str, executable_path: &str) -> bool {
    let app_path = normalized_app_path(app_path);
    let executable_path = normalized_app_path(executable_path);
    executable_path
        .rsplit_once('/')
        .is_some_and(|(parent, executable)| parent == app_path && !executable.is_empty())
}

fn normalized_app_path(path: &str) -> &str {
    path.strip_prefix("file://localhost")
        .or_else(|| path.strip_prefix("file://"))
        .unwrap_or(path)
        .trim_end_matches('/')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppOperationKind {
    Uninstall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppOperationState {
    Idle,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppOperationView {
    pub id: u64,
    pub kind: Option<AppOperationKind>,
    pub state: AppOperationState,
    pub stage: Option<String>,
    pub progress: Option<u8>,
    pub label: Option<String>,
    pub error: Option<String>,
}

impl Default for AppOperationView {
    fn default() -> Self {
        Self {
            id: 0,
            kind: None,
            state: AppOperationState::Idle,
            stage: None,
            progress: None,
            label: None,
            error: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceApp {
    pub bundle_id: String,
    pub name: String,
    pub version: Option<String>,
    pub bundle_version: Option<String>,
    pub is_removable: bool,
    pub is_first_party: bool,
    pub is_developer_app: bool,
    pub is_app_clip: bool,
    pub signing_kind: AppSigningKind,
    pub minimum_os_version: Option<String>,
    pub debuggable: Option<bool>,
    pub documents_available: bool,
    pub static_disk_usage_bytes: Option<u64>,
    pub dynamic_disk_usage_bytes: Option<u64>,
    pub total_disk_usage_bytes: Option<u64>,
    /// `None` means the process list was unavailable for this request.
    pub is_running: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppLifecycleStatus {
    pub bundle_id: String,
    pub installed: bool,
    pub running: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppLifecycleWaitResult {
    pub condition_met: bool,
    pub expected_running: bool,
    pub elapsed_ms: u64,
    pub app: AppLifecycleStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunningProcess {
    pub pid: u32,
    pub name: String,
    pub app_name: Option<String>,
    pub is_application: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunningProcessList {
    pub processes: Vec<RunningProcess>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunningProcessStatus {
    pub pid: u32,
    pub running: bool,
    pub executable_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunningProcessWaitResult {
    pub condition_met: bool,
    pub expected_running: bool,
    pub elapsed_ms: u64,
    pub process: RunningProcessStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppSigningKind {
    System,
    Development,
    TestFlight,
    AppStore,
    Distribution,
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_only_an_apps_main_executable() {
        assert!(process_executable_belongs_to_app(
            "/private/var/containers/Bundle/Application/A/Game.app",
            "file:///private/var/containers/Bundle/Application/A/Game.app/Game",
        ));
        assert!(!process_executable_belongs_to_app(
            "/private/var/containers/Bundle/Application/A/Game.app",
            "/private/var/containers/Bundle/Application/A/Game.app/Frameworks/Helper",
        ));
    }
}
