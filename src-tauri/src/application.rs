//! Application services shared by HTTP, WebSocket, and MCP adapters.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use crate::browser_video::BrowserVideoSlot;
use crate::protocol::{Frame, FrameSlot, InputCmd, InputSink};

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

#[derive(Clone)]
pub struct DeviceControlService {
    frames: FrameSlot,
    browser_frames: BrowserVideoSlot,
    input: InputSink,
}

impl DeviceControlService {
    pub fn new(frames: FrameSlot, browser_frames: BrowserVideoSlot, input: InputSink) -> Self {
        Self {
            frames,
            browser_frames,
            input,
        }
    }

    pub fn send(&self, command: InputCmd) -> Result<(), DeviceControlError> {
        self.input
            .try_send(command)
            .then_some(())
            .ok_or(DeviceControlError::Unavailable)
    }

    pub fn frame_version(&self) -> u64 {
        self.frames
            .version()
            .saturating_add(self.browser_frames.version())
    }

    pub fn latest_frame(&self) -> Option<(u64, Arc<Frame>)> {
        self.frames.latest()
    }

    pub fn browser_dimensions(&self) -> Option<(u32, u32)> {
        self.browser_frames.dimensions()
    }

    pub async fn capture_screenshot(
        &self,
        timeout: Duration,
    ) -> Result<Vec<u8>, DeviceControlError> {
        let (reply, response) = tokio::sync::oneshot::channel();
        self.send(InputCmd::TakeScreenshot(reply))?;
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
        let mut native = self.frames.subscribe();
        let mut browser = self.browser_frames.subscribe();
        // Close the publication race between the initial version check and
        // installing both subscriptions.
        if self.frame_version() > after {
            return true;
        }
        tokio::time::timeout(timeout, async {
            loop {
                tokio::select! {
                    changed = native.changed() => {
                        if changed.is_err() {
                            return false;
                        }
                    }
                    changed = browser.recv() => {
                        if matches!(changed, Err(tokio::sync::broadcast::error::RecvError::Closed)) {
                            return false;
                        }
                    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{FrameFormat, InputSink};
    use std::sync::OnceLock;
    use std::time::Instant;
    use tokio::sync::mpsc::unbounded_channel;

    fn service() -> (
        DeviceControlService,
        tokio::sync::mpsc::UnboundedReceiver<InputCmd>,
    ) {
        let input = InputSink::default();
        let (sender, receiver) = unbounded_channel();
        input.set(Some(sender));
        (
            DeviceControlService::new(FrameSlot::default(), BrowserVideoSlot::default(), input),
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
        let InputCmd::TakeScreenshot(reply) = commands.recv().await.unwrap() else {
            panic!("expected screenshot command");
        };
        reply.send(Ok(vec![1, 2, 3])).unwrap();
        assert_eq!(request.await.unwrap().unwrap(), vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn frame_wait_is_woken_by_native_publication() {
        let frames = FrameSlot::default();
        let service = DeviceControlService::new(
            frames.clone(),
            BrowserVideoSlot::default(),
            InputSink::default(),
        );
        let waiter = tokio::spawn({
            let service = service.clone();
            async move { service.wait_for_frame(0, Duration::from_secs(1)).await }
        });
        tokio::task::yield_now().await;
        frames.publish(Arc::new(Frame {
            width: 1,
            height: 1,
            format: FrameFormat::Rgb24,
            pixels: vec![0, 0, 0],
            decoded_at: Instant::now(),
            jpeg: OnceLock::new(),
        }));
        assert!(waiter.await.unwrap());
    }

    #[tokio::test]
    async fn frame_wait_is_woken_by_browser_publication() {
        let browser = BrowserVideoSlot::default();
        let service =
            DeviceControlService::new(FrameSlot::default(), browser.clone(), InputSink::default());
        let waiter = tokio::spawn({
            let service = service.clone();
            async move { service.wait_for_frame(0, Duration::from_secs(1)).await }
        });
        tokio::task::yield_now().await;
        browser.publish(0, true, 100, 200, vec![0, 0, 0, 1, 0x26]);
        assert!(waiter.await.unwrap());
    }
}
