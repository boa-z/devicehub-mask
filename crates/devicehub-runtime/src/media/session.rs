use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tokio::sync::Notify;

const HEVC_AUD: &[u8] = b"\0\0\0\x01\x46\x01\x50";
/// Bound compressed video waiting for the WebSocket/WebCodecs publisher.
/// This is deliberately byte-based: access-unit sizes vary dramatically
/// between static P-frames and an IRAP.
pub const HEVC_QUEUE_MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct RunningStats {
    count: u64,
    mean: f64,
    squared_deviations: f64,
    min: f64,
    max: f64,
}

impl Default for RunningStats {
    fn default() -> Self {
        Self {
            count: 0,
            mean: 0.0,
            squared_deviations: 0.0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
        }
    }
}

impl RunningStats {
    pub fn push(&mut self, value: f64) {
        self.count += 1;
        let delta = value - self.mean;
        self.mean += delta / self.count as f64;
        self.squared_deviations += delta * (value - self.mean);
        self.min = self.min.min(value);
        self.max = self.max.max(value);
    }

    pub fn mean(&self) -> Option<f64> {
        (self.count > 0).then_some(self.mean)
    }

    pub fn min(&self) -> Option<f64> {
        (self.count > 0).then_some(self.min)
    }

    pub fn max(&self) -> Option<f64> {
        (self.count > 0).then_some(self.max)
    }

    pub fn standard_deviation(&self) -> Option<f64> {
        (self.count > 0).then(|| (self.squared_deviations / self.count as f64).sqrt())
    }
}

#[derive(Debug)]
pub struct HevcAccessUnit {
    pub bytes: Vec<u8>,
    pub is_irap: bool,
    pub rtp_timestamp: u32,
}

#[derive(Debug, Default)]
pub struct AccessUnitAssembler {
    pending: Vec<u8>,
    pending_timestamp: Option<u32>,
}

impl AccessUnitAssembler {
    pub fn push(&mut self, bytes: &[u8], rtp_timestamp: u32) -> Vec<HevcAccessUnit> {
        if self.pending.is_empty() {
            self.pending_timestamp = Some(rtp_timestamp);
        }
        self.pending.extend_from_slice(bytes);
        let mut completed = Vec::new();
        loop {
            // The depacketizer inserts an AUD before each new RTP timestamp. If
            // pending already starts with one, search for the following AUD.
            let search_from = usize::from(self.pending.starts_with(HEVC_AUD)) * HEVC_AUD.len();
            let Some(relative_boundary) = find_subslice(&self.pending[search_from..], HEVC_AUD)
            else {
                break;
            };
            let boundary = search_from + relative_boundary;
            let remaining = self.pending.split_off(boundary);
            let access_unit = std::mem::replace(&mut self.pending, remaining);
            if !access_unit.is_empty() {
                completed.push(HevcAccessUnit {
                    is_irap: annexb_contains_irap(&access_unit),
                    bytes: access_unit,
                    rtp_timestamp: self.pending_timestamp.unwrap_or(rtp_timestamp),
                });
            }
            self.pending_timestamp = Some(rtp_timestamp);
        }
        completed
    }

    pub fn finish(&mut self) -> Option<HevcAccessUnit> {
        if self.pending.is_empty() {
            return None;
        }
        let bytes = std::mem::take(&mut self.pending);
        Some(HevcAccessUnit {
            is_irap: annexb_contains_irap(&bytes),
            bytes,
            rtp_timestamp: self.pending_timestamp.take()?,
        })
    }

    pub fn clear(&mut self) {
        self.pending.clear();
        self.pending_timestamp = None;
    }
}

#[derive(Debug, Default)]
pub struct RtpVideoClock {
    last_timestamp: Option<u32>,
    elapsed_ticks: u64,
}

