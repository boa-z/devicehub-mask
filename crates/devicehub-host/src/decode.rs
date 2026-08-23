//! AAC-ELD audio decoding through the bundled FFmpeg subprocess.
//!
//! Live HEVC video is forwarded to WebCodecs and is intentionally not decoded
//! in the Rust backend.

use std::ffi::OsString;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdout, Command};

use devicehub_core::{AUDIO_CHANNELS, AUDIO_SAMPLE_RATE, ActiveSlot};
use devicehub_runtime::{AudioPublisher, Demand};
use devicehub_runtime::{DeviceAudioSource, audio_decoder_restart_backoff};

const AUDIO_CHUNK_MILLIS: usize = 20;
const AUDIO_DIAGNOSTIC_CHUNKS: u64 = 5_000 / AUDIO_CHUNK_MILLIS as u64;
const AUDIO_ACTIVE_SAMPLE_THRESHOLD: i32 = 32;
const AUDIO_DECODER_STABLE_RUNTIME: Duration = Duration::from_secs(10);
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Debug)]
pub struct AudioDecoderConfig {
    candidates: Arc<[PathBuf]>,
}

#[derive(Clone)]
pub struct FfmpegAudioPipeline {
    output: AudioPublisher,
    decoder: AudioDecoderConfig,
    enabled: bool,
    activation: AudioActivation,
}

#[derive(Clone)]
enum AudioActivation {
    Selected {
        active: ActiveSlot,
        selection_id: Arc<str>,
    },
    Demand(Demand),
}

impl AudioActivation {
    fn enabled(&self) -> bool {
        match self {
            Self::Selected {
                active,
                selection_id,
            } => active.selection_id().as_deref() == Some(selection_id.as_ref()),
            Self::Demand(demand) => demand.enabled(),
        }
    }

    async fn wait_for(&self, enabled: bool) {
        match self {
            Self::Demand(demand) => {
                let mut receiver = demand.subscribe();
                while *receiver.borrow() != enabled {
                    if receiver.changed().await.is_err() {
                        return;
                    }
                }
            }
            Self::Selected { .. } => {
                while self.enabled() != enabled {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }
    }
}

/// Reuses host-resolved FFmpeg inputs while creating one pipeline per device
/// session from the latest audio preference.
#[derive(Clone)]
pub struct FfmpegAudioPipelineFactory {
    output: AudioPublisher,
    decoder: AudioDecoderConfig,
    selected_only: bool,
}

impl FfmpegAudioPipelineFactory {
    pub fn new(output: AudioPublisher, decoder: AudioDecoderConfig) -> Self {
        Self {
            output,
            decoder,
            selected_only: true,
        }
    }

    /// Publish every device to a device-aware consumer such as the headless
    /// browser audio registry instead of filtering to the desktop selection.
    pub fn all_sessions(mut self) -> Self {
        self.selected_only = false;
        self
    }
}

impl devicehub_runtime::DeviceAudioPipelineFactory for FfmpegAudioPipelineFactory {
    type Pipeline = FfmpegAudioPipeline;

    fn create(
        &self,
        enabled: bool,
        selection_id: &str,
        active: devicehub_core::ActiveSlot,
        demand: Demand,
    ) -> Self::Pipeline {
        let selected = self.selected_only.then_some(active);
        let activation = match selected.clone() {
            Some(active) => AudioActivation::Selected {
                active,
                selection_id: Arc::from(selection_id),
            },
            None => AudioActivation::Demand(demand),
        };
        FfmpegAudioPipeline::new(
            self.output.for_device(selection_id, selected),
            self.decoder.clone(),
            enabled,
            activation,
        )
    }
}

impl FfmpegAudioPipeline {
    fn new(
        output: AudioPublisher,
        decoder: AudioDecoderConfig,
        enabled: bool,
        activation: AudioActivation,
    ) -> Self {
        Self {
            output,
            decoder,
            enabled,
            activation,
        }
    }
}

impl devicehub_runtime::DeviceAudioPipeline for FfmpegAudioPipeline {
    fn run(&self, source: DeviceAudioSource) -> devicehub_runtime::DeviceAudioFuture {
        Box::pin(run_audio_pipeline(
            source,
            self.output.clone(),
            self.decoder.clone(),
            self.enabled,
            self.activation.clone(),
        ))
    }
}

impl AudioDecoderConfig {
    /// Build a lazy FFmpeg search plan from host-resolved process inputs.
    /// Candidate existence is checked only when device audio is enabled.
    pub fn from_host(
        configured: Option<OsString>,
        search_path: Option<OsString>,
        resource_dir: Option<&std::path::Path>,
        current_exe: Option<&std::path::Path>,
    ) -> Self {
        Self {
            candidates: ffmpeg_candidates(configured, search_path, resource_dir, current_exe)
                .into(),
        }
    }

    fn resolve(&self) -> std::io::Result<PathBuf> {
        let mut rejected = Vec::new();
        for path in self.candidates.iter().filter(|path| path.is_file()) {
            match validate_host_executable(path) {
                Ok(()) => return Ok(path.clone()),
                Err(error) => {
                    tracing::warn!(
                        path = %path.display(),
                        %error,
                        "skipping unusable ffmpeg candidate"
                    );
                    rejected.push(format!("{} ({error})", path.display()));
                }
            }
        }
        let searched = self
            .candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let rejected = if rejected.is_empty() {
            String::new()
        } else {
            format!("; rejected: {}", rejected.join(", "))
        };
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "ffmpeg was not found; install it and add it to PATH, or set \
                 DEVICEHUB_FFMPEG to its absolute path (searched: {searched}{rejected})"
            ),
        ))
    }
}

