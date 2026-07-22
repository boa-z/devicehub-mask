// HEVC decode via an `ffmpeg` subprocess. The device RTP stream is forwarded to
// a loopback UDP port so ffmpeg receives its real 90 kHz media timestamps. PAM
// (P7) fallback frames are self-describing, so resolution changes need no extra
// signalling. The stream has no useful alpha channel, so RGB24 saves 25% of
// pipe and memory bandwidth compared with RGBA.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

use bytes::Bytes;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::Notify;

use crate::protocol::{EncodedVideoStream, Frame, FrameSlot, VideoPipeline};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Spawn `ffmpeg` decoding raw HEVC (Annex-B on stdin) to PAM frames on stdout.
/// stderr is piped so the session can watch it for decode errors.
pub fn spawn_ffmpeg_jpeg() -> std::io::Result<(Child, ChildStdin, ChildStdout, ChildStderr)> {
    let ffmpeg = resolve_ffmpeg()?;
    let max_dimension = configured_jpeg_max_dimension(
        std::env::var_os("DEVICEHUB_VIDEO_MAX_DIMENSION"),
        cfg!(windows),
    );
    tracing::info!(path = %ffmpeg.display(), ?max_dimension, "using ffmpeg JPEG fallback");
    let mut command = Command::new(ffmpeg);
    command
        // Do *not* add `-fflags nobuffer`: it makes ffmpeg skip the opening IDR +
        // parameter sets, so every P-frame fails with "Could not find ref".
        .args(["-flags", "low_delay"])
        .args(rtp_input_args());
    if let Some(max_dimension) = max_dimension {
        command.args([
            "-vf",
            &format!(
                "scale=w='min(iw,{max_dimension})':h='min(ih,{max_dimension})':force_original_aspect_ratio=decrease:force_divisible_by=2"
            ),
        ]);
    }
    hide_console(&mut command);
    let mut child = command
        .args([
            "-an",
            "-f",
            "image2pipe",
            "-vcodec",
            "pam",
            "-pix_fmt",
            "rgb24",
            "-fps_mode",
            "passthrough",
        ])
        .arg("pipe:1")
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

fn configured_jpeg_max_dimension(configured: Option<OsString>, windows: bool) -> Option<u32> {
    let fallback = windows.then_some(1280);
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

pub async fn select_video_pipeline() -> VideoPipeline {
    let requested = std::env::var("DEVICEHUB_VIDEO_PIPELINE")
        .unwrap_or_else(|_| "auto".into())
        .to_ascii_lowercase();
    if requested == "jpeg" {
        return VideoPipeline::Jpeg;
    }

    let automatic: &[VideoPipeline] = if cfg!(windows) {
        &[
            VideoPipeline::H264Qsv,
            VideoPipeline::H264Nvenc,
            VideoPipeline::H264Amf,
        ]
    } else if cfg!(target_os = "macos") {
        &[VideoPipeline::H264VideoToolbox]
    } else {
        &[]
    };
    let candidates: &[VideoPipeline] = match requested.as_str() {
        "qsv" | "h264-qsv" => &[VideoPipeline::H264Qsv],
        "nvenc" | "h264-nvenc" => &[VideoPipeline::H264Nvenc],
        "amf" | "h264-amf" => &[VideoPipeline::H264Amf],
        "videotoolbox" | "h264-videotoolbox" => &[VideoPipeline::H264VideoToolbox],
        "auto" => automatic,
        other => {
            tracing::warn!(other, "unknown DEVICEHUB_VIDEO_PIPELINE; using auto");
            automatic
        }
    };
    for pipeline in candidates {
        if probe_pipeline(*pipeline).await {
            return *pipeline;
        }
    }
    tracing::warn!("no supported Windows hardware H.264 encoder; using TurboJPEG fallback");
    VideoPipeline::Jpeg
}

async fn probe_pipeline(pipeline: VideoPipeline) -> bool {
    let Ok(ffmpeg) = resolve_ffmpeg() else {
        return false;
    };
    let mut command = Command::new(ffmpeg);
    command.args(probe_args(pipeline));
    hide_console(&mut command);
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::null());
    command.stderr(std::process::Stdio::null());
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), command.status()).await;
    let available = matches!(result, Ok(Ok(status)) if status.success());
    tracing::info!(
        pipeline = pipeline.label(),
        available,
        "probed hardware video backend"
    );
    available
}

