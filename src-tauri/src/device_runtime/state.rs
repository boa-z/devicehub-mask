//! Tokio-backed runtime ports that are intentionally excluded from core.

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::UnboundedSender;

use super::commands::InputCmd;
pub(crate) use devicehub_runtime::ClipboardSlot;

pub(crate) use devicehub_core::{
    ActiveSlot, AppOperationSlot, DeviceListSlot, ErrorSlot, LocationStatusSlot, OrientationSlot,
    StatusSlot, VideoCounters,
};

/// The input channel to the current session. The manager swaps the sender on
/// reconnect, and adapters can detect that commands were dropped while idle.
#[derive(Clone, Default)]
pub(crate) struct InputSink(Arc<Mutex<Option<UnboundedSender<InputCmd>>>>);

impl InputSink {
    pub(crate) fn set(&self, tx: Option<UnboundedSender<InputCmd>>) {
        *self.0.lock().unwrap() = tx;
    }

    pub(crate) fn send(&self, cmd: InputCmd) {
        let _ = self.try_send(cmd);
    }

    pub(crate) fn try_send(&self, cmd: InputCmd) -> bool {
        if let Some(tx) = self.0.lock().unwrap().as_ref() {
            tx.send(cmd).is_ok()
        } else {
            false
        }
    }
}