impl RtpVideoClock {
    pub fn timestamp_us(&mut self, timestamp: u32) -> u64 {
        if let Some(previous) = self.last_timestamp {
            let delta = timestamp.wrapping_sub(previous);
            if delta < (1 << 31) {
                self.elapsed_ticks = self.elapsed_ticks.saturating_add(u64::from(delta));
            }
        }
        self.last_timestamp = Some(timestamp);
        self.elapsed_ticks.saturating_mul(1_000_000) / 90_000
    }
}

#[derive(Debug)]
struct QueuedHevcAccessUnit {
    access_unit: HevcAccessUnit,
    enqueued_at: Instant,
}

#[derive(Debug)]
pub enum HevcQueuePush {
    Enqueued,
    Dropped,
    NeedsKeyframe {
        queued_bytes: usize,
        incoming_bytes: usize,
    },
    Recovered {
        dropped_access_units: u64,
        dropped_bytes: u64,
    },
}

#[derive(Debug)]
struct HevcQueueState {
    access_units: VecDeque<QueuedHevcAccessUnit>,
    queued_bytes: usize,
    peak_bytes: usize,
    waiting_for_irap: bool,
    dropped_access_units: u64,
    dropped_bytes: u64,
    wait_samples: u64,
    wait_total_micros: u64,
    wait_max_micros: u64,
    closed: bool,
}

#[derive(Debug)]
pub struct HevcQueue {
    max_bytes: usize,
    state: Mutex<HevcQueueState>,
    ready: Notify,
}

