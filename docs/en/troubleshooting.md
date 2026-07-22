# Troubleshooting

[简体中文](../zh-CN/troubleshooting.md) | [Documentation](README.md)

## A Debug Executable Opens a Blank Window

`tauri dev` compiles a WebView that loads Vite from `127.0.0.1:5173`. Running
that development executable after Vite stops produces a blank page.

Use hot reload with:

```sh
npm run tauri:dev
```

Or build an embedded standalone frontend:

```sh
npm run tauri:build:debug
./src-tauri/target/debug/devicehub-mask
```

Development and standalone builds use separate Cargo target directories.

## The Private Backend Does Not Start

The default random loopback port avoids normal conflicts. Stop stale
`devicehub-mask`, `devicehub_rs`, and FFmpeg processes that may still own the
CoreDevice session. Keep `DEVICEHUB_ADDR` bound to loopback. The API has no web
root and always requires its launch token.

## Collect Runtime Logs

Open Settings > Diagnostics and select **Open log directory**. Logs are JSON
Lines files, rotate daily, and retain the latest seven files. Enable detailed
Debug logging only while reproducing the problem, then disable it before
performance measurements. Include the Run ID from the settings page when
sharing excerpts from one application run. Tokens, clipboard contents, video
frames, and raw HID reports are not written by the diagnostics bridge.

If the UI cannot open, use `DEVICEHUB_LOG=devicehub_mask=debug` when launching
from a terminal. Do not use an unrestricted global `trace` filter for long
captures.

## FFmpeg Is Missing or No Frames Appear

- macOS: `brew install ffmpeg`. Packaged apps also search
  `/opt/homebrew/bin/ffmpeg`, `/usr/local/bin/ffmpeg`, and
  `/opt/local/bin/ffmpeg` because they do not inherit the shell `PATH`.
- Windows: `winget install --id Gyan.FFmpeg --exact`, then open a new terminal.
- Custom path: set `DEVICEHUB_FFMPEG` to the executable's absolute path for the
  application process.
- Unlock and reconnect the device, close other display sessions, and inspect
  the status badge and Rust logs for RSD or displayservice failures.

## Displayservice Is Not Advertised

If RSD does not advertise `com.apple.coredevice.displayservice`, connection and
the RSD handshake succeeded but the device is not exposing screen streaming.
This is not proof that USB is unsupported.

On Windows, keep the phone connected and unlocked, then run:

```powershell
.\scripts\prepare-windows-device.ps1
```

The helper checks Developer Mode, mounts the Personalized Developer Disk Image,
performs a new USB RSD handshake, and verifies the service name. Reconnect after
successful preparation. A persistent failure may require completing cable
pairing once in Xcode Device Hub or may indicate an incompatible iOS beta.

Use `RUST_LOG=devicehub_mask::session=debug` for the complete RSD service list.
An address such as `192.168.9.147:62078` is a Lockdown endpoint, not the RSD
endpoint returned by CoreDeviceProxy, and cannot make a missing service appear.

## CoreDevice Error 9021

The device rejected the remote-control capability. Support depends on the
hardware and iOS combination; it does not mean every device below iOS 27 is
unsupported. For the rejected device, updating to iOS 27 or using supported
newer hardware is required.

Changing USB/Wi-Fi transport, FFmpeg, app signing, or retrying cannot bypass this
device-side check. DeviceHub Mask reports the localized description rather than
the archived binary plist. There is currently no screen-only fallback because
the initial audio media session also establishes authorization for video and
Universal HID control.

## Touch Coordinates Are Incorrect or Landscape Is Stretched

Do not force the canvas to an arbitrary width and height. DeviceHub Mask
contain-fits the rotated frame with one shared scale and normalizes touch inside
the displayed rectangle. Report a regression with the source resolution,
display resolution, orientation, and a screenshot.

## Windows CPU Usage Is High

Use the live Decode / Send / Display FPS and JPEG latency metrics:

- Source FPS reports complete RTP frame markers; Decode and Published FPS separate
  FFmpeg output from duplicate-frame suppression.
- Send and Display FPS should track Published FPS. Up to two JPEG frames are in
  flight so backend encoding can overlap WebView decoding without an unbounded queue.
- Debug performance logs also report RTP timestamp deltas, source arrival jitter,
  HEVC queue wait, JPEG encode, frame age, WebSocket write, presentation
  acknowledgement, frontend JPEG decode, Canvas draw, and per-stage dropped frames.
- Windows defaults to a 1920-pixel decoded long edge and RGB24 transport.

These metrics and Debug log fields are platform-independent. Compare macOS,
Windows, and Linux with Release builds, the same device/content, pixel format,
decoded dimensions, and `DEVICEHUB_VIDEO_IN_FLIGHT_FRAMES` value; do not compare
an idle screen on one host with active motion on another.

Try a smaller decode limit without changing aspect ratio:

```powershell
$env:DEVICEHUB_VIDEO_MAX_DIMENSION = "1280"
npm run tauri:dev
```

Set it to `0` only when diagnosing native resolution. Record CPU usage, all FPS
metrics, JPEG latency, device resolution, GPU, and whether the installed release
or a debug build was tested. Debug builds are not representative of release
performance.

## Update Check Fails

- Confirm the nightly release has `latest.json`, the platform updater artifact,
  and matching `.sig` file.
- Confirm `plugins.updater.pubkey` in `src-tauri/tauri.conf.json` matches the CI
  private key.
- Verify the installed version is lower than the manifest version.
- Windows and Linux update from NSIS and AppImage; macOS uses the app archive.

See [Distribution](distribution.md) for key setup and artifact names.
