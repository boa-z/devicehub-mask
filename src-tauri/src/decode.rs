// HEVC decode via an `ffmpeg` subprocess: Annex-B in on stdin, PAM (P7) frames
// out on stdout. PAM is self-describing (size in every header, so resolution
// changes need no extra signalling). The stream has no useful alpha channel, so
// RGB24 saves 25% of pipe and memory bandwidth compared with RGBA.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::Notify;

use crate::protocol::{
    AUDIO_CHANNELS, AUDIO_SAMPLE_RATE, AudioSlot, Frame, FrameFormat, FrameSlot, VideoCounters,
};

const AUDIO_CHUNK_MILLIS: usize = 20;

pub async fn spawn_audio_ffmpeg()
-> std::io::Result<(Child, ChildStdout, ChildStderr, std::net::SocketAddr)> {
    let ffmpeg = resolve_ffmpeg()?;
    let reservation = tokio::net::UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let rtp_address = reservation.local_addr()?;
    drop(reservation);

    let sdp = format!(
        "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=DeviceHub iPhone Audio\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio {} RTP/AVP 101\r\na=rtpmap:101 MPEG4-GENERIC/48000/2\r\na=fmtp:101 streamtype=5; mode=AAC-hbr; config=F8E65000; SizeLength=13; IndexLength=3; IndexDeltaLength=3; constantDuration=480\r\na=ptime:10\r\na=rtcp-mux\r\n",
        rtp_address.port()
    );
    tracing::info!(path = %ffmpeg.display(), %rtp_address, "using ffmpeg AAC-ELD audio decoder");
    let mut child = Command::new(ffmpeg)
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

pub async fn read_audio_chunks(mut stdout: ChildStdout, slot: AudioSlot) {
    let frames_per_chunk = AUDIO_SAMPLE_RATE as usize * AUDIO_CHUNK_MILLIS / 1_000;
    let mut chunk = vec![0_u8; frames_per_chunk * usize::from(AUDIO_CHANNELS) * 2];
    let mut chunks = 0_u64;
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
                slot.publish(bytes::Bytes::copy_from_slice(&chunk));
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

/// Spawn `ffmpeg` decoding raw HEVC (Annex-B on stdin) to PAM frames on stdout.
/// stderr is piped so the session can watch it for decode errors.
pub fn spawn_ffmpeg(
    frame_format: FrameFormat,
) -> std::io::Result<(Child, ChildStdin, ChildStdout, ChildStderr)> {
    let ffmpeg = resolve_ffmpeg()?;
    let max_dimension = configured_max_dimension(
        std::env::var_os("DEVICEHUB_VIDEO_MAX_DIMENSION"),
        cfg!(windows),
    );
    tracing::info!(path = %ffmpeg.display(), ?max_dimension, ?frame_format, "using ffmpeg");
    let mut command = Command::new(ffmpeg);
    command
        // Do *not* add `-fflags nobuffer`: it makes ffmpeg skip the opening IDR +
        // parameter sets, so every P-frame fails with "Could not find ref".
        .args(["-flags", "low_delay"])
        .args(["-hwaccel", "auto"])
        .args(["-f", "hevc", "-i", "pipe:0"]);
    let scale = max_dimension.map(|max_dimension| {
        format!(
            "scale=w='min(iw,{max_dimension})':h='min(ih,{max_dimension})':force_original_aspect_ratio=decrease:force_divisible_by=2"
        )
    });
    match frame_format {
        FrameFormat::Rgb24 => {
            if let Some(scale) = &scale {
                command.args(["-vf", scale]);
            }
            command.args([
                "-an",
                "-f",
                "image2pipe",
                "-vcodec",
                "pam",
                "-pix_fmt",
                "rgb24",
                "pipe:1",
            ]);
        }
        FrameFormat::Yuv420p => {
            let full_range_filter = match scale {
                Some(scale) => format!("{scale}:out_range=full"),
                None => "scale=in_range=auto:out_range=full".to_owned(),
            };
            command.args([
                "-vf",
                &full_range_filter,
                "-an",
                "-f",
                "yuv4mpegpipe",
                "-pix_fmt",
                "yuv420p",
                "pipe:1",
            ]);
        }
    }
    let mut child = command
        .args(["-loglevel", "error"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

    let stdin = child.stdin.take().expect("ffmpeg stdin piped");
    let stdout = child.stdout.take().expect("ffmpeg stdout piped");
    let stderr = child.stderr.take().expect("ffmpeg stderr piped");
    Ok((child, stdin, stdout, stderr))
}

fn configured_max_dimension(configured: Option<OsString>, windows: bool) -> Option<u32> {
    let fallback = windows.then_some(1920);
    let Some(configured) = configured else {
        return fallback;
    };
    let Some(value) = configured
        .to_str()
        .and_then(|value| value.parse::<u32>().ok())
    else {
        tracing::warn!(
            ?configured,
            "ignoring invalid DEVICEHUB_VIDEO_MAX_DIMENSION"
        );
        return fallback;
    };
    match value {
        0 => None,
        value if value >= 320 => Some(value),
        value => {
            tracing::warn!(value, "ignoring invalid DEVICEHUB_VIDEO_MAX_DIMENSION");
            fallback
        }
    }
}

fn resolve_ffmpeg() -> std::io::Result<PathBuf> {
    let candidates = ffmpeg_candidates(
        std::env::var_os("DEVICEHUB_FFMPEG"),
        std::env::var_os("PATH"),
    );
    if let Some(path) = candidates.iter().find(|path| path.is_file()) {
        return Ok(path.clone());
    }

    let searched = candidates
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!(
            "ffmpeg was not found; install it and add it to PATH, or set \
             DEVICEHUB_FFMPEG to its absolute path (searched: {searched})"
        ),
    ))
}

fn ffmpeg_candidates(configured: Option<OsString>, path: Option<OsString>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(configured) = configured.filter(|value| !value.is_empty()) {
        candidates.push(PathBuf::from(configured));
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

fn ffmpeg_executable() -> &'static str {
    if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    }
}

/// Read PAM frames from ffmpeg's stdout, publishing each (already RGBA) into
/// `slot` and waking the UI via `repaint`. Each frame pulses `beat`, the liveness
/// heartbeat watched by the session's stall watchdog. Returns when the stream
/// ends.
pub async fn read_frames(
    stdout: ChildStdout,
    frame_format: FrameFormat,
    slot: FrameSlot,
    counters: VideoCounters,
    beat: Arc<Notify>,
    repaint: impl Fn(),
) {
    let mut reader = BufReader::new(stdout);
    let mut last_dims: Option<(usize, usize)> = None;
    let mut read_buf: Vec<u8> = Vec::new();
    let mut last: Option<Arc<Frame>> = None;
    let mut pool: Vec<Vec<u8>> = Vec::new();
    let mut y4m_dimensions = None;
    loop {
        let decoded = match frame_format {
            FrameFormat::Rgb24 => read_pam(&mut reader, &mut read_buf).await,
            FrameFormat::Yuv420p => read_y4m(&mut reader, &mut read_buf, &mut y4m_dimensions).await,
        };
        match decoded {
            Ok(Some((width, height))) => {
                counters.note_decoded_frame();
                let dims = (width, height);
                if last_dims != Some(dims) {
                    tracing::info!(?frame_format, "decoded frame size: {}x{}", dims.0, dims.1);
                    last_dims = Some(dims);
                }

                // Pulse even for duplicate frames: a frozen-but-streaming
                // screen is still a healthy stream.
                beat.notify_one();

                if last
                    .as_ref()
                    .is_some_and(|previous| previous.pixels == read_buf)
                {
                    counters.note_duplicate_frame();
                    continue;
                }

                let frame = Arc::new(Frame {
                    width,
                    height,
                    format: frame_format,
                    pixels: std::mem::take(&mut read_buf),
                    decoded_at: std::time::Instant::now(),
                    jpeg: OnceLock::new(),
                });
                read_buf = pool.pop().unwrap_or_default();
                last = Some(frame.clone());
                if let Some(prev) = slot.publish(frame)
                    && let Ok(frame) = Arc::try_unwrap(prev)
                    && pool.len() < 2
                {
                    pool.push(frame.pixels);
                }
                repaint();
            }
            Ok(None) => {
                tracing::info!("ffmpeg stdout closed");
                break;
            }
            Err(e) => {
                tracing::warn!("pam read error: {e}");
                break;
            }
        }
    }
}

/// Read a single binary PAM (P7) image into `rgb` as a raw top-down RGB24 raster,
/// reusing its allocation. Returns the dimensions, or `Ok(None)` at clean EOF.
///
/// PAM headers are line-oriented: `P7`, then `KEY VALUE` lines (`WIDTH`,
/// `HEIGHT`, `DEPTH`, `MAXVAL`, `TUPLTYPE`) in any order, terminated by `ENDHDR`,
/// then the raster. We require the 3-channel/8-bit layout ffmpeg emits for `rgb24`.
async fn read_pam<R: AsyncReadExt + AsyncBufReadExt + Unpin>(
    r: &mut R,
    rgb: &mut Vec<u8>,
) -> std::io::Result<Option<(usize, usize)>> {
    let invalid = |msg: String| std::io::Error::new(std::io::ErrorKind::InvalidData, msg);

    let mut line = Vec::new();
    // First line: the `P7` magic. Zero bytes here is a clean end-of-stream.
    if r.read_until(b'\n', &mut line).await? == 0 {
        return Ok(None);
    }
    if trim(&line) != b"P7" {
        return Err(invalid(format!(
            "expected PAM 'P7' magic, got {:?}",
            trim(&line)
        )));
    }

    let (mut width, mut height, mut depth, mut maxval) = (0usize, 0usize, 0usize, 0usize);
    loop {
        line.clear();
        if r.read_until(b'\n', &mut line).await? == 0 {
            return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
        }
        let l = trim(&line);
        if l == b"ENDHDR" {
            break;
        }
        if l.is_empty() || l[0] == b'#' {
            continue;
        }
        // Parse `KEY VALUE`; only the numeric fields matter (TUPLTYPE is ignored).
        let text = String::from_utf8_lossy(l);
        let mut it = text.split_ascii_whitespace();
        let key = it.next().unwrap_or("");
        let slot = match key {
            "WIDTH" => &mut width,
            "HEIGHT" => &mut height,
            "DEPTH" => &mut depth,
            "MAXVAL" => &mut maxval,
            _ => continue,
        };
        *slot = it
            .next()
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| invalid(format!("bad PAM header line: {text:?}")))?;
    }

    if depth != 3 || maxval != 255 {
        return Err(invalid(format!(
            "expected 8-bit 3-channel PAM, got depth={depth} maxval={maxval}"
        )));
    }
    if width == 0 || height == 0 {
        return Err(invalid(format!("bad PAM dimensions {width}x{height}")));
    }

    rgb.resize(width * height * depth, 0);
    r.read_exact(rgb).await?;
    Ok(Some((width, height)))
}

async fn read_y4m<R: AsyncReadExt + AsyncBufReadExt + Unpin>(
    reader: &mut R,
    pixels: &mut Vec<u8>,
    dimensions: &mut Option<(usize, usize)>,
) -> std::io::Result<Option<(usize, usize)>> {
    if dimensions.is_none() {
        let mut header = Vec::new();
        if reader.read_until(b'\n', &mut header).await? == 0 {
            return Ok(None);
        }
        *dimensions = Some(parse_y4m_header(trim(&header))?);
    }
    let (width, height) = dimensions.expect("Y4M dimensions initialized");

    let mut frame_header = Vec::new();
    if reader.read_until(b'\n', &mut frame_header).await? == 0 {
        return Ok(None);
    }
    let frame_header = trim(&frame_header);
    if frame_header != b"FRAME" && !frame_header.starts_with(b"FRAME ") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("expected Y4M FRAME header, got {frame_header:?}"),
        ));
    }

    let y = width
        .checked_mul(height)
        .ok_or_else(|| std::io::Error::other("Y4M dimensions overflow"))?;
    let uv = (width / 2)
        .checked_mul(height / 2)
        .ok_or_else(|| std::io::Error::other("Y4M dimensions overflow"))?;
    let frame_len = y
        .checked_add(uv.saturating_mul(2))
        .ok_or_else(|| std::io::Error::other("Y4M frame size overflow"))?;
    pixels.resize(frame_len, 0);
    reader.read_exact(pixels).await?;
    Ok(Some((width, height)))
}

