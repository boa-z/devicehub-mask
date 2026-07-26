//! Compressed media publication and host-consumer flow control.

mod audio_rtp;
mod browser_video;
mod flow_control;
mod orchestrator;
mod rtcp;
mod session;
mod video_rtp;
mod watchdog;

pub use browser_video::{BrowserVideoFrame, BrowserVideoSlot, encode_packet};
pub(crate) use browser_video::{forward_keyframe_requests, publish_hevc_queue};
pub use flow_control::{
    BrowserFrameDecision, FrameCredit, FramePacer, FramePacerMetrics, browser_frame_decision,
    configured_in_flight_frames, duration_average_ms,
};
pub(crate) use negotiation::start_screen_media_stream;
pub(crate) use orchestrator::{MediaSessionConfig, MediaSessionRuntime};
pub use rtcp::RtcpOptions;
pub(crate) use rtcp::{RtcpShared, receive_task as receive_rtcp, send_task as send_rtcp};
pub use session::audio_decoder_restart_backoff;
pub(crate) use session::{HEVC_QUEUE_MAX_BYTES, HevcQueue};
pub(crate) use video_rtp::{VideoRtpOptions, receive_video_rtp};
pub(crate) use watchdog::stall_watchdog;
mod negotiation;
pub(crate) use audio_rtp::receive_audio_rtp;