fn validate_host_executable(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if path.metadata()?.permissions().mode() & 0o111 == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "file has no executable permission bits",
            ));
        }
    }

    let mut header = [0_u8; 20];
    let bytes_read = File::open(path)?.read(&mut header)?;
    if executable_header_matches_host(&header[..bytes_read]) {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "binary format is not executable on {}",
                std::env::consts::OS
            ),
        ))
    }
}

fn executable_header_matches_host(header: &[u8]) -> bool {
    #[cfg(unix)]
    if header.starts_with(b"#!") {
        return true;
    }
    #[cfg(target_os = "macos")]
    return matches!(
        header.get(..4),
        Some([0xfe, 0xed, 0xfa, 0xce])
            | Some([0xce, 0xfa, 0xed, 0xfe])
            | Some([0xfe, 0xed, 0xfa, 0xcf])
            | Some([0xcf, 0xfa, 0xed, 0xfe])
            | Some([0xca, 0xfe, 0xba, 0xbe])
            | Some([0xbe, 0xba, 0xfe, 0xca])
            | Some([0xca, 0xfe, 0xba, 0xbf])
            | Some([0xbf, 0xba, 0xfe, 0xca])
    );
    #[cfg(target_os = "linux")]
    return elf_header_matches_host(header);
    #[cfg(target_os = "windows")]
    return header.starts_with(b"MZ");
    #[allow(unreachable_code)]
    false
}

#[cfg(target_os = "linux")]
fn elf_header_matches_host(header: &[u8]) -> bool {
    if !header.starts_with(b"\x7fELF") || header.len() < 20 {
        return false;
    }
    let machine = match header[5] {
        1 => u16::from_le_bytes([header[18], header[19]]),
        2 => u16::from_be_bytes([header[18], header[19]]),
        _ => return false,
    };
    #[cfg(target_arch = "x86_64")]
    return machine == 62;
    #[cfg(target_arch = "aarch64")]
    return machine == 183;
    #[cfg(target_arch = "x86")]
    return machine == 3;
    #[cfg(target_arch = "arm")]
    return machine == 40;
    #[allow(unreachable_code)]
    true
}

#[derive(Default)]
struct AudioSignalWindow {
    sample_count: u64,
    active_sample_count: u64,
    square_sum: f64,
    peak: i32,
}

impl AudioSignalWindow {
    fn observe(&mut self, pcm: &[u8]) {
        for bytes in pcm.chunks_exact(2) {
            let sample = i32::from(i16::from_le_bytes([bytes[0], bytes[1]]));
            let magnitude = sample.abs();
            self.sample_count += 1;
            self.active_sample_count += u64::from(magnitude > AUDIO_ACTIVE_SAMPLE_THRESHOLD);
            self.square_sum += f64::from(sample * sample);
            self.peak = self.peak.max(magnitude);
        }
    }

