// HEVC decode via an `ffmpeg` subprocess: Annex-B in on stdin, PAM (P7) frames
// out on stdout. PAM is self-describing (size in every header, so resolution
// changes need no extra signalling). The stream has no useful alpha channel, so
// RGB24 saves 25% of pipe and memory bandwidth compared with RGBA.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::Notify;

use crate::protocol::{Frame, FrameSlot};

/// Spawn `ffmpeg` decoding raw HEVC (Annex-B on stdin) to PAM frames on stdout.
/// stderr is piped so the session can watch it for decode errors.
pub fn spawn_ffmpeg() -> std::io::Result<(Child, ChildStdin, ChildStdout, ChildStderr)> {
    let ffmpeg = resolve_ffmpeg()?;
    let max_dimension = configured_max_dimension(
        std::env::var_os("DEVICEHUB_VIDEO_MAX_DIMENSION"),
        cfg!(windows),
    );
    tracing::info!(path = %ffmpeg.display(), ?max_dimension, "using ffmpeg");
    let mut command = Command::new(ffmpeg);
    command
        // Do *not* add `-fflags nobuffer`: it makes ffmpeg skip the opening IDR +
        // parameter sets, so every P-frame fails with "Could not find ref".
        .args(["-flags", "low_delay"])
        .args(["-hwaccel", "auto"])
        .args(["-f", "hevc", "-i", "pipe:0"]);
    if let Some(max_dimension) = max_dimension {
        command.args([
            "-vf",
            &format!(
                "scale=w='min(iw,{max_dimension})':h='min(ih,{max_dimension})':force_original_aspect_ratio=decrease:force_divisible_by=2"
            ),
        ]);
    }
    let mut child = command
        .args([
            "-an",
            "-f",
            "image2pipe",
            "-vcodec",
            "pam",
            "-pix_fmt",
            "rgb24",
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
}