fn probe_args(pipeline: VideoPipeline) -> Vec<String> {
    let encoder = match pipeline {
        VideoPipeline::H264Qsv => "h264_qsv",
        VideoPipeline::H264Nvenc => "h264_nvenc",
        VideoPipeline::H264Amf => "h264_amf",
        VideoPipeline::H264VideoToolbox => "h264_videotoolbox",
        VideoPipeline::Jpeg => return Vec::new(),
    };
    [
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=size=128x128:rate=1",
        "-frames:v",
        "1",
        "-c:v",
        encoder,
        "-f",
        "null",
        "NUL",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

pub fn spawn_ffmpeg_h264(
    pipeline: VideoPipeline,
) -> std::io::Result<(Child, ChildStdin, ChildStdout, ChildStderr)> {
    debug_assert!(pipeline.is_h264());
    let ffmpeg = resolve_ffmpeg()?;
    let max_dimension =
        configured_h264_max_dimension(std::env::var_os("DEVICEHUB_VIDEO_MAX_DIMENSION"));
    let args = h264_args(pipeline, max_dimension);
    tracing::info!(path = %ffmpeg.display(), pipeline = pipeline.label(), ?max_dimension, "using hardware video pipeline");
    let mut command = Command::new(ffmpeg);
    command.args(args);
    hide_console(&mut command);
    let mut child = command
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

fn configured_h264_max_dimension(configured: Option<OsString>) -> Option<u32> {
    let configured = configured?;
    configured
        .to_str()
        .and_then(|value| value.parse::<u32>().ok())
        .and_then(|value| (value >= 320).then_some(value))
}

fn scale_filter(name: &str, max_dimension: u32) -> String {
    format!(
        "{name}=w='if(gte(iw,ih),min(iw,{max_dimension}),-1)':h='if(gte(iw,ih),-1,min(ih,{max_dimension}))':format=nv12"
    )
}

fn rtp_input_args() -> Vec<String> {
    [
        "-protocol_whitelist".to_string(),
        "pipe,udp,rtp".to_string(),
        "-max_delay".to_string(),
        "100000".to_string(),
        "-reorder_queue_size".to_string(),
        "64".to_string(),
        "-rtbufsize".to_string(),
        "8M".to_string(),
        "-f".to_string(),
        "sdp".to_string(),
        "-i".to_string(),
        "pipe:0".to_string(),
    ]
    .into_iter()
    .collect()
}

pub fn rtp_sdp(rtp_port: u16, payload_type: u8) -> String {
    format!(
        "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=DeviceHub\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=video {rtp_port} RTP/AVP {payload_type}\r\na=rtpmap:{payload_type} H265/90000\r\na=framerate:60\r\na=recvonly\r\n"
    )
}

fn h264_args(pipeline: VideoPipeline, max_dimension: Option<u32>) -> Vec<String> {
    let mut args: Vec<String> = vec!["-hide_banner", "-flags", "low_delay"]
        .into_iter()
        .map(str::to_string)
        .collect();
    match pipeline {
        VideoPipeline::H264Qsv => args.extend(
            [
                "-hwaccel",
                "qsv",
                "-hwaccel_output_format",
                "qsv",
                "-c:v",
                "hevc_qsv",
            ]
            .into_iter()
            .map(str::to_string),
        ),
        VideoPipeline::H264Nvenc => args.extend(
            [
                "-hwaccel",
                "cuda",
                "-hwaccel_output_format",
                "cuda",
                "-c:v",
                "hevc_cuvid",
            ]
            .into_iter()
            .map(str::to_string),
        ),
        VideoPipeline::H264Amf => args.extend(
            ["-hwaccel", "d3d11va", "-hwaccel_output_format", "d3d11"]
                .into_iter()
                .map(str::to_string),
        ),
        VideoPipeline::H264VideoToolbox => args.extend(
            [
                "-hwaccel",
                "videotoolbox",
                "-hwaccel_output_format",
                "videotoolbox",
                "-c:v",
                "hevc_videotoolbox",
            ]
            .into_iter()
            .map(str::to_string),
        ),
        VideoPipeline::Jpeg => return Vec::new(),
    }
    args.extend(rtp_input_args());
    if let Some(max_dimension) = max_dimension {
        let filter = match pipeline {
            VideoPipeline::H264Qsv => scale_filter("scale_qsv", max_dimension),
            VideoPipeline::H264Nvenc => scale_filter("scale_cuda", max_dimension),
            VideoPipeline::H264Amf => scale_filter("scale_d3d11", max_dimension),
            VideoPipeline::H264VideoToolbox => scale_filter("scale_vt", max_dimension),
            VideoPipeline::Jpeg => unreachable!(),
        };
        args.extend(["-vf".into(), filter]);
    }
    let encoder_args: &[&str] = match pipeline {
        VideoPipeline::H264Qsv => &[
            "-c:v",
            "h264_qsv",
            "-preset",
            "veryfast",
            "-async_depth",
            "1",
            "-look_ahead",
            "0",
            "-global_quality",
            "24",
            "-aud",
            "1",
            "-repeat_pps",
            "1",
        ],
        VideoPipeline::H264Nvenc => &[
            "-c:v",
            "h264_nvenc",
            "-preset",
            "p1",
            "-tune",
            "ll",
            "-rc",
            "constqp",
            "-qp",
            "24",
            "-aud",
            "1",
            "-repeat_headers",
            "1",
        ],
        VideoPipeline::H264Amf => &[
            "-c:v",
            "h264_amf",
            "-usage",
            "ultralowlatency",
            "-quality",
            "speed",
            "-rc",
            "cqp",
            "-qp_i",
            "22",
            "-qp_p",
            "24",
            "-async_depth",
            "1",
            "-aud",
            "1",
            "-header_spacing",
            "1",
        ],
        VideoPipeline::H264VideoToolbox => &[
            "-c:v",
            "h264_videotoolbox",
            "-realtime",
            "1",
            "-prio_speed",
            "1",
            "-q:v",
            "65",
        ],
        VideoPipeline::Jpeg => unreachable!(),
    };
    args.extend(encoder_args.iter().map(|value| (*value).to_string()));
    args.extend(
        [
            "-an",
            "-bf",
            "0",
            "-g",
            "30",
            "-profile:v",
            "high",
            "-level:v",
            "5.1",
            "-bsf:v",
            "dump_extra=freq=keyframe",
            "-fps_mode",
            "passthrough",
            "-mpegts_flags",
            "+resend_headers",
            "-muxdelay",
            "0",
            "-muxpreload",
            "0",
            "-f",
            "mpegts",
            "pipe:1",
            "-loglevel",
            "warning",
        ]
        .into_iter()
        .map(str::to_string),
    );
    args
}

fn hide_console(command: &mut Command) {
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    #[cfg(not(windows))]
    let _ = command;
}

pub async fn read_mpegts(stdout: ChildStdout, stream: EncodedVideoStream) {
    let mut reader = BufReader::new(stdout);
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(length) => stream.publish(Bytes::copy_from_slice(&buffer[..length])),
            Err(error) => {
                tracing::warn!("MPEG-TS read error: {error}");
                break;
            }
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
    slot: FrameSlot,
    beat: Arc<Notify>,
    repaint: impl Fn(),
) {
    let mut reader = BufReader::new(stdout);
    let mut last_dims: Option<(usize, usize)> = None;
    let mut read_buf: Vec<u8> = Vec::new();
    let mut last: Option<Arc<Frame>> = None;
    let mut pool: Vec<Vec<u8>> = Vec::new();
    loop {
        match read_pam(&mut reader, &mut read_buf).await {
            Ok(Some((width, height))) => {
                let dims = (width, height);
                if last_dims != Some(dims) {
                    tracing::info!("decoded frame size: {}x{}", dims.0, dims.1);
                    last_dims = Some(dims);
                }

                // Pulse even for duplicate frames: a frozen-but-streaming
                // screen is still a healthy stream.
                beat.notify_one();

                if last
                    .as_ref()
                    .is_some_and(|previous| previous.rgb == read_buf)
                {
                    continue;
                }

                let frame = Arc::new(Frame {
                    width,
                    height,
                    rgb: std::mem::take(&mut read_buf),
                    jpeg: OnceLock::new(),
                });
                read_buf = pool.pop().unwrap_or_default();
                last = Some(frame.clone());
                if let Some(prev) = slot.publish(frame)
                    && let Ok(frame) = Arc::try_unwrap(prev)
                    && pool.len() < 2
                {
                    pool.push(frame.rgb);
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
    fn jpeg_fallback_uses_a_conservative_windows_limit() {
        assert_eq!(configured_jpeg_max_dimension(None, true), Some(1280));
        assert_eq!(configured_jpeg_max_dimension(None, false), None);
        assert_eq!(
            configured_jpeg_max_dimension(Some(OsString::from("1440")), true),
            Some(1440)
        );
        assert_eq!(
            configured_jpeg_max_dimension(Some(OsString::from("0")), true),
            None
        );
        assert_eq!(
            configured_jpeg_max_dimension(Some(OsString::from("12")), true),
            Some(1280)
        );
        assert_eq!(
            configured_jpeg_max_dimension(Some(OsString::from("invalid")), true),
            Some(1280)
        );
    }

    #[test]
    fn hardware_pipeline_defaults_to_native_resolution() {
        assert_eq!(configured_h264_max_dimension(None), None);
        assert_eq!(
            configured_h264_max_dimension(Some(OsString::from("1920"))),
            Some(1920)
        );
        assert_eq!(
            configured_h264_max_dimension(Some(OsString::from("invalid"))),
            None
        );
    }

    #[test]
    fn qsv_pipeline_keeps_frames_on_the_gpu() {
        let native = h264_args(VideoPipeline::H264Qsv, None);
        assert!(native.windows(2).any(|args| args == ["-hwaccel", "qsv"]));
        assert!(
            native
                .windows(2)
                .any(|args| args == ["-hwaccel_output_format", "qsv"])
        );
        assert!(native.windows(2).any(|args| args == ["-c:v", "hevc_qsv"]));
        assert!(native.windows(2).any(|args| args == ["-c:v", "h264_qsv"]));
        assert!(native.windows(2).any(|args| args == ["-f", "sdp"]));
        assert!(
            !native
                .iter()
                .any(|arg| arg == "-use_wallclock_as_timestamps")
        );
        assert!(
            native
                .windows(2)
                .any(|args| args == ["-fps_mode", "passthrough"])
        );
        assert!(!native.iter().any(|arg| arg == "-vf"));

        let scaled = h264_args(VideoPipeline::H264Qsv, Some(1920));
        assert!(scaled.iter().any(|arg| arg.starts_with("scale_qsv=")));
    }

    #[test]
    fn rtp_session_preserves_the_device_media_clock() {
        let sdp = rtp_sdp(49152, 112);
        assert!(sdp.contains("m=video 49152 RTP/AVP 112\r\n"));
        assert!(sdp.contains("a=rtpmap:112 H265/90000\r\n"));
        assert!(sdp.contains("a=framerate:60\r\n"));
    }

    #[test]
    fn macos_pipeline_uses_videotoolbox_end_to_end() {
        let args = h264_args(VideoPipeline::H264VideoToolbox, None);
        assert!(
            args.windows(2)
                .any(|args| args == ["-hwaccel", "videotoolbox"])
        );
        assert!(
            args.windows(2)
                .any(|args| args == ["-c:v", "hevc_videotoolbox"])
        );
        assert!(
            args.windows(2)
                .any(|args| args == ["-c:v", "h264_videotoolbox"])
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
}
