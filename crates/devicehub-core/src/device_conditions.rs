//! Bounded device-condition catalog and active simulation state.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceConditionProfile {
    pub identifier: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceConditionGroup {
    pub identifier: String,
    pub profiles: Vec<DeviceConditionProfile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActiveDeviceCondition {
    pub group_identifier: String,
    pub profile_identifier: String,
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DeviceConditionStatus {
    pub available: bool,
    pub groups: Vec<DeviceConditionGroup>,
    pub active: Option<ActiveDeviceCondition>,
    pub cleanup_pending: bool,
    pub error: Option<String>,
}
