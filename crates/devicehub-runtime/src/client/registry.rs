//! Thread-safe index of device-scoped runtime clients.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use devicehub_core::ActiveSlot;

use super::DeviceSessionClient;

/// Cloneable registry of independently owned device sessions.
///
/// Keys are transport-aware selection IDs (`<udid>::usb` or
/// `<udid>::wifi`). A caller must resolve a session for each operation instead
/// of retaining whichever device happened to be selected when it started.
pub struct DeviceSessionRegistry<HostPath>(
    Arc<RwLock<HashMap<String, DeviceSessionClient<HostPath>>>>,
);

impl<HostPath> Clone for DeviceSessionRegistry<HostPath> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<HostPath> Default for DeviceSessionRegistry<HostPath> {
    fn default() -> Self {
        Self(Arc::new(RwLock::new(HashMap::new())))
    }
}

impl<HostPath> DeviceSessionRegistry<HostPath> {
    /// Resolve a session by its exact transport-aware selection ID.
    pub fn get(&self, selection_id: &str) -> Option<DeviceSessionClient<HostPath>> {
        self.0.read().unwrap().get(selection_id).cloned()
    }

    /// Resolve the session currently selected by the manager-facing UI state.
    pub fn selected(&self, active: &ActiveSlot) -> Option<DeviceSessionClient<HostPath>> {
        active
            .selection_id()
            .and_then(|selection_id| self.get(&selection_id))
    }

    /// Return a stable, sorted view suitable for status and diagnostic output.
    pub fn selection_ids(&self) -> Vec<String> {
        let mut ids = self.0.read().unwrap().keys().cloned().collect::<Vec<_>>();
        ids.sort();
        ids
    }

    pub fn len(&self) -> usize {
        self.0.read().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.read().unwrap().is_empty()
    }

    pub(crate) fn insert(
        &self,
        selection_id: String,
        session: DeviceSessionClient<HostPath>,
    ) -> Option<DeviceSessionClient<HostPath>> {
        self.0.write().unwrap().insert(selection_id, session)
    }

    pub(crate) fn remove(&self, selection_id: &str) -> Option<DeviceSessionClient<HostPath>> {
        self.0.write().unwrap().remove(selection_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::CoreRuntimeState;

    #[test]
    fn sessions_are_isolated_and_resolved_by_exact_selection() {
        let first = CoreRuntimeState::<String>::default();
        let second = CoreRuntimeState::<String>::default();
        let (first_control, _) = tokio::sync::mpsc::unbounded_channel();
        let (second_control, _) = tokio::sync::mpsc::unbounded_channel();
        let first_client = first.client(first_control).device;
        let second_client = second.client(second_control).device;
        first_client
            .status
            .set_phase(devicehub_core::SessionPhase::Connected, "connected:first");
        second_client
            .status
            .set_phase(devicehub_core::SessionPhase::Connected, "connected:second");

        let registry = DeviceSessionRegistry::default();
        registry.insert("phone::usb".into(), first_client);
        registry.insert("tablet::wifi".into(), second_client);

        assert_eq!(
            registry.get("phone::usb").unwrap().status.get(),
            "connected:first"
        );
        assert_eq!(
            registry.get("tablet::wifi").unwrap().status.get(),
            "connected:second"
        );
        assert_eq!(
            registry.selection_ids(),
            vec!["phone::usb".to_string(), "tablet::wifi".to_string()]
        );
    }

    #[test]
    fn selected_session_tracks_manager_selection_without_removing_others() {
        let state = CoreRuntimeState::<String>::default();
        let (control, _) = tokio::sync::mpsc::unbounded_channel();
        let session = state.client(control).device;
        let active = ActiveSlot::default();
        let registry = DeviceSessionRegistry::default();
        registry.insert("phone::usb".into(), session);

        assert!(registry.selected(&active).is_none());
        active.set_selected("phone".into(), "phone::usb".into());
        assert!(registry.selected(&active).is_some());
        assert_eq!(registry.len(), 1);
    }
}
