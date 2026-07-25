# Troubleshooting

[简体中文](../zh-CN/troubleshooting.md) | [Documentation](README.md)

## A Debug Executable Opens a Blank Window

`tauri dev` compiles a WebView that loads Vite from `127.0.0.1:5173`. Running that development executable after Vite stops produces a blank page.

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

The default random loopback port avoids normal conflicts. Stop stale `devicehub-mask`, `devicehub_rs`, and FFmpeg processes that may still own the CoreDevice session. Keep `DEVICEHUB_ADDR` bound to loopback. The API has no web root and always requires its launch token.

## Collect Runtime Logs

Open Settings > Diagnostics and select **Open log directory**. Logs are JSON Lines files, rotate daily, and retain the latest seven files. Enable detailed Debug logging only while reproducing the problem, then disable it before performance measurements. Include the Run ID from the settings page when sharing excerpts from one application run. Tokens, clipboard contents, video frames, and raw HID reports are not written by the diagnostics bridge.

If the UI cannot open, use `DEVICEHUB_LOG=devicehub_mask=debug` when launching from a terminal. Do not use an unrestricted global `trace` filter for long captures.

## FFmpeg Is Missing or Device Audio Is Silent

- Packaged applications include a checksum-verified FFmpeg executable and use it before `PATH`. Development builds can use `brew install ffmpeg` on macOS. They also search `/opt/homebrew/bin/ffmpeg`, `/usr/local/bin/ffmpeg`, and `/opt/local/bin/ffmpeg` because they do not inherit the shell `PATH`.
- Set `DEVICEHUB_FFMPEG` to an absolute executable path to explicitly override the bundled or system copy while diagnosing AAC-ELD audio decoding.
- Windows: `winget install --id Gyan.FFmpeg --exact`, then open a new terminal.
- Custom path: set `DEVICEHUB_FFMPEG` to the executable's absolute path for the application process.
- Unlock and reconnect the device, close other display sessions, and inspect the status badge and Rust logs for RSD or displayservice failures.

## Displayservice Is Not Advertised

If RSD does not advertise `com.apple.coredevice.displayservice`, connection and the RSD handshake succeeded but the device is not exposing screen streaming. This is not proof that USB is unsupported.

On Windows, keep the phone connected and unlocked, then run:

```powershell
.\scripts\prepare-windows-device.ps1
```

The helper checks Developer Mode, mounts the Personalized Developer Disk Image, performs a new USB RSD handshake, and verifies the service name. Reconnect after successful preparation. A persistent failure may require completing cable pairing once in Xcode Device Hub or may indicate an incompatible iOS beta.

Use `RUST_LOG=devicehub_mask::session=debug` for the complete RSD service list. An address such as `192.168.9.147:62078` is a Lockdown endpoint, not the RSD endpoint returned by CoreDeviceProxy, and cannot make a missing service appear.

## CoreDevice Error 9021

The device rejected the remote-control capability. Support depends on the hardware and iOS combination; it does not mean every device below iOS 27 is unsupported. For the rejected device, updating to iOS 27 or using supported newer hardware is required.

Changing USB/Wi-Fi transport, FFmpeg, app signing, or retrying cannot bypass this device-side check. DeviceHub Mask reports the localized description rather than the archived binary plist. There is currently no screen-only fallback because the initial audio media session also establishes authorization for video and Universal HID control.

## Touch Coordinates Are Incorrect or Landscape Is Stretched

Do not force the canvas to an arbitrary width and height. DeviceHub Mask contain-fits the rotated frame with one shared scale and normalizes touch inside the displayed rectangle. Report a regression with the source resolution, display resolution, orientation, and a screenshot.

## Windows CPU Usage Is High

Live video uses WebCodecs exclusively. If Windows reports `OperationError: Unsupported configuration`, the app reads the HEVC profile and level from the SPS and retries conservative `hev1` and `hvc1` configurations. If all configurations fail, WebView2 or its system codec cannot decode the device stream. GPU HEVC capability alone is insufficient; Windows commonly requires HEVC Video Extensions. There is no Native / FFmpeg video fallback.

`browser video client lagged` means the WebSocket sender briefly fell behind the compressed HEVC broadcast; it does not mean CoreDevice stopped producing frames. The app discards dependent frames, repeatedly requests IRAP until resynchronized, and resumes without reconnecting. If the toolbar remains at nonzero Source/Decode but zero Send/Display for more than a few seconds, collect Debug logs containing the lag warning, following PLI/FIR requests, received IRAP entries, and `devicehub_mask::perf` output.

Use the live Decode / Send / Display FPS and decoder-ingress latency metrics:

- Source FPS reports complete RTP frame markers; Published FPS reports compressed access units admitted to the WebCodecs transport.
- Send and Display FPS should track Published FPS. The backend allows at most two unacknowledged packets and the frontend has a bounded eight-packet ingress queue.
- Debug performance logs report RTP timestamp deltas, source arrival jitter, HEVC queue wait, frame age, WebSocket write, decoder acceptance, presentation acknowledgement, WebCodecs output, Canvas draw, and per-stage drops.
- Windows decodes the source resolution exposed by WebCodecs; there is no RGB24/YUV420P transport or FFmpeg dimension limit.

These metrics and Debug log fields are platform-independent. Compare macOS, Windows, and Linux with Release builds, the same device/content, decoded dimensions, and `DEVICEHUB_VIDEO_IN_FLIGHT_FRAMES` value. Record CPU usage, all FPS metrics, ingress/presentation latency, device resolution, GPU, WebView version, and whether an installed release or Debug build was tested. Debug builds are not representative of Release performance.

## A Process-Filtered Network Capture Contains No Packets

The filter uses the PID snapshot selected before capture. Relaunching the app assigns a new PID; refresh the running-process inventory, select the new entry, and start another capture. A nonzero Excluded packets count with zero written packets means pcapd traffic was present but none was attributed to the selected primary or effective PID. Choose All processes to determine whether the service itself is producing packets.

## Bluetooth Capture Contains No Packets

Install Apple's Bluetooth Logging configuration profile on the iPhone before starting an HCI capture. `BTPacketLogger` can accept the connection without that profile but remain silent, so a valid 24-byte PCAP containing only the global header is expected in that case. Keep the target Bluetooth controller or audio device active during the capture and inspect `bluetooth.capture` in Service health if starting the service itself fails.

## Update Check Fails

- Confirm the nightly release has `latest.json`, the platform updater artifact, and matching `.sig` file.
- Confirm `plugins.updater.pubkey` in `src-tauri/tauri.conf.json` matches the CI private key.
- Verify the installed version is lower than the manifest version.
- Windows and Linux update from NSIS and AppImage; macOS uses the app archive.

See [Distribution](distribution.md) for key setup and artifact names.