fn parse_y4m_header(header: &[u8]) -> std::io::Result<(usize, usize)> {
    let invalid = |message: String| std::io::Error::new(std::io::ErrorKind::InvalidData, message);
    let header = std::str::from_utf8(header)
        .map_err(|error| invalid(format!("invalid Y4M header encoding: {error}")))?;
    let mut fields = header.split_ascii_whitespace();
    if fields.next() != Some("YUV4MPEG2") {
        return Err(invalid(format!(
            "expected YUV4MPEG2 header, got {header:?}"
        )));
    }
    let mut width = None;
    let mut height = None;
    for field in fields {
        if let Some(value) = field.strip_prefix('W') {
            width = value.parse::<usize>().ok();
        } else if let Some(value) = field.strip_prefix('H') {
            height = value.parse::<usize>().ok();
        } else if let Some(value) = field.strip_prefix('C')
            && !matches!(value, "420" | "420jpeg" | "420mpeg2" | "420paldv")
        {
            return Err(invalid(format!("unsupported Y4M colorspace C{value}")));
        }
    }
    let (width, height) = match (width, height) {
        (Some(width), Some(height)) if width > 0 && height > 0 => (width, height),
        _ => return Err(invalid(format!("missing Y4M dimensions in {header:?}"))),
    };
    if width % 2 != 0 || height % 2 != 0 {
        return Err(invalid(format!(
            "YUV420P requires even dimensions, got {width}x{height}"
        )));
    }
    Ok((width, height))
}

