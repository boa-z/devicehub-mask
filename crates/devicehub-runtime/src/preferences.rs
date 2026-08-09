use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, RwLock};

/// Session preferences shared atomically between host settings and runtime work.
#[derive(Clone, Debug)]
pub struct RuntimePreferences(Arc<RuntimePreferencesInner>);

#[derive(Debug)]
struct RuntimePreferencesInner {
    audio_enabled: AtomicBool,
    clipboard_sync_enabled: AtomicBool,
    startup_device_priority: RwLock<Vec<String>>,
    developer_image_mount_policy: AtomicU8,
}

impl RuntimePreferences {
    pub fn new(audio_enabled: bool, clipboard_sync_enabled: bool) -> Self {
        Self(Arc::new(RuntimePreferencesInner {
            audio_enabled: AtomicBool::new(audio_enabled),
            clipboard_sync_enabled: AtomicBool::new(clipboard_sync_enabled),
            startup_device_priority: RwLock::new(Vec::new()),
            developer_image_mount_policy: AtomicU8::new(1),
        }))
    }

    pub fn audio_enabled(&self) -> bool {
        self.0.audio_enabled.load(Ordering::Acquire)
    }

    pub fn set_audio_enabled(&self, enabled: bool) {
        self.0.audio_enabled.store(enabled, Ordering::Release);
    }

    pub fn clipboard_sync_enabled(&self) -> bool {
        self.0.clipboard_sync_enabled.load(Ordering::Acquire)
    }

    pub fn set_clipboard_sync_enabled(&self, enabled: bool) {
        self.0
            .clipboard_sync_enabled
            .store(enabled, Ordering::Release);
    }

    pub fn startup_device_priority(&self) -> Vec<String> {
        self.0
            .startup_device_priority
            .read()
            .expect("runtime preference lock poisoned")
            .clone()
    }

    pub fn set_startup_device_priority(&self, priority: Vec<String>) {
        *self
            .0
            .startup_device_priority
            .write()
            .expect("runtime preference lock poisoned") = priority;
    }

    pub fn developer_image_mount_policy(&self) -> devicehub_core::DeveloperImageMountPolicy {
        match self.0.developer_image_mount_policy.load(Ordering::Acquire) {
            0 => devicehub_core::DeveloperImageMountPolicy::Manual,
            2 => devicehub_core::DeveloperImageMountPolicy::Automatic,
            _ => devicehub_core::DeveloperImageMountPolicy::Ask,
        }
    }

    pub fn set_developer_image_mount_policy(
        &self,
        policy: devicehub_core::DeveloperImageMountPolicy,
    ) {
        let value = match policy {
            devicehub_core::DeveloperImageMountPolicy::Manual => 0,
            devicehub_core::DeveloperImageMountPolicy::Ask => 1,
            devicehub_core::DeveloperImageMountPolicy::Automatic => 2,
        };
        self.0
            .developer_image_mount_policy
            .store(value, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_observe_preference_updates() {
        let writer = RuntimePreferences::new(false, false);
        let reader = writer.clone();

        writer.set_audio_enabled(true);
        writer.set_clipboard_sync_enabled(true);
        writer.set_startup_device_priority(vec!["phone".into(), "tablet".into()]);
        writer
            .set_developer_image_mount_policy(devicehub_core::DeveloperImageMountPolicy::Automatic);

        assert!(reader.audio_enabled());
        assert!(reader.clipboard_sync_enabled());
        assert_eq!(reader.startup_device_priority(), ["phone", "tablet"]);
        assert_eq!(
            reader.developer_image_mount_policy(),
            devicehub_core::DeveloperImageMountPolicy::Automatic
        );
    }
}
