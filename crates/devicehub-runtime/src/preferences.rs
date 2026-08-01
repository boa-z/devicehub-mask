use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

/// Session preferences shared atomically between host settings and runtime work.
#[derive(Clone, Debug)]
pub struct RuntimePreferences(Arc<RuntimePreferencesInner>);

#[derive(Debug)]
struct RuntimePreferencesInner {
    audio_enabled: AtomicBool,
    clipboard_sync_enabled: AtomicBool,
    startup_device_priority: RwLock<Vec<String>>,
}

impl RuntimePreferences {
    pub fn new(audio_enabled: bool, clipboard_sync_enabled: bool) -> Self {
        Self(Arc::new(RuntimePreferencesInner {
            audio_enabled: AtomicBool::new(audio_enabled),
            clipboard_sync_enabled: AtomicBool::new(clipboard_sync_enabled),
            startup_device_priority: RwLock::new(Vec::new()),
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

        assert!(reader.audio_enabled());
        assert!(reader.clipboard_sync_enabled());
        assert_eq!(reader.startup_device_priority(), ["phone", "tablet"]);
    }
}
