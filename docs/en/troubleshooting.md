# Troubleshooting

[简体中文](../zh-CN/troubleshooting.md) | [Documentation](README.md)

## macOS Cannot Verify the App Is Free of Malware

Current macOS packages use an ad-hoc signature. A free Apple developer account cannot obtain the Developer ID Application certificate required for distribution outside the App Store or notarize a release package. On first launch, macOS may therefore report: “Apple could not verify ‘DeviceHub Mask’ is free of malware that may harm your Mac or compromise your privacy.” This does not mean macOS detected malware; it means Apple cannot verify the publisher identity or a notarization ticket.

Download only from the project's GitHub Releases and verify the accompanying SHA-256 file first. In Finder, Control-click the app and choose **Open**, or go to **System Settings > Privacy & Security** and select **Open Anyway** beside the blocked-app record. If macOS still prevents launch, remove the quarantine attribute from this DeviceHub Mask bundle only, then open it:

```sh
sudo xattr -rd com.apple.quarantine "/Applications/DeviceHub Mask.app"
open "/Applications/DeviceHub Mask.app"
```

Replace the full path if the app is installed elsewhere. Do not run this command against `/Applications`, `~/Downloads`, or another entire directory, and do not disable Gatekeeper globally. macOS may attach quarantine again after downloading a new version; verify that version's source and checksum before clearing it.

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
- Runtime discovery rejects bundled candidates with missing execute permissions or a binary format for another operating system, logs the rejected path, and continues to system locations. A `cannot execute binary file` warning usually means an old cross-target sidecar remains in a build output; run `npm run ffmpeg:prepare -- --force` for the current host or set `DEVICEHUB_FFMPEG` to a valid binary.
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

## Remote Pairing Verification Ends With Early EOF

`remote pairing verification failed: Socket(... UnexpectedEof ... "early eof")` means the app reached the device's Bonjour `_remotepairing._tcp` service, but the device closed that TCP stream before sending a complete RemotePairing handshake frame. It does not by itself mean the saved authorization is invalid. A device lock or network transition, an iOS RemotePairing service restart, a recently replaced Bonjour address, or a previous tunnel still shutting down can all produce this transient result.

DeviceHub Mask preserves the existing credentials and retries transient disconnects with fresh sockets before rebuilding the complete Wi-Fi tunnel with bounded backoff. Keep the device awake, unlocked, and on the same network. Do not remove trust for a single EOF.

If the app repeatedly reports `Wi-Fi control authorization is no longer accepted by the device`, create fresh credentials as follows:

1. Connect the device with a data-capable USB cable and unlock it.
2. In the connection center, select the **USB** entry for that physical device, not its Wi-Fi entry.
3. Wait until the USB entry is paired and its device information is available.
4. Open the Device inspector's **Info** tab and scroll to **Computer trust**.
5. Select **Forget computer trust** and confirm. The button is available only for a selected, paired USB transport.
6. Disconnect and reconnect the USB cable if the device does not immediately ask to pair again.
7. Keep the device unlocked, select **Trust device** when shown in DeviceHub Mask, approve **Trust This Computer** on the device, and enter its passcode.
8. Keep USB connected until Wi-Fi authorization completes and the Wi-Fi session starts; then select the Wi-Fi transport.

The in-app removal clears the device's Lockdown relationship, the host pairing record, and DeviceHub Mask's separate RemotePairing credentials. It is not the same as forgetting a Wi-Fi network or disconnecting a session. If the in-app action cannot be reached, the device-wide last resort is **Settings > General > Transfer or Reset iPhone/iPad > Reset > Reset Location & Privacy**. That resets trust decisions for every computer and does not by itself clean DeviceHub Mask's host-side credentials, so prefer the targeted in-app action.

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
