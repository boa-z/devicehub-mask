//! Compressed media publication and host-consumer flow control.

mod browser_video;
mod flow_control;
mod rtcp;
mod session;
mod watchdog;

pub use browser_video::{BrowserVideoFrame, BrowserVideoSlot, encode_packet, hevc_dimensions};
pub use flow_control::{
    BrowserFrameDecision, FrameCredit, FramePacer, FramePacerMetrics, browser_frame_decision,
    configured_in_flight_frames, duration_average_ms,
};
pub use rtcp::{RtcpShared, receive_task as receive_rtcp, send_task as send_rtcp};
pub use session::{
    AccessUnitAssembler, HEVC_QUEUE_MAX_BYTES, HevcAccessUnit, HevcQueue, HevcQueuePush,
    HevcQueueSnapshot, RtpVideoClock, RunningStats, audio_decoder_restart_backoff,
};
pub use watchdog::stall_watchdog;
