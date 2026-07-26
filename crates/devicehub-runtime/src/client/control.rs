//! Connected-device commands that are shared across host adapters.

use std::fmt;
use std::time::Duration;

use crate::{BrowserVideoSlot, DeviceSessionCommand, SessionCommandSlot};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceControlError {
    Unavailable,
    SessionEnded,
    Timeout(&'static str),
    Operation(String),
}

impl fmt::Display for DeviceControlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => formatter.write_str("no active device session"),
            Self::SessionEnded => formatter.write_str("device session ended"),
            Self::Timeout(operation) => write!(formatter, "{operation} timed out"),
            Self::Operation(error) => formatter.write_str(error),
        }
    }
}

impl std::error::Error for DeviceControlError {}

/// Commands and compressed-frame observations for one runtime state graph.
pub struct DeviceControlService<HostPath> {
    browser_frames: BrowserVideoSlot,
    input: SessionCommandSlot<HostPath>,
}

impl<HostPath> DeviceControlService<HostPath> {
    pub(crate) fn new(
        browser_frames: BrowserVideoSlot,
        input: SessionCommandSlot<HostPath>,
    ) -> Self {
        Self {
            browser_frames,
            input,
        }
    }

    pub fn send(&self, command: DeviceSessionCommand<HostPath>) -> Result<(), DeviceControlError> {
        self.input
            .try_send(command)
            .then_some(())
            .ok_or(DeviceControlError::Unavailable)
    }

    pub fn frame_version(&self) -> u64 {
        self.browser_frames.version()
    }

    pub fn browser_dimensions(&self) -> Option<(u32, u32)> {
        self.browser_frames.dimensions()
    }

    pub async fn capture_screenshot(
        &self,
        timeout: Duration,
    ) -> Result<Vec<u8>, DeviceControlError> {
        let (reply, response) = tokio::sync::oneshot::channel();
        self.send(DeviceSessionCommand::TakeScreenshot(reply))?;
        tokio::time::timeout(timeout, response)
            .await
            .map_err(|_| DeviceControlError::Timeout("device screenshot request"))?
            .map_err(|_| DeviceControlError::SessionEnded)?
            .map_err(DeviceControlError::Operation)
    }

    pub async fn wait_for_frame(&self, after: u64, timeout: Duration) -> bool {
        if self.frame_version() > after {
            return true;
        }
        let mut browser = self.browser_frames.subscribe();
        // Close the publication race between the initial version check and
        // installing the compressed-frame subscription.
        if self.frame_version() > after {
            return true;
        }
        tokio::time::timeout(timeout, async {
            loop {
                let changed = browser.recv().await;
                if matches!(
                    changed,
                    Err(tokio::sync::broadcast::error::RecvError::Closed)
                ) {
                    return false;
                }
                if self.frame_version() > after {
                    return true;
                }
            }
        })
        .await
        .unwrap_or(false)
    }
}

impl<HostPath> Clone for DeviceControlService<HostPath> {
    fn clone(&self) -> Self {
        Self {
            browser_frames: self.browser_frames.clone(),
            input: self.input.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> (
        DeviceControlService<String>,
        tokio::sync::mpsc::UnboundedReceiver<DeviceSessionCommand<String>>,
    ) {
        let input = SessionCommandSlot::default();
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        input.set(Some(sender));
        (
            DeviceControlService::new(BrowserVideoSlot::default(), input),
            receiver,
        )
    }

    #[tokio::test]
    async fn screenshot_dispatches_through_the_active_session() {
        let (service, mut commands) = service();
        let request = tokio::spawn({
            let service = service.clone();
            async move { service.capture_screenshot(Duration::from_secs(1)).await }
        });
        let DeviceSessionCommand::TakeScreenshot(reply) = commands.recv().await.unwrap() else {
            panic!("expected screenshot command");
        };
        reply.send(Ok(vec![1, 2, 3])).unwrap();
        assert_eq!(request.await.unwrap().unwrap(), vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn frame_wait_is_woken_by_browser_publication() {
        let browser = BrowserVideoSlot::default();
        let service =
            DeviceControlService::<String>::new(browser.clone(), SessionCommandSlot::default());
        let waiter = tokio::spawn({
            let service = service.clone();
            async move { service.wait_for_frame(0, Duration::from_secs(1)).await }
        });
        tokio::task::yield_now().await;
        browser.publish(0, true, 100, 200, vec![0, 0, 0, 1, 0x26]);
        assert!(waiter.await.unwrap());
    }
}
