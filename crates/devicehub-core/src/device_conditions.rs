//! Bounded device-condition catalog and active simulation state.

use std::sync::{Arc, Mutex};

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

/// Shared observation port for the device-condition catalog and active state.
#[derive(Clone, Default)]
pub struct DeviceConditionSlot(Arc<Mutex<DeviceConditionStatus>>);

impl DeviceConditionSlot {
    pub fn set(&self, status: DeviceConditionStatus) {
        *self
            .0
            .lock()
            .expect("device condition status lock poisoned") = status;
    }

    pub fn get(&self) -> DeviceConditionStatus {
        self.0
            .lock()
            .expect("device condition status lock poisoned")
            .clone()
    }

    pub fn reset(&self) {
        self.set(DeviceConditionStatus::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloned_condition_slot_shares_state_and_reset() {
        let slot = DeviceConditionSlot::default();
        let reader = slot.clone();
        slot.set(DeviceConditionStatus {
            available: true,
            ..DeviceConditionStatus::default()
        });
        assert!(reader.get().available);
        slot.reset();
        assert_eq!(reader.get(), DeviceConditionStatus::default());
    }
}