/// Strip a trailing `\n`/`\r\n` and surrounding ASCII whitespace from a header line.
fn trim(line: &[u8]) -> &[u8] {
    let mut s = line;
    while let [rest @ .., last] = s
        && last.is_ascii_whitespace()
    {
        s = rest;
    }
    while let [first, rest @ ..] = s
        && first.is_ascii_whitespace()
    {
        s = rest;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_ffmpeg_precedes_path_and_common_locations() {
        let search_path = std::env::join_paths([PathBuf::from("first"), PathBuf::from("second")])
            .expect("build test PATH");
        let candidates =
            ffmpeg_candidates(Some(OsString::from("/custom/ffmpeg")), Some(search_path));

        assert_eq!(candidates[0], PathBuf::from("/custom/ffmpeg"));
        assert_eq!(
            candidates[1],
            PathBuf::from("first").join(ffmpeg_executable())
        );
        assert_eq!(
            candidates[2],
            PathBuf::from("second").join(ffmpeg_executable())
        );
        assert!(candidates.contains(&PathBuf::from("/opt/homebrew/bin/ffmpeg")));
        assert!(candidates.contains(&PathBuf::from("/usr/local/bin/ffmpeg")));
    }

    #[test]
    fn windows_limits_transport_resolution_unless_overridden() {
        assert_eq!(configured_max_dimension(None, true), Some(1920));
        assert_eq!(configured_max_dimension(None, false), None);
        assert_eq!(
            configured_max_dimension(Some(OsString::from("1440")), true),
            Some(1440)
        );
        assert_eq!(
            configured_max_dimension(Some(OsString::from("0")), true),
            None
        );
        assert_eq!(
            configured_max_dimension(Some(OsString::from("12")), true),
            Some(1920)
        );
        assert_eq!(
            configured_max_dimension(Some(OsString::from("invalid")), true),
            Some(1920)
        );
    }

    #[tokio::test]
    async fn reads_rgb24_pam_frame() {
        let input =
            b"P7\nWIDTH 2\nHEIGHT 1\nDEPTH 3\nMAXVAL 255\nTUPLTYPE RGB\nENDHDR\n\xff\0\0\0\xff\0";
        let mut reader = tokio::io::BufReader::new(&input[..]);
        let mut rgb = Vec::new();

        assert_eq!(read_pam(&mut reader, &mut rgb).await.unwrap(), Some((2, 1)));
        assert_eq!(rgb, [255, 0, 0, 0, 255, 0]);
        assert_eq!(read_pam(&mut reader, &mut rgb).await.unwrap(), None);
    }

    #[tokio::test]
    async fn reads_yuv420p_y4m_frames() {
        let input = b"YUV4MPEG2 W2 H2 F60:1 Ip A1:1 C420jpeg\nFRAME\n\x10\x20\x30\x40\x80\x90FRAME XPTS=1\n\x11\x21\x31\x41\x81\x91";
        let mut reader = tokio::io::BufReader::new(&input[..]);
        let mut pixels = Vec::new();
        let mut dimensions = None;

        assert_eq!(
            read_y4m(&mut reader, &mut pixels, &mut dimensions)
                .await
                .unwrap(),
            Some((2, 2))
        );
        assert_eq!(pixels, [0x10, 0x20, 0x30, 0x40, 0x80, 0x90]);
        assert_eq!(
            read_y4m(&mut reader, &mut pixels, &mut dimensions)
                .await
                .unwrap(),
            Some((2, 2))
        );
        assert_eq!(pixels, [0x11, 0x21, 0x31, 0x41, 0x81, 0x91]);
        assert_eq!(
            read_y4m(&mut reader, &mut pixels, &mut dimensions)
                .await
                .unwrap(),
            None
        );
    }
}