    fn levels(&self) -> (f64, f64, f64) {
        let rms = if self.sample_count == 0 {
            0.0
        } else {
            (self.square_sum / self.sample_count as f64).sqrt()
        };
        let dbfs = |amplitude: f64| {
            if amplitude <= 0.0 {
                -96.0
            } else {
                (20.0 * (amplitude / 32_768.0).log10()).max(-96.0)
            }
        };
        let active_ratio = if self.sample_count == 0 {
            0.0
        } else {
            self.active_sample_count as f64 / self.sample_count as f64
        };
        (dbfs(f64::from(self.peak)), dbfs(rms), active_ratio)
    }
}

async fn spawn_audio_ffmpeg(
    config: &AudioDecoderConfig,
) -> std::io::Result<(Child, ChildStdout, ChildStderr, std::net::SocketAddr)> {
    let ffmpeg = config.resolve()?;
    let reservation = tokio::net::UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let rtp_address = reservation.local_addr()?;
    drop(reservation);

    let sdp = format!(
        "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=DeviceHub iPhone Audio\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio {} RTP/AVP 101\r\na=rtpmap:101 MPEG4-GENERIC/48000/2\r\na=fmtp:101 streamtype=5; mode=AAC-hbr; config=F8E65000; SizeLength=13; IndexLength=3; IndexDeltaLength=3; constantDuration=480\r\na=ptime:10\r\na=rtcp-mux\r\n",
        rtp_address.port()
    );
    tracing::info!(path = %ffmpeg.display(), %rtp_address, "using ffmpeg AAC-ELD audio decoder");
    let mut command = Command::new(ffmpeg);
    hide_windows_console(&mut command);
    let mut child = command
        .args(["-protocol_whitelist", "pipe,udp,rtp"])
        .args(["-f", "sdp", "-i", "pipe:0"])
        .args(["-vn", "-acodec", "pcm_s16le"])
        .args(["-ar", "48000", "-ac", "2", "-f", "s16le", "pipe:1"])
        .args(["-loglevel", "error"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child.stdin.take().expect("audio ffmpeg stdin piped");
    stdin.write_all(sdp.as_bytes()).await?;
    stdin.shutdown().await?;
    let stdout = child.stdout.take().expect("audio ffmpeg stdout piped");
    let stderr = child.stderr.take().expect("audio ffmpeg stderr piped");
    Ok((child, stdout, stderr, rtp_address))
}

async fn read_audio_chunks(mut stdout: ChildStdout, output: AudioPublisher) {
    let frames_per_chunk = AUDIO_SAMPLE_RATE as usize * AUDIO_CHUNK_MILLIS / 1_000;
    let mut chunk = vec![0_u8; frames_per_chunk * usize::from(AUDIO_CHANNELS) * 2];
    let mut chunks = 0_u64;
    let mut signal = AudioSignalWindow::default();
    loop {
        match stdout.read_exact(&mut chunk).await {
            Ok(_) => {
                chunks += 1;
                if chunks == 1 {
                    tracing::info!(
                        sample_rate = AUDIO_SAMPLE_RATE,
                        channels = AUDIO_CHANNELS,
                        frames = frames_per_chunk,
                        "ffmpeg audio PCM output started"
                    );
                }
                signal.observe(&chunk);
                if chunks.is_multiple_of(AUDIO_DIAGNOSTIC_CHUNKS) {
                    let (peak_dbfs, rms_dbfs, active_sample_ratio) = signal.levels();
                    tracing::debug!(
                        target: "devicehub_mask::audio",
                        peak_dbfs,
                        rms_dbfs,
                        active_sample_ratio,
                        "decoded PCM signal diagnostics"
                    );
                    signal = AudioSignalWindow::default();
                }
                output.publish(bytes::Bytes::copy_from_slice(&chunk));
            }
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                tracing::info!(chunks, "ffmpeg audio output closed");
                return;
            }
            Err(error) => {
                tracing::warn!(%error, chunks, "ffmpeg audio output read failed");
                return;
            }
        }
    }
}

/// Runs the optional host audio pipeline for one negotiated device session.
/// Disabled audio still drains RTP so the shared DisplayService session remains healthy.
async fn run_audio_pipeline(
    source: DeviceAudioSource,
    output: AudioPublisher,
    decoder: AudioDecoderConfig,
    enabled: bool,
    activation: AudioActivation,
) {
    if !enabled {
        tracing::info!("device audio playback disabled; draining negotiated audio stream");
        source.drain().await;
        return;
    }

    let mut restart_attempt = 0_u32;
    loop {
        if !activation.enabled() {
            tracing::debug!("device audio decoder idle without a media consumer");
            let activated = activation.wait_for(true);
            tokio::pin!(activated);
            loop {
                tokio::select! {
                    _ = &mut activated => break,
                    alive = source.drain_packet() => {
                        if !alive {
                            return;
                        }
                    }
                }
            }
            tracing::debug!("device audio consumer active; starting decoder");
        }
        let (mut child, stdout, stderr, rtp_address) = match spawn_audio_ffmpeg(&decoder).await {
            Ok(process) => process,
            Err(error) => {
                tracing::warn!(%error, "cannot start device audio decoder; draining audio stream");
                source.drain().await;
                return;
            }
        };
        let decoder_started = Instant::now();
        let decoded_output = read_audio_chunks(stdout, output.clone());
        let errors = watch_audio_errors(stderr);
        let receive = source.forward_rtp_to_local_port(rtp_address.port());
        let deactivated = activation.wait_for(false);
        tokio::pin!(decoded_output, errors, receive, deactivated);
        let exit_reason = tokio::select! {
            _ = &mut decoded_output => "output-ended",
            _ = &mut errors => "stderr-ended",
            _ = &mut receive => {
                tracing::warn!("device audio RTP input ended");
                return;
            }
            status = child.wait() => {
                tracing::warn!(?status, "device audio decoder stopped");
                "process-ended"
            },
            _ = &mut deactivated => "demand-ended",
        };
        let elapsed = decoder_started.elapsed();
        if exit_reason == "demand-ended" {
            tracing::debug!(
                elapsed_ms = elapsed.as_millis() as u64,
                "stopping idle audio decoder"
            );
            drop(child);
            restart_attempt = 0;
            continue;
        }
        restart_attempt = if elapsed >= AUDIO_DECODER_STABLE_RUNTIME {
            1
        } else {
            restart_attempt.saturating_add(1)
        };
        let retry_delay = audio_decoder_restart_backoff(restart_attempt - 1);
        tracing::warn!(
            exit_reason,
            elapsed_ms = elapsed.as_millis() as u64,
            restart_attempt,
            retry_ms = retry_delay.as_millis() as u64,
            "device audio decoder ended; restarting"
        );
        drop(child);
        if !source.drain_for(retry_delay).await {
            return;
        }
    }
}

async fn watch_audio_errors(stderr: ChildStderr) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::warn!(target: "devicehub_mask::audio", message = %line, "ffmpeg audio decode error");
    }
}

