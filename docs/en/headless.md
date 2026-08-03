# Headless Service

[简体中文](https://github.com/boa-z/devicehub-mask/blob/main/docs/zh-CN/headless.md) | [Documentation](https://github.com/boa-z/devicehub-mask/blob/main/docs/en/README.md)

`devicehub-headless` is the experimental standalone native host. It does not link Tauri or Wry, but shares the desktop application's device runtime, authenticated HTTP/WebSocket API, WebCodecs video path, and React UI. The service listens on loopback by default and is intended for operating devices from a browser on a computer without a desktop window.

## Prerequisites

- The iPhone or iPad is paired with and trusts the host computer. Keep it unlocked while testing.
- Developer Mode is enabled and a Developer Disk Image matching the device OS is mounted. See [Getting Started](https://github.com/boa-z/devicehub-mask/blob/main/docs/en/getting-started.md) for device preparation.
- Windows has Apple Mobile Device Service installed and running; Linux has the required usbmuxd/libusb runtime.
- The browser supports HEVC WebCodecs. On Windows, availability depends on the browser, GPU driver, and HEVC decoding capability supplied by the system.
- Device audio requires the packaged FFmpeg or a binary selected with `--ffmpeg`. FFmpeg is not used for video decoding.

New headless data directories enable device audio by default, so the mobile app can establish its audio stream after requesting `audio_demand`. An existing `settings.json` with `audio_enabled: false` remains disabled and can be enabled through the host settings API or the shared Web UI.

## Using a Nightly Package

Download the headless archive for the host platform and its adjacent `.sha256` file from the [nightly release](https://github.com/boa-z/devicehub-mask/releases/tag/nightly):

```text
devicehub-mask-headless_<version>+<build>_macos-universal.tar.gz
devicehub-mask-headless_<version>+<build>_windows-x64.zip
devicehub-mask-headless_<version>+<build>_linux-x64.tar.gz
devicehub-mask-headless_<version>+<build>_linux-arm64.tar.gz
```

Verify and fully extract the archive. Do not move only the executable: `devicehub-headless`, `dist/`, FFmpeg, netmuxd, licenses, and the startup guides must remain together. Use `shasum -a 256 <archive>` or `sha256sum <archive>` on macOS/Linux, and `Get-FileHash <archive> -Algorithm SHA256` on Windows.

Start the service from the top level of the extracted directory:

```sh
./devicehub-headless
```

Windows PowerShell:

```powershell
.\devicehub-headless.exe
```

Open the printed `Open http://127.0.0.1:8080/#access_token=...` URL. The temporary token is carried in a URL fragment that is not sent with ordinary HTTP requests; the frontend removes it from the address bar after bootstrap. Do not share the startup URL with an untrusted party.

Press `Ctrl+C` to stop the HTTP/MCP listeners, device session, and sidecars. Do not remove a data directory while it is in use.

## Developing From Source

Install Node.js, npm, Rust stable, and the native build dependencies for the host platform. From the repository root:

```sh
npm ci
npm run headless:dev -- --listen 127.0.0.1:8080
```

`headless:dev` builds the shared React frontend before running the Cargo debug binary. It does not launch, install, or replace the Tauri desktop application. The default paths are `dist/` and `./.devicehub-mask/` under the repository root.

To build and run only a release binary, prepare the frontend and sidecars first and pass their locations explicitly:

```sh
npm ci
npm run sidecars:prepare
npm run build
cargo build -p devicehub-headless --release --locked
./src-tauri/target/release/devicehub-headless \
  --frontend-dir ./dist \
  --ffmpeg ./src-tauri/resources/ffmpeg \
  --netmuxd ./src-tauri/resources/netmuxd \
  --data-dir ./.devicehub-mask
```

Append `.exe` to the three executable names on Windows. Automatic sidecar discovery is relative to the headless executable, so explicit paths are recommended when running the raw Cargo artifact from the repository.

## Building a Distributable Archive

The packaging script builds the release executable and frontend, prepares checksum-verified sidecars, copies licenses, and produces an archive plus `.sha256`:

```sh
npm ci
npm run headless:package -- --version 0.1.0 --build-number 1
```

Outputs are written to `release-artifacts/`. `--version` should match the project version and `--build-number` should identify the build. The script supports the current release matrix of macOS arm64/x64, Windows x64, and Linux x64/ARM64; CI uses `universal-apple-darwin` to merge the Universal macOS executable. See [CI, Releases, and Updates](https://github.com/boa-z/devicehub-mask/blob/main/docs/en/distribution.md) for the complete release policy.

Before committing or building a package, run at least:

```sh
npm run verify
```

Run `npm run verify:full` for substantial changes and releases. The full gate compiles the desktop debug target but does not start or install it.

## Common Startup Configurations

Select a persistent data directory and initial device:

```sh
./devicehub-headless \
  --data-dir /var/lib/devicehub-mask \
  --device <DEVICE_IDENTIFIER>
```

Use a persistent token so browser clients can reconnect:

```sh
openssl rand -hex 32 > devicehub.token
chmod 600 devicehub.token
./devicehub-headless --token-file ./devicehub.token
```

Windows PowerShell:

```powershell
[guid]::NewGuid().ToString("N") | Set-Content -NoNewline devicehub.token
.\devicehub-headless.exe --token-file .\devicehub.token
```

The token must be one URL-safe line of at least 24 characters containing only letters, digits, `-`, and `_`. On Unix, the service rejects a token file readable by group or other users.

Override local tools or use only the system usbmuxd:

```sh
./devicehub-headless --ffmpeg /opt/devicehub/ffmpeg --netmuxd off
./devicehub-headless --usbmuxd 127.0.0.1:27015
```

Enable the local MCP server:

```sh
./devicehub-headless --mcp-listen 127.0.0.1:8009
```

The MCP Streamable HTTP endpoint is `http://127.0.0.1:8009/mcp`. MCP currently has no authentication and is therefore restricted to loopback.

## LAN Access

A non-loopback listener requires explicit opt-in:

```sh
./devicehub-headless \
  --listen 0.0.0.0:8080 \
  --allow-lan \
  --token-file ./devicehub.token
```

Replace `127.0.0.1` in the printed URL with the server's LAN address when opening it from another computer. If opening the host firewall, restrict the rule to trusted LAN sources and never port-forward the listener directly to the Internet.

The built-in server provides token authentication but not TLS, user accounts, rate limiting, or Internet deployment protection. Browser APIs such as WebCodecs commonly require a secure context: browsers trust `http://localhost` specially, but may reject `http://<LAN-IP>`. Full LAN video access should terminate HTTPS at a trusted reverse proxy and forward the static UI, `/api/*`, and WebSocket `/api/ws`. TLS does not replace the access token.

When `--allow-lan` publishes a non-loopback listener, headless also advertises `_devicehub._tcp.local.` over Bonjour/mDNS. The record contains only the service port and `targets=ios`; it never contains the access token. Clients must still authenticate with the token from the printed URL or `--token-file`.

## Option Reference

| Option | Default | Purpose |
| --- | --- | --- |
| `--listen <IP:PORT>` | `127.0.0.1:8080` | Browser HTTP/WebSocket listener |
| `--allow-lan` | off | Permit non-loopback `--listen`; does not add TLS |
| `--data-dir <PATH>` | `./.devicehub-mask` | Settings, pairings, profiles, and transfer staging |
| `--frontend-dir <PATH>` | `./dist` | Vite output containing `index.html` |
| `--token-file <PATH>` | temporary random token | Read a persistent API token |
| `--device <IDENTIFIER>` | automatic selection | Device to prefer after startup |
| `--ffmpeg <PATH>` | automatic discovery | AAC-ELD audio decoder path |
| `--netmuxd <PATH\|off>` | automatic discovery | netmuxd path, or disable its sidecar |
| `--usbmuxd <ADDRESS>` | platform default | Override the system usbmuxd address |
| `--mcp-listen <IP:PORT>` | off | Optional loopback-only MCP listener |

Run `./devicehub-headless --help` for the options supported by the current binary. Relative paths are resolved from the startup working directory, not from a configuration-file location.

## Data and Logging

The data directory defaults to `.devicehub-mask/` under the startup directory:

```text
.devicehub-mask/
├── settings.json
├── pairings/
├── profiles/
└── transfers/
```

`transfers/` is isolated staging for browser file transfers. Normal operations remove their staging immediately, and startup removes data left by an abnormal exit. Pairing records and profiles must persist; production deployments should restrict the data directory to the service account and back it up appropriately.

Logs are written to standard error by default. Use the standard tracing filter for additional detail:

```sh
RUST_LOG=devicehub_mask=debug,devicehub_runtime=debug ./devicehub-headless
```

Production deployments may use systemd, launchd, a Windows service wrapper, or a container runtime for log collection and restart policy. Allow sufficient time for the termination signal to perform graceful shutdown.

## Browser Capabilities and Limitations

Browser fullscreen, device control, WebCodecs video, device audio, AFC/application-storage single-file transfers, and crash-report downloads are available. Browser autoplay policy may require one page click before audio starts. AFC and application-storage uploads are limited to 64 MiB and downloads to 256 MiB; directory transfer remains desktop-only.

Multiple devices may remain connected at once. Each browser tab selects an exact transport-aware device ID, and its HTTP and WebSocket requests stay scoped to that session. Video work runs only while a page displays that device. Audio decoding runs only while at least one page requests enabled, unmuted audio for it; muting every viewer releases the decoder. Performance sampling and device-log streaming are also independently demand-gated per device. Closing a tab automatically releases its media demand, while the lightweight device session remains available for quick switching.

Browser input uses a device-scoped control lease. The first WebSocket connected to a device can send touch, keyboard, rotation, text, and hardware-button input. Additional tabs for the same exact device remain connected as view-only observers and continue receiving status, video, audio, and events. When the controlling tab closes, one waiting observer acquires the lease automatically without reconnecting. Tabs controlling different devices hold independent leases and can operate concurrently. The UI shows **View only** and disables its input controls while another tab owns the device. HTTP management operations remain token-authorized and explicitly device-scoped; the WebSocket lease does not silently redirect them. MCP sessions keep their own explicit target and remain loopback-only.

Desktop-only capabilities such as always-on-top windows, installer updates, native file dialogs, opening server directories, and host clipboard synchronization are disabled. Packet capture, sysdiagnose, log archive, Developer Image, and other workflows that still require host paths have not all received browser transfer adapters yet.

DeviceHub Mask does not install, sideload, sign, or upgrade iOS applications. These capabilities remain outside both desktop and headless product scope.

## Troubleshooting

- `frontend build is missing`: start from the extracted directory containing `dist/`, pass the correct `--frontend-dir`, or run `npm run build` before a source launch.
- `address already in use`: choose another `--listen` port or stop the process holding it.
- A non-loopback listener is rejected: pass `--allow-lan`; this acknowledges exposure but is not a security configuration.
- The browser returns `401`: reopen the complete URL printed by the current process. Persistent deployments must ensure every client uses the same protected token file.
- The page opens but WebCodecs is unavailable: verify that the browser is in a secure context, then check Windows HEVC capability, GPU drivers, and hardware acceleration.
- No device appears: confirm that the device is unlocked and trusted, Developer Mode/DDI is ready, and Apple Mobile Device Service or usbmuxd is running; then refresh devices in the UI.
- No audio: confirm that FFmpeg is available and that an existing `settings.json` does not set `audio_enabled` to `false`; browser clients also need a click on the device toolbar's audio button to satisfy autoplay policy. Browsers may treat `http://localhost` and `http://<LAN-IP>` differently. If playback is still blocked, use an HTTPS reverse proxy and inspect the service log for `browser_playback_suspended` or `browser_playback_failed` diagnostics and the FFmpeg path.
- A Wi-Fi device is missing: pair it over USB first, confirm that the pairing directory is writable, and keep the device and server on the same trusted network.
