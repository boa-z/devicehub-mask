//! Lifetime orchestration for one negotiated screen media session.

use std::future::Future;
use std::sync::{Arc, Mutex};

use devicehub_core::VideoCounters;
use idevice::tcp::handle::UdpSocketHandle;
use tokio::sync::Notify;

use super::{
    BrowserVideoSlot, HEVC_QUEUE_MAX_BYTES, HevcQueue, RtcpOptions, RtcpShared, VideoRtpOptions,
    forward_keyframe_requests, publish_hevc_queue, receive_rtcp, receive_video_rtp, send_rtcp,
    stall_watchdog,
};

/// Explicit media protocol options supplied by a host adapter.
pub struct MediaSessionConfig {
    pub our_ssrc: u32,
    pub cname: String,
    pub video: VideoRtpOptions,
    pub rtcp: RtcpOptions,
}

/// Runtime-owned transport and publication state for an active media session.
pub struct MediaSessionRuntime {
    video_udp: UdpSocketHandle,
    rtcp_udp: Option<UdpSocketHandle>,
    counters: VideoCounters,
    browser_frames: BrowserVideoSlot,
    config: MediaSessionConfig,
}

impl MediaSessionRuntime {
    pub fn new(
        video_udp: UdpSocketHandle,
        rtcp_udp: Option<UdpSocketHandle>,
        counters: VideoCounters,
        browser_frames: BrowserVideoSlot,
        config: MediaSessionConfig,
    ) -> Self {
        Self {
            video_udp,
            rtcp_udp,
            counters,
            browser_frames,
            config,
        }
    }

    /// Runs runtime media tasks beside host-provided session capabilities.
    /// Completion of any task ends the task set so the host can perform its
    /// ordered service and DisplayService teardown.
    pub async fn run<Audio, Clipboard, Orientation, Input>(
        self,
        audio: Audio,
        clipboard: Clipboard,
        orientation: Orientation,
        input: Input,
    ) where
        Audio: Future<Output = ()>,
        Clipboard: Future<Output = ()>,
        Orientation: Future<Output = ()>,
        Input: Future<Output = ()>,
    {
        let video_udp = Arc::new(self.video_udp);
        let rtcp_udp = self.rtcp_udp.map(Arc::new);
        let corruption = Arc::new(Notify::new());
        let rtcp = Arc::new(Mutex::new(RtcpShared::default()));
        let queue = Arc::new(HevcQueue::new(HEVC_QUEUE_MAX_BYTES));

        tokio::select! {
            _ = receive_video_rtp(
                video_udp.clone(),
                queue.clone(),
                rtcp.clone(),
                corruption.clone(),
                self.counters.clone(),
                self.config.our_ssrc,
                self.config.video,
            ) => tracing::warn!("video task ended early"),
            _ = audio => tracing::warn!("audio task ended early"),
            _ = publish_hevc_queue(
                queue,
                self.browser_frames.clone(),
                self.counters.clone(),
                corruption.clone(),
            ) => {},
            _ = stall_watchdog(self.counters.clone(), &corruption) => {},
            _ = forward_keyframe_requests(self.browser_frames, corruption.clone()) => {},
            _ = receive_rtcp(rtcp_udp.clone(), rtcp.clone(), self.counters) => {},
            _ = send_rtcp(
                video_udp,
                rtcp_udp,
                rtcp,
                self.config.our_ssrc,
                self.config.cname,
                self.config.rtcp,
                &corruption,
            ) => {},
            _ = clipboard => {},
            _ = orientation => {},
            _ = input => {},
        }
    }
}
