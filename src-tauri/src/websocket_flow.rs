//! Compatibility imports for WebCodecs flow control owned by `devicehub-runtime`.

pub(crate) use devicehub_runtime::{
    BrowserFrameDecision, FrameCredit, FramePacer, browser_frame_decision,
    configured_in_flight_frames, duration_average_ms,
};
