//! Compressed media publication and host-consumer flow control.

mod audio_rtp;
mod browser_video;
mod flow_control;
mod orchestrator;
mod rtcp;
mod session;
mod video_rtp;
mod watchdog;

pub use browser_video::{
    BrowserVideoFrame, BrowserVideoSlot, encode_packet, forward_keyframe_requests, hevc_dimensions,
    publish_hevc_queue,
};
pub use flow_control::{
    BrowserFrameDecision, FrameCredit, FramePacer, FramePacerMetrics, browser_frame_decision,
    configured_in_flight_frames, duration_average_ms,
};
pub use negotiation::{ScreenMediaStream, start_screen_media_stream};
pub use orchestrator::{MediaSessionConfig, MediaSessionRuntime};
pub use rtcp::{RtcpOptions, RtcpShared, receive_task as receive_rtcp, send_task as send_rtcp};
pub use session::{
    AccessUnitAssembler, HEVC_QUEUE_MAX_BYTES, HevcAccessUnit, HevcQueue, HevcQueuePush,
    HevcQueueSnapshot, RtpVideoClock, RunningStats, audio_decoder_restart_backoff,
};
pub use video_rtp::{VideoRtpOptions, receive_video_rtp};
pub use watchdog::stall_watchdog;
mod negotiation;
pub use audio_rtp::receive_audio_rtp;
