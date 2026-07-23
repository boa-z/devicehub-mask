use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use bytes::Bytes;
use rodio::{OutputStream, OutputStreamBuilder, Sink, buffer::SamplesBuffer};
use serde::Serialize;

use crate::protocol::{AUDIO_CHANNELS, AUDIO_SAMPLE_RATE};

const PCM_QUEUE_CAPACITY: usize = 16;
const MAX_QUEUED_CHUNKS: usize = 12;
const REOPEN_DELAY: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioOutputState {
    Idle,
    Running,
    Unavailable,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct AudioOutputStatus {
    pub state: AudioOutputState,
    pub muted: bool,
    pub volume: f32,
    pub dropped_chunks: u64,
}

struct SharedState {
    muted: AtomicBool,
    volume_bits: AtomicU32,
    output_state: Mutex<AudioOutputState>,
    dropped_chunks: AtomicU64,
    shutdown: AtomicBool,
}

struct AudioOutputInner {
    pcm_tx: mpsc::SyncSender<Bytes>,
    state: Arc<SharedState>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for AudioOutputInner {
    fn drop(&mut self) {
        self.state.shutdown.store(true, Ordering::Release);
        if let Some(thread) = self
            .thread
            .lock()
            .expect("audio thread lock poisoned")
            .take()
        {
            let _ = thread.join();
        }
    }
}

#[derive(Clone)]
pub struct AudioOutput(Arc<AudioOutputInner>);

impl AudioOutput {
    pub fn spawn(muted: bool, volume: f32) -> Result<Self, String> {
        let volume = validate_volume(volume)?;
        let (pcm_tx, pcm_rx) = mpsc::sync_channel(PCM_QUEUE_CAPACITY);
        let state = Arc::new(SharedState {
            muted: AtomicBool::new(muted),
            volume_bits: AtomicU32::new(volume.to_bits()),
            output_state: Mutex::new(AudioOutputState::Idle),
            dropped_chunks: AtomicU64::new(0),
            shutdown: AtomicBool::new(false),
        });
        let thread_state = state.clone();
        let thread = std::thread::Builder::new()
            .name("devicehub-audio-output".into())
            .spawn(move || run_audio_output(pcm_rx, thread_state))
            .map_err(|error| format!("cannot start native audio output thread: {error}"))?;
        Ok(Self(Arc::new(AudioOutputInner {
            pcm_tx,
            state,
            thread: Mutex::new(Some(thread)),
        })))
    }

    pub fn publish(&self, pcm: Bytes) {
        if self.0.state.muted.load(Ordering::Acquire) {
            return;
        }
        if self.0.pcm_tx.try_send(pcm).is_err() {
            let dropped = self.0.state.dropped_chunks.fetch_add(1, Ordering::Relaxed) + 1;
            if dropped == 1 || dropped.is_multiple_of(100) {
                tracing::debug!(
                    dropped_chunks = dropped,
                    "dropping native audio chunk due to backpressure"
                );
            }
        }
    }

    pub fn set_preferences(&self, muted: bool, volume: f32) -> Result<AudioOutputStatus, String> {
        let volume = validate_volume(volume)?;
        self.0
            .state
            .volume_bits
            .store(volume.to_bits(), Ordering::Release);
        self.0.state.muted.store(muted, Ordering::Release);
        tracing::info!(muted, volume, "native audio playback preferences changed");
        Ok(self.status())
    }

    pub fn status(&self) -> AudioOutputStatus {
        AudioOutputStatus {
            state: *self
                .0
                .state
                .output_state
                .lock()
                .expect("audio output state lock poisoned"),
            muted: self.0.state.muted.load(Ordering::Acquire),
            volume: f32::from_bits(self.0.state.volume_bits.load(Ordering::Acquire)),
            dropped_chunks: self.0.state.dropped_chunks.load(Ordering::Relaxed),
        }
    }
}

fn validate_volume(volume: f32) -> Result<f32, String> {
    if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
        return Err("audio volume must be a finite value between 0 and 1".into());
    }
    Ok(volume)
}

struct Playback {
    _stream: OutputStream,
    sink: Sink,
    failed: Arc<AtomicBool>,
}

fn open_playback() -> Result<Playback, String> {
    let failed = Arc::new(AtomicBool::new(false));
    let callback_failed = failed.clone();
    let stream = OutputStreamBuilder::from_default_device()
        .map(|builder| {
            builder.with_error_callback(move |error| {
                callback_failed.store(true, Ordering::Release);
                tracing::warn!(%error, "native audio output stream failed");
            })
        })
        .and_then(|builder| builder.open_stream_or_fallback())
        .map_err(|error| format!("cannot open the default audio output: {error}"))?;
    tracing::info!(
        channels = stream.config().channel_count(),
        sample_rate = stream.config().sample_rate(),
        sample_format = ?stream.config().sample_format(),
        "native audio output opened"
    );
    let sink = Sink::connect_new(stream.mixer());
    Ok(Playback {
        _stream: stream,
        sink,
        failed,
    })
}

fn run_audio_output(pcm_rx: mpsc::Receiver<Bytes>, state: Arc<SharedState>) {
    let mut playback: Option<Playback> = None;
    let mut next_open = Instant::now();
    let mut applied_muted = true;
    let mut applied_volume = f32::NAN;
    while !state.shutdown.load(Ordering::Acquire) {
        if playback
            .as_ref()
            .is_some_and(|active| active.failed.load(Ordering::Acquire))
        {
            playback = None;
            next_open = Instant::now() + REOPEN_DELAY;
            set_output_state(&state, AudioOutputState::Unavailable);
        }
        let muted = state.muted.load(Ordering::Acquire);
        let volume = f32::from_bits(state.volume_bits.load(Ordering::Acquire));
        if let Some(active) = playback.as_mut() {
            if muted != applied_muted {
                if muted {
                    active.sink.clear();
                }
                applied_muted = muted;
            }
            if volume != applied_volume {
                active.sink.set_volume(volume);
                applied_volume = volume;
            }
        }

        let pcm = match pcm_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(pcm) => pcm,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        if muted {
            continue;
        }
        if playback.is_none() {
            if Instant::now() < next_open {
                continue;
            }
            match open_playback() {
                Ok(active) => {
                    applied_muted = false;
                    applied_volume = volume;
                    active.sink.set_volume(volume);
                    playback = Some(active);
                    set_output_state(&state, AudioOutputState::Running);
                }
                Err(error) => {
                    tracing::warn!(%error, retry_ms = REOPEN_DELAY.as_millis() as u64, "native audio output unavailable");
                    set_output_state(&state, AudioOutputState::Unavailable);
                    next_open = Instant::now() + REOPEN_DELAY;
                    continue;
                }
            }
        }

        let Some(active) = playback.as_mut() else {
            continue;
        };
        if active.sink.len() >= MAX_QUEUED_CHUNKS {
            let stale = active.sink.len();
            active.sink.clear();
            state
                .dropped_chunks
                .fetch_add(stale as u64, Ordering::Relaxed);
            tracing::debug!(
                stale_chunks = stale,
                "cleared stale native audio playback queue"
            );
        }
        let samples = pcm_s16le_to_f32(&pcm);
        if !samples.is_empty() {
            active.sink.append(SamplesBuffer::new(
                AUDIO_CHANNELS as u16,
                AUDIO_SAMPLE_RATE,
                samples,
            ));
        }
    }
}

fn set_output_state(state: &SharedState, next: AudioOutputState) {
    *state
        .output_state
        .lock()
        .expect("audio output state lock poisoned") = next;
}

fn pcm_s16le_to_f32(pcm: &[u8]) -> Vec<f32> {
    pcm.chunks_exact(2)
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32_768.0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_signed_little_endian_pcm() {
        let samples = pcm_s16le_to_f32(&[0x00, 0x80, 0x00, 0x00, 0xff, 0x7f, 0xaa]);
        assert_eq!(samples.len(), 3);
        assert_eq!(samples[0], -1.0);
        assert_eq!(samples[1], 0.0);
        assert!((samples[2] - (32_767.0 / 32_768.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn rejects_invalid_volume() {
        assert!(validate_volume(f32::NAN).is_err());
        assert!(validate_volume(-0.1).is_err());
        assert!(validate_volume(1.1).is_err());
        assert_eq!(validate_volume(0.8).unwrap(), 0.8);
    }
}
