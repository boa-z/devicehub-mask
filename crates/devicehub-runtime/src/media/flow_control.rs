//! Bounded WebCodecs ingress flow control for the private WebSocket transport.
//!
//! This module owns only transport pacing and acknowledgement telemetry. It
//! deliberately has no access to Axum, device services, or session resources,
//! so congestion policy can be tested without constructing the HTTP adapter.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const FRAME_CREDIT_LEASE: Duration = Duration::from_millis(500);
const PRESENTATION_SAMPLE_LEASE: Duration = Duration::from_secs(5);
// Match the frontend decoder's eight-packet hard limit. Missing ingress ACKs
// then indicate real queue saturation rather than a normal access-unit burst.
const DEFAULT_IN_FLIGHT_FRAMES: usize = 8;
const MAX_IN_FLIGHT_FRAMES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameCredit {
    BrowserAccepted(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserFrameDecision {
    Send { completes_resync: bool },
    SkipForResync,
    Backpressured { entered_resync: bool },
}

pub fn browser_frame_decision(
    key: bool,
    sequence: u64,
    resync: &AtomicBool,
    pacer: &FramePacer,
) -> BrowserFrameDecision {
    let completes_resync = resync.load(Ordering::Acquire);
    if completes_resync && !key {
        return BrowserFrameDecision::SkipForResync;
    }
    if !pacer.try_acquire(FrameCredit::BrowserAccepted(sequence)) {
        return BrowserFrameDecision::Backpressured {
            entered_resync: !resync.swap(true, Ordering::AcqRel),
        };
    }
    BrowserFrameDecision::Send { completes_resync }
}

pub fn configured_in_flight_frames(value: Option<&std::ffi::OsStr>) -> usize {
    let Some(value) = value
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
    else {
        return DEFAULT_IN_FLIGHT_FRAMES;
    };
    match value.parse::<usize>() {
        Ok(value) if (1..=MAX_IN_FLIGHT_FRAMES).contains(&value) => value,
        _ => {
            tracing::warn!(value, "ignoring invalid DEVICEHUB_VIDEO_IN_FLIGHT_FRAMES");
            DEFAULT_IN_FLIGHT_FRAMES
        }
    }
}

pub fn duration_average_ms(total: Duration, samples: u64) -> f64 {
    if samples == 0 {
        0.0
    } else {
        total.as_secs_f64() * 1000.0 / samples as f64
    }
}

#[derive(Debug, Clone, Copy)]
struct PendingFrameCredit {
    kind: FrameCredit,
    acquired_at: Instant,
}

#[derive(Default)]
struct AckStats {
    samples: u64,
    total: Duration,
    max: Duration,
}

impl AckStats {
    fn record(&mut self, elapsed: Duration) {
        self.samples = self.samples.saturating_add(1);
        self.total += elapsed;
        self.max = self.max.max(elapsed);
    }

    fn snapshot_and_reset(&mut self) -> (f64, f64) {
        let average_ms = duration_average_ms(self.total, self.samples);
        let max_ms = self.max.as_secs_f64() * 1000.0;
        *self = Self::default();
        (average_ms, max_ms)
    }
}

#[derive(Default)]
struct FramePacerState {
    pending: VecDeque<PendingFrameCredit>,
    browser_presentations: HashMap<u64, Instant>,
    decoder_accept: AckStats,
    presentation: AckStats,
    expired_credits: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct FramePacerMetrics {
    pub decoder_accept_average_ms: f64,
    pub decoder_accept_max_ms: f64,
    pub presentation_average_ms: f64,
    pub presentation_max_ms: f64,
    pub expired_credits: u64,
}

pub struct FramePacer {
    max_in_flight: usize,
    state: Mutex<FramePacerState>,
}

impl FramePacer {
    pub fn new(max_in_flight: usize) -> Self {
        Self {
            max_in_flight,
            state: Mutex::new(FramePacerState::default()),
        }
    }

    pub fn try_acquire(&self, kind: FrameCredit) -> bool {
        let mut state = self.state.lock().expect("frame pacer lock poisoned");
        while state
            .pending
            .front()
            .is_some_and(|credit| credit.acquired_at.elapsed() >= FRAME_CREDIT_LEASE)
        {
            let expired = state.pending.pop_front().expect("pending credit exists");
            let FrameCredit::BrowserAccepted(sequence) = expired.kind;
            state.browser_presentations.remove(&sequence);
            state.expired_credits = state.expired_credits.saturating_add(1);
        }
        state
            .browser_presentations
            .retain(|_, sent_at| sent_at.elapsed() < PRESENTATION_SAMPLE_LEASE);
        if state.pending.len() >= self.max_in_flight {
            return false;
        }
        let acquired_at = Instant::now();
        state
            .pending
            .push_back(PendingFrameCredit { kind, acquired_at });
        let FrameCredit::BrowserAccepted(sequence) = kind;
        state.browser_presentations.insert(sequence, acquired_at);
        true
    }

    pub fn release(&self, kind: FrameCredit) {
        let mut state = self.state.lock().expect("frame pacer lock poisoned");
        let Some(index) = state
            .pending
            .iter()
            .position(|pending| pending.kind == kind)
        else {
            return;
        };
        let pending = state.pending.remove(index).expect("matching credit exists");
        state.decoder_accept.record(pending.acquired_at.elapsed());
    }

    pub fn presented(&self, sequence: u64) {
        let mut state = self.state.lock().expect("frame pacer lock poisoned");
        if let Some(sent_at) = state.browser_presentations.remove(&sequence) {
            state.presentation.record(sent_at.elapsed());
        }
    }

    pub fn clear_browser(&self) {
        let mut state = self.state.lock().expect("frame pacer lock poisoned");
        state.pending.clear();
        state.browser_presentations.clear();
    }

    pub fn clear(&self) {
        self.clear_browser();
    }

    pub fn take_metrics(&self) -> FramePacerMetrics {
        let mut state = self.state.lock().expect("frame pacer lock poisoned");
        let (decoder_accept_average_ms, decoder_accept_max_ms) =
            state.decoder_accept.snapshot_and_reset();
        let (presentation_average_ms, presentation_max_ms) =
            state.presentation.snapshot_and_reset();
        let metrics = FramePacerMetrics {
            decoder_accept_average_ms,
            decoder_accept_max_ms,
            presentation_average_ms,
            presentation_max_ms,
            expired_credits: state.expired_credits,
        };
        state.expired_credits = 0;
        metrics
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_browser_decoder_ingress() {
        let pacer = FramePacer::new(2);
        assert!(pacer.try_acquire(FrameCredit::BrowserAccepted(1)));
        assert!(pacer.try_acquire(FrameCredit::BrowserAccepted(2)));
        assert!(!pacer.try_acquire(FrameCredit::BrowserAccepted(3)));

        pacer.release(FrameCredit::BrowserAccepted(1));
        assert!(pacer.try_acquire(FrameCredit::BrowserAccepted(3)));
        pacer.release(FrameCredit::BrowserAccepted(2));
        assert!(pacer.try_acquire(FrameCredit::BrowserAccepted(4)));
        assert!(!pacer.try_acquire(FrameCredit::BrowserAccepted(5)));
    }

    #[test]
    fn acceptance_must_match_and_presentation_is_telemetry_only() {
        let pacer = FramePacer::new(1);
        assert!(pacer.try_acquire(FrameCredit::BrowserAccepted(7)));
        pacer.release(FrameCredit::BrowserAccepted(6));
        assert!(!pacer.try_acquire(FrameCredit::BrowserAccepted(8)));
        pacer.presented(7);
        assert!(!pacer.try_acquire(FrameCredit::BrowserAccepted(8)));
        pacer.release(FrameCredit::BrowserAccepted(7));
        assert!(pacer.try_acquire(FrameCredit::BrowserAccepted(8)));
    }

    #[test]
    fn reset_clears_all_video_credits() {
        let pacer = FramePacer::new(2);
        assert!(pacer.try_acquire(FrameCredit::BrowserAccepted(1)));
        assert!(pacer.try_acquire(FrameCredit::BrowserAccepted(2)));
        pacer.clear_browser();
        assert!(pacer.try_acquire(FrameCredit::BrowserAccepted(3)));
        assert!(pacer.try_acquire(FrameCredit::BrowserAccepted(4)));
    }

    #[test]
    fn backpressure_resyncs_and_resumes_only_from_a_keyframe() {
        let pacer = FramePacer::new(1);
        let resync = AtomicBool::new(false);
        assert_eq!(
            browser_frame_decision(false, 1, &resync, &pacer),
            BrowserFrameDecision::Send {
                completes_resync: false
            }
        );
        assert_eq!(
            browser_frame_decision(false, 2, &resync, &pacer),
            BrowserFrameDecision::Backpressured {
                entered_resync: true
            }
        );
        pacer.release(FrameCredit::BrowserAccepted(1));
        assert_eq!(
            browser_frame_decision(false, 3, &resync, &pacer),
            BrowserFrameDecision::SkipForResync
        );
        assert_eq!(
            browser_frame_decision(true, 4, &resync, &pacer),
            BrowserFrameDecision::Send {
                completes_resync: true
            }
        );
    }

    #[test]
    fn configured_depth_accepts_only_bounded_diagnostic_values() {
        assert_eq!(configured_in_flight_frames(None), 8);
        assert_eq!(configured_in_flight_frames(Some("1".as_ref())), 1);
        assert_eq!(configured_in_flight_frames(Some("8".as_ref())), 8);
        assert_eq!(configured_in_flight_frames(Some("16".as_ref())), 8);
        assert_eq!(configured_in_flight_frames(Some("0".as_ref())), 8);
    }

    #[test]
    fn expired_credit_does_not_stall_stream() {
        let pacer = FramePacer {
            max_in_flight: 2,
            state: Mutex::new(FramePacerState {
                pending: VecDeque::from([PendingFrameCredit {
                    kind: FrameCredit::BrowserAccepted(1),
                    acquired_at: Instant::now() - FRAME_CREDIT_LEASE,
                }]),
                ..FramePacerState::default()
            }),
        };
        assert!(pacer.try_acquire(FrameCredit::BrowserAccepted(2)));
        assert!(pacer.try_acquire(FrameCredit::BrowserAccepted(3)));
        assert!(!pacer.try_acquire(FrameCredit::BrowserAccepted(4)));
    }
}