fn ffmpeg_candidates(
    configured: Option<OsString>,
    path: Option<OsString>,
    resource_dir: Option<&std::path::Path>,
    current_exe: Option<&std::path::Path>,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(configured) = configured.filter(|value| !value.is_empty()) {
        candidates.push(PathBuf::from(configured));
    }
    if let Some(resource_dir) = resource_dir {
        candidates.push(resource_dir.join(ffmpeg_executable()));
    }
    if let Some(parent) = current_exe.and_then(std::path::Path::parent) {
        let adjacent = parent.join(ffmpeg_executable());
        if !candidates.contains(&adjacent) {
            candidates.push(adjacent);
        }
    }
    if let Some(path) = path {
        candidates.extend(
            std::env::split_paths(&path).map(|directory| directory.join(ffmpeg_executable())),
        );
    }
    for path in [
        "/opt/homebrew/bin/ffmpeg",
        "/usr/local/bin/ffmpeg",
        "/opt/local/bin/ffmpeg",
    ] {
        let path = PathBuf::from(path);
        if !candidates.contains(&path) {
            candidates.push(path);
        }
    }
    candidates
}

#[cfg(windows)]
fn hide_windows_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.as_std_mut().creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_windows_console(_command: &mut Command) {}

fn ffmpeg_executable() -> &'static str {
    if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn measures_silent_and_audible_pcm_windows() {
        let mut silent = AudioSignalWindow::default();
        silent.observe(&[0; 8]);
        assert_eq!(silent.levels(), (-96.0, -96.0, 0.0));

        let samples = [0_i16, 16_384, -16_384, i16::MIN];
        let pcm = samples
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect::<Vec<_>>();
        let mut audible = AudioSignalWindow::default();
        audible.observe(&pcm);
        let (peak_dbfs, rms_dbfs, active_sample_ratio) = audible.levels();
        assert_eq!(peak_dbfs, 0.0);
        assert!((-4.3..-4.2).contains(&rms_dbfs));
        assert_eq!(active_sample_ratio, 0.75);
    }

    #[test]
    fn configured_ffmpeg_precedes_path_and_common_locations() {
        let search_path = std::env::join_paths([PathBuf::from("first"), PathBuf::from("second")])
            .expect("build test PATH");
        let candidates = ffmpeg_candidates(
            Some(OsString::from("/custom/ffmpeg")),
            Some(search_path),
            Some(std::path::Path::new("/bundle/resources")),
            Some(std::path::Path::new("/bundle/devicehub-mask")),
        );

        assert_eq!(candidates[0], PathBuf::from("/custom/ffmpeg"));
        assert_eq!(
            candidates[1],
            PathBuf::from("/bundle/resources").join(ffmpeg_executable())
        );
        assert_eq!(
            candidates[2],
            PathBuf::from("/bundle").join(ffmpeg_executable())
        );
        assert_eq!(
            candidates[3],
            PathBuf::from("first").join(ffmpeg_executable())
        );
        assert_eq!(
            candidates[4],
            PathBuf::from("second").join(ffmpeg_executable())
        );
        assert!(candidates.contains(&PathBuf::from("/opt/homebrew/bin/ffmpeg")));
        assert!(candidates.contains(&PathBuf::from("/usr/local/bin/ffmpeg")));
    }

    #[test]
    fn executable_header_accepts_the_host_format_and_scripts() {
        #[cfg(unix)]
        assert!(executable_header_matches_host(b"#!/bin/sh\n"));
        #[cfg(target_os = "macos")]
        assert!(executable_header_matches_host(&[0xcf, 0xfa, 0xed, 0xfe]));
        #[cfg(target_os = "linux")]
        {
            let mut elf = [0_u8; 20];
            elf[..4].copy_from_slice(b"\x7fELF");
            elf[5] = 1;
            #[cfg(target_arch = "x86_64")]
            elf[18..20].copy_from_slice(&62_u16.to_le_bytes());
            #[cfg(target_arch = "aarch64")]
            elf[18..20].copy_from_slice(&183_u16.to_le_bytes());
            assert!(executable_header_matches_host(&elf));
        }
        #[cfg(target_os = "windows")]
        assert!(executable_header_matches_host(b"MZ"));
    }

    #[test]
    fn executable_header_rejects_a_foreign_binary() {
        #[cfg(not(target_os = "linux"))]
        assert!(!executable_header_matches_host(b"\x7fELF"));
        #[cfg(not(target_os = "windows"))]
        assert!(!executable_header_matches_host(b"MZ"));
        #[cfg(not(target_os = "macos"))]
        assert!(!executable_header_matches_host(&[0xcf, 0xfa, 0xed, 0xfe]));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn executable_header_rejects_an_elf_for_another_architecture() {
        let mut elf = [0_u8; 20];
        elf[..4].copy_from_slice(b"\x7fELF");
        elf[5] = 1;
        let foreign_machine: u16 = if cfg!(target_arch = "aarch64") {
            62
        } else {
            183
        };
        elf[18..20].copy_from_slice(&foreign_machine.to_le_bytes());
        assert!(!executable_header_matches_host(&elf));
    }

    #[cfg(unix)]
    #[test]
    fn resolver_skips_a_foreign_candidate_and_uses_the_next_executable() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let directory =
            std::env::temp_dir().join(format!("devicehub-ffmpeg-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory).expect("create test directory");
        let foreign = directory.join("foreign-ffmpeg");
        let fallback = directory.join("fallback-ffmpeg");
        fs::write(&foreign, b"MZforeign").expect("write foreign executable");
        fs::write(&fallback, b"#!/bin/sh\nexit 0\n").expect("write fallback executable");
        fs::set_permissions(&foreign, fs::Permissions::from_mode(0o755))
            .expect("chmod foreign executable");
        fs::set_permissions(&fallback, fs::Permissions::from_mode(0o755))
            .expect("chmod fallback executable");

        let config = AudioDecoderConfig {
            candidates: Arc::from([foreign, fallback.clone()]),
        };
        assert_eq!(config.resolve().expect("resolve fallback"), fallback);
        fs::remove_dir_all(directory).expect("remove test directory");
    }
}
