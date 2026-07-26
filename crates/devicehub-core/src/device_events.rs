//! Normalized device metadata events shared by runtime and adapters.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceEventKind {
    AppInstalled,
    AppUninstalled,
    ActivationStateChanged,
    DiskUsageChanged,
    DeviceNameChanged,
    RegionalSettingsChanged,
    DeveloperImageMounted,
    LockStateChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DeviceEvent {
    pub sequence: u64,
    pub kind: DeviceEventKind,
}
