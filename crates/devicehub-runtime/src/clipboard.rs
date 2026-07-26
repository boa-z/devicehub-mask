use devicehub_core::ClipboardEvent;
use tokio::sync::broadcast;

mod session;

pub use session::{
    ClipboardBridge, ClipboardImage, DeviceClipboardSession, HostClipboard, HostClipboardFactory,
    connect_device_clipboard,
};

/// Bounded clipboard event fan-out. Slow hosts lose stale metadata rather than
/// delaying the device pasteboard session; clipboard payloads are never retained.
#[derive(Clone)]
pub struct ClipboardSlot(broadcast::Sender<ClipboardEvent>);

impl Default for ClipboardSlot {
    fn default() -> Self {
        let (sender, _) = broadcast::channel(8);
        Self(sender)
    }
}

impl ClipboardSlot {
    pub fn set(&self, event: ClipboardEvent) {
        let _ = self.0.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ClipboardEvent> {
        self.0.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devicehub_core::ClipboardContentKind;

    #[test]
    fn activity_is_broadcast_without_retaining_content() {
        let slot = ClipboardSlot::default();
        let mut receiver = slot.subscribe();
        let event = ClipboardEvent {
            from_device: true,
            kind: ClipboardContentKind::Text,
            preview: "copied text".into(),
        };
        slot.set(event.clone());

        assert_eq!(receiver.try_recv().unwrap(), event);
    }
}