impl HevcQueue {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            state: Mutex::new(HevcQueueState {
                access_units: VecDeque::new(),
                queued_bytes: 0,
                peak_bytes: 0,
                waiting_for_irap: false,
                dropped_access_units: 0,
                dropped_bytes: 0,
                wait_samples: 0,
                wait_total_micros: 0,
                wait_max_micros: 0,
                closed: false,
            }),
            ready: Notify::new(),
        }
    }

    pub fn push(&self, access_unit: HevcAccessUnit) -> HevcQueuePush {
        let incoming_bytes = access_unit.bytes.len();
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return HevcQueuePush::Dropped;
        }

        if state.waiting_for_irap {
            if !access_unit.is_irap || incoming_bytes > self.max_bytes {
                state.dropped_access_units = state.dropped_access_units.saturating_add(1);
                state.dropped_bytes = state.dropped_bytes.saturating_add(incoming_bytes as u64);
                return HevcQueuePush::Dropped;
            }
            state.waiting_for_irap = false;
            let dropped_access_units = std::mem::take(&mut state.dropped_access_units);
            let dropped_bytes = std::mem::take(&mut state.dropped_bytes);
            state.queued_bytes = incoming_bytes;
            state.peak_bytes = state.peak_bytes.max(state.queued_bytes);
            state.access_units.push_back(QueuedHevcAccessUnit {
                access_unit,
                enqueued_at: Instant::now(),
            });
            drop(state);
            self.ready.notify_one();
            return HevcQueuePush::Recovered {
                dropped_access_units,
                dropped_bytes,
            };
        }

        if incoming_bytes > self.max_bytes
            || state.queued_bytes.saturating_add(incoming_bytes) > self.max_bytes
        {
            let queued_bytes = state.queued_bytes;
            state.dropped_access_units = state
                .dropped_access_units
                .saturating_add(state.access_units.len() as u64);
            state.dropped_bytes = state.dropped_bytes.saturating_add(queued_bytes as u64);
            state.access_units.clear();
            state.queued_bytes = 0;

            if access_unit.is_irap && incoming_bytes <= self.max_bytes {
                let dropped_access_units = std::mem::take(&mut state.dropped_access_units);
                let dropped_bytes = std::mem::take(&mut state.dropped_bytes);
                state.access_units.push_back(QueuedHevcAccessUnit {
                    access_unit,
                    enqueued_at: Instant::now(),
                });
                state.queued_bytes = incoming_bytes;
                state.peak_bytes = state.peak_bytes.max(state.queued_bytes);
                drop(state);
                self.ready.notify_one();
                return HevcQueuePush::Recovered {
                    dropped_access_units,
                    dropped_bytes,
                };
            }

            state.waiting_for_irap = true;
            state.dropped_access_units = state.dropped_access_units.saturating_add(1);
            state.dropped_bytes = state.dropped_bytes.saturating_add(incoming_bytes as u64);
            return HevcQueuePush::NeedsKeyframe {
                queued_bytes,
                incoming_bytes,
            };
        }

        state.queued_bytes += incoming_bytes;
        state.peak_bytes = state.peak_bytes.max(state.queued_bytes);
        state.access_units.push_back(QueuedHevcAccessUnit {
            access_unit,
            enqueued_at: Instant::now(),
        });
        drop(state);
        self.ready.notify_one();
        HevcQueuePush::Enqueued
    }

    pub fn force_resync(&self) -> (u64, u64) {
        let mut state = self.state.lock().unwrap();
        state.dropped_access_units = state
            .dropped_access_units
            .saturating_add(state.access_units.len() as u64);
        state.dropped_bytes = state
            .dropped_bytes
            .saturating_add(state.queued_bytes as u64);
        state.access_units.clear();
        state.queued_bytes = 0;
        state.waiting_for_irap = true;
        (state.dropped_access_units, state.dropped_bytes)
    }

    pub async fn pop(&self) -> Option<HevcAccessUnit> {
        loop {
            let notified = self.ready.notified();
            {
                let mut state = self.state.lock().unwrap();
                if let Some(queued) = state.access_units.pop_front() {
                    state.queued_bytes -= queued.access_unit.bytes.len();
                    let wait_micros = queued.enqueued_at.elapsed().as_micros() as u64;
                    state.wait_samples = state.wait_samples.saturating_add(1);
                    state.wait_total_micros = state.wait_total_micros.saturating_add(wait_micros);
                    state.wait_max_micros = state.wait_max_micros.max(wait_micros);
                    return Some(queued.access_unit);
                }
                if state.closed {
                    return None;
                }
            }
            notified.await;
        }
    }

    pub fn take_snapshot(&self) -> HevcQueueSnapshot {
        let mut state = self.state.lock().unwrap();
        let snapshot = HevcQueueSnapshot {
            queued_access_units: state.access_units.len(),
            queued_bytes: state.queued_bytes,
            peak_bytes: state.peak_bytes,
            waiting_for_irap: state.waiting_for_irap,
            wait_ms: if state.wait_samples == 0 {
                0.0
            } else {
                state.wait_total_micros as f64 / state.wait_samples as f64 / 1000.0
            },
            wait_max_ms: state.wait_max_micros as f64 / 1000.0,
        };
        state.peak_bytes = state.queued_bytes;
        state.wait_samples = 0;
        state.wait_total_micros = 0;
        state.wait_max_micros = 0;
        snapshot
    }

    pub fn close(&self) {
        self.state.lock().unwrap().closed = true;
        self.ready.notify_waiters();
    }
}

#[derive(Debug, Clone, Copy)]
pub struct HevcQueueSnapshot {
    pub queued_access_units: usize,
    pub queued_bytes: usize,
    pub peak_bytes: usize,
    pub waiting_for_irap: bool,
    pub wait_ms: f64,
    pub wait_max_ms: f64,
}

