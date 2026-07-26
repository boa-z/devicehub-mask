//! Video transport and decoder liveness policy.

use std::time::Duration;

use devicehub_core::{VideoCounterSnapshot, VideoCounters};
use tokio::sync::Notify;

const WATCHDOG_INTERVAL: Duration = Duration::from_secs(3);
const TRANSPORT_SILENT_WINDOWS: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatchdogObservation {
    Decoded,
    SourceWithoutDecode,
    TransportOnly,
    Silent,
}

fn observation(
    previous: VideoCounterSnapshot,
    current: VideoCounterSnapshot,
) -> WatchdogObservation {
    if current.decoded_frames != previous.decoded_frames {
        WatchdogObservation::Decoded
    } else if current.source_frames != previous.source_frames {
        WatchdogObservation::SourceWithoutDecode
    } else if current.transport_events != previous.transport_events {
        WatchdogObservation::TransportOnly
    } else {
        WatchdogObservation::Silent
    }
}

/// Recover only from evidence of a decoder stall or a genuinely silent transport.
/// RTCP-only activity is healthy for a static screen and must not trigger PLI.
pub async fn stall_watchdog(counters: VideoCounters, corruption: &Notify) {
    let mut previous = counters.snapshot();
    let mut silent_windows = 0_u8;
    loop {
        tokio::time::sleep(WATCHDOG_INTERVAL).await;
        let current = counters.snapshot();
        match observation(previous, current) {
            WatchdogObservation::Decoded | WatchdogObservation::TransportOnly => {
                silent_windows = 0;
            }
            WatchdogObservation::SourceWithoutDecode => {
                silent_windows = 0;
                tracing::warn!(
                    interval_ms = WATCHDOG_INTERVAL.as_millis() as u64,
                    "video source advanced without decoded output; requesting keyframe"
                );
                corruption.notify_one();
            }
            WatchdogObservation::Silent => {
                silent_windows = silent_windows.saturating_add(1);
                if silent_windows >= TRANSPORT_SILENT_WINDOWS {
                    tracing::warn!(
                        silent_ms =
                            WATCHDOG_INTERVAL.as_millis() as u64 * u64::from(silent_windows),
                        "video RTP/RTCP transport is silent; requesting keyframe"
                    );
                    corruption.notify_one();
                    silent_windows = 0;
                }
            }
        }
        previous = current;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(
        transport_events: u64,
        source_frames: u64,
        decoded_frames: u64,
    ) -> VideoCounterSnapshot {
        VideoCounterSnapshot {
            transport_events,
            source_frames,
            decoded_frames,
        }
    }

    #[test]
    fn static_transport_is_distinct_from_decoder_stalls() {
        let previous = snapshot(10, 5, 5);
        assert_eq!(
            observation(previous, snapshot(11, 5, 5)),
            WatchdogObservation::TransportOnly
        );
        assert_eq!(
            observation(previous, snapshot(12, 6, 5)),
            WatchdogObservation::SourceWithoutDecode
        );
        assert_eq!(
            observation(previous, snapshot(12, 6, 6)),
            WatchdogObservation::Decoded
        );
        assert_eq!(observation(previous, previous), WatchdogObservation::Silent);
    }
}