pub fn audio_decoder_restart_backoff(attempt: u32) -> Duration {
    Duration::from_millis((250_u64.saturating_mul(1_u64 << attempt.min(4))).min(4_000))
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn annexb_contains_irap(bytes: &[u8]) -> bool {
    bytes
        .windows(5)
        .any(|window| window[..4] == [0, 0, 0, 1] && (16..=23).contains(&((window[4] >> 1) & 0x3f)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_stats_reports_mean_range_and_jitter() {
        let mut stats = RunningStats::default();
        stats.push(10.0);
        stats.push(20.0);
        stats.push(30.0);

        assert_eq!(stats.mean(), Some(20.0));
        assert_eq!(stats.min(), Some(10.0));
        assert_eq!(stats.max(), Some(30.0));
        assert!((stats.standard_deviation().unwrap() - 8.164_965_809).abs() < 1e-6);
    }

    #[test]
    fn assembles_access_units_across_split_aud_boundaries() {
        let first = [0, 0, 0, 1, 0x02, 0x01, 0xaa];
        let second = [0, 0, 0, 1, 0x26, 0x01, 0xbb];
        let mut assembler = AccessUnitAssembler::default();

        let mut first_chunk = first.to_vec();
        first_chunk.extend_from_slice(&HEVC_AUD[..3]);
        assert!(assembler.push(&first_chunk, 90_000).is_empty());

        let mut second_chunk = HEVC_AUD[3..].to_vec();
        second_chunk.extend_from_slice(&second);
        let completed = assembler.push(&second_chunk, 91_500);
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].bytes, first);
        assert!(!completed[0].is_irap);
        assert_eq!(completed[0].rtp_timestamp, 90_000);

        let completed = assembler.push(HEVC_AUD, 93_000);
        assert_eq!(completed.len(), 1);
        assert!(completed[0].bytes.starts_with(HEVC_AUD));
        assert!(completed[0].is_irap);
        assert_eq!(completed[0].rtp_timestamp, 91_500);
    }

    #[test]
    fn finishes_access_unit_at_complete_rtp_marker() {
        let irap = [0, 0, 0, 1, 0x26, 0x01, 0xbb];
        let mut assembler = AccessUnitAssembler::default();

        assert!(assembler.push(&irap, 123_456).is_empty());
        let completed = assembler.finish().unwrap();
        assert_eq!(completed.bytes, irap);
        assert!(completed.is_irap);
        assert_eq!(completed.rtp_timestamp, 123_456);
        assert!(assembler.finish().is_none());
    }

    #[test]
    fn browser_video_clock_preserves_source_cadence_and_wraps() {
        let mut clock = RtpVideoClock::default();
        assert_eq!(clock.timestamp_us(u32::MAX - 749), 0);
        assert_eq!(clock.timestamp_us(u32::MAX), 8_322);
        assert_eq!(clock.timestamp_us(749), 16_655);
        assert_eq!(clock.timestamp_us(1_499), 24_988);
    }

    #[test]
    fn audio_restart_backoff_is_bounded() {
        assert_eq!(audio_decoder_restart_backoff(0), Duration::from_millis(250));
        assert_eq!(audio_decoder_restart_backoff(1), Duration::from_millis(500));
        assert_eq!(audio_decoder_restart_backoff(4), Duration::from_secs(4));
        assert_eq!(audio_decoder_restart_backoff(20), Duration::from_secs(4));
    }

    fn access_unit(size: usize, is_irap: bool) -> HevcAccessUnit {
        HevcAccessUnit {
            bytes: vec![0x5a; size],
            is_irap,
            rtp_timestamp: 0,
        }
    }

    #[tokio::test]
    async fn bounded_queue_recovers_only_at_irap() {
        let queue = HevcQueue::new(10);
        assert!(matches!(
            queue.push(access_unit(6, false)),
            HevcQueuePush::Enqueued
        ));
        assert!(matches!(
            queue.push(access_unit(6, false)),
            HevcQueuePush::NeedsKeyframe {
                queued_bytes: 6,
                incoming_bytes: 6,
            }
        ));
        assert!(matches!(
            queue.push(access_unit(2, false)),
            HevcQueuePush::Dropped
        ));
        assert!(matches!(
            queue.push(access_unit(4, true)),
            HevcQueuePush::Recovered {
                dropped_access_units: 3,
                dropped_bytes: 14,
            }
        ));

        let recovered = queue.pop().await.unwrap();
        assert!(recovered.is_irap);
        assert_eq!(recovered.bytes.len(), 4);
        queue.close();
        assert!(queue.pop().await.is_none());
    }
}
