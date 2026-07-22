# DeviceHub Mask

DeviceHub Mask is a Tauri 2 desktop workspace for controlling iOS games from
macOS, Windows, and Linux. It combines a React mapping editor inspired by
[scrcpy-mask](https://github.com/AkiChase/scrcpy-mask) with the CoreDevice screen
streaming, orientation handling, and Universal HID support developed in
`devicehub_rs`.

The project does not use eframe, iPhone Mirroring, ADB, or scrcpy's Android
transport.

## Features

- Native desktop window powered by Tauri 2, using WKWebView on macOS and
  WebView2 on Windows
- Live iOS screen at up to 60 FPS over CoreDevice displayservice and RTP/HEVC
- Latest-frame 60 FPS delivery with JPEG caching and decode backpressure
- Aspect-ratio-preserving portrait and landscape rendering
- Up to five typed Universal HID contacts in one report
- Concurrent keyboard mappings, WASD direction pad, and direct pointer gestures
- Mutually exclusive mapping and raw keyboard HID modes with safe key release
- Drag-to-place overlays and profile persistence
- Device, keyboard mapping, and application settings workspaces
- Live Lockdown device metadata and CoreDevice AppService app browsing/launching
- Native IPA selection, asynchronous installation progress, and confirmed safe
  removal of removable user applications
- Read-only installed provisioning profile inspection through Misagent, with
  expiration, team, app identifier, development, and device-scope metadata
- Runtime Simplified Chinese and English localization with persistent selection
- scrcpy-mask profile import/export for taps and button direction pads
- Always-visible hardware buttons with user-defined keyboard shortcuts
- Desktop controls for always-on-top, fullscreen, and inspector visibility
- Live decode, transmit, render, JPEG latency, and bandwidth diagnostics
- Signed in-app nightly updates on macOS, Windows, and Linux with download
  progress and automatic restart

## Architecture

```text
Tauri 2 / WKWebView
        |
React 19 + Ant Design mapping workspace
        |
Authenticated private loopback transport (random port)
        |
Rust / Axum control and stream service
        |
CoreDevice displayservice + Universal HID
```

The Axum transport is internal to the desktop process: it binds a random
loopback port, requires a per-launch token obtained through Tauri IPC, and does
not serve frontend files or a browser entry point. The frontend sends normalized
typed contacts, never raw HID reports. Rust validates contact identities, the
five-contact limit, coordinate ranges, and orientation before dispatching input.
The CoreDevice session runs on a dedicated Tokio runtime because several
`idevice` service objects cannot safely be moved through a normal `tokio::spawn`
boundary.

Device metadata is read once through Lockdown when a session connects. App list
and launch requests prefer a long-lived CoreDevice AppService client owned by
that same session, so the UI does not create a second RSD tunnel for each
operation. App listing falls back to Lockdown's Installation Proxy when the
newer AppService is absent. IPA installation and app removal run in independent
Tokio tasks with fresh Installation Proxy connections, so long uploads and
device-side mutations do not block video, HID, or app-list requests. Switching
devices or ending the session aborts the active mutation. Before uninstalling,
the backend queries the target again and requires device metadata to identify it
as a removable, non-first-party user app; frontend state is never treated as
authorization. The private API exposes one operation state with stage and
device-reported progress, and the Apps tab refreshes automatically after a
successful mutation.

The current `idevice` package helper reads a selected IPA into memory before AFC
upload and reports percentages only after Installation Proxy begins installing.
The UI therefore shows an indeterminate **Preparing and uploading** stage rather
than inventing upload progress. Large IPA memory use and byte-level AFC progress
remain candidates for an upstream `idevice` streaming API.

Provisioning profiles are read through a long-lived
Misagent connection and decoded as CMS SignedData before their plist metadata is
exposed; raw profile payloads and provisioned device identifiers never cross the
private API. A malformed profile is isolated as an error row instead of failing
the whole list. If DisplayService is unavailable, the backend keeps this reduced
management session alive instead of discarding usable Lockdown capabilities;
screen control and app launching remain explicitly unavailable.
Device-management routes are exposed only through the authenticated private
loopback API and return `503` while no session is active.

The `idevice` dependency is temporarily pinned to the reviewed
`0371286` revision from the project fork. That revision adds the
`requireContainerAccess=false` field required by the iOS 27 AppService request
decoder. Replace the pin with an upstream release after the fix is merged and
published.

The repository follows the standard Tauri 2 layout:

```text
devicehub-mask/
├── package.json          # Vite, React, and Tauri CLI scripts
├── src/                  # React application
├── dist/                 # generated frontend assets
└── src-tauri/
    ├── Cargo.toml
    ├── tauri.conf.json
    ├── capabilities/
    ├── icons/
    └── src/              # Rust desktop backend
```

## Requirements

- macOS with Xcode Command Line Tools, or Windows 10/11 with WebView2 and the
  Microsoft C++ Build Tools, or a Linux desktop with WebKitGTK 4.1
- A paired and trusted iPhone or iPad
- Developer Mode enabled on the iOS device when required by the OS version
- Rust stable toolchain
- Node.js 22 or newer and npm
- `ffmpeg` available on `PATH`

The interface uses the native system UI font stack on every platform. No web
font download, font file, or font installation is required.

Install the common macOS dependencies with Homebrew if they are not already
available:

```sh
xcode-select --install
brew install node ffmpeg rustup
rustup-init
```

Open a new shell after installing Rust and verify the tools:

```sh
rustc --version
node --version
npm --version
ffmpeg -version
```

On Windows, install the Rust MSVC toolchain, Node.js, FFmpeg, and Apple's device
support. The desktop version of iTunes installs Apple Mobile Device Service,
which exposes the usbmuxd endpoint used by `idevice` on `127.0.0.1:27015`.
Connect and trust the device once in iTunes before starting DeviceHub Mask. Make
sure `ffmpeg.exe` is on `PATH`, or set `DEVICEHUB_FFMPEG` to its absolute path.
Python 3.12 is used only by the device-preparation helper described below; it is
not part of the application runtime.

```powershell
winget install --id Rustlang.Rustup --exact
winget install --id OpenJS.NodeJS.LTS --exact
winget install --id Gyan.FFmpeg --exact
winget install --id Kitware.CMake --exact
winget install --id NASM.NASM --exact
winget install --id 9NP83LWLPZ9K --source msstore
winget install --id Python.Python.3.12 --exact
rustup default stable-msvc
rustc --version
node --version
ffmpeg -version
Get-Service "Apple Mobile Device Service"
```

Visual Studio Build Tools with the **Desktop development with C++** workload is
also required to compile the native dependencies. The `turbojpeg` crate builds
its bundled libjpeg-turbo source statically; CMake and NASM are build-time
requirements, while no separate TurboJPEG DLL is needed at runtime. Release
builds can generate the normal Windows MSI and NSIS installers through Tauri.

CoreDevice display streaming works directly over USB. It does not require a
privileged TUN adapter, tunneld, Bonjour, Wi-Fi sync, or the phone's LAN address.
Before the first run, use the repository helper to verify Developer Mode, mount
the Personalized Developer Disk Image, and confirm that the USB RSD endpoint
advertises `com.apple.coredevice.displayservice`:

```powershell
.\scripts\prepare-windows-device.ps1
```

The helper creates an isolated runtime below
`%LOCALAPPDATA%\devicehub-mask\pymobiledevice3`, installs
`pymobiledevice3 9.38.0`, and exits after preparation and verification. It does
not need elevation and no helper process needs to remain running. The mounted
image may need to be prepared again after rebooting or upgrading the device.

## Get The Source

```sh
git clone https://github.com/boa-z/devicehub-mask.git
cd devicehub-mask
npm ci
```

`npm ci` installs both the frontend dependencies and the repository-local Tauri
CLI. A global `cargo-tauri` installation is not required.

## Prepare A Device

1. Connect the iOS device over USB.
2. Unlock it and accept the computer trust prompt.
3. Enable Developer Mode on the device.
4. On Windows, run `.\scripts\prepare-windows-device.ps1` once.
5. Keep the device unlocked for the first connection.
6. Close other DeviceHub processes that may already own the CoreDevice media
   session.

When usbmuxd reports both USB and network records for the same UDID, DeviceHub
Mask deliberately selects USB. The iTunes **Sync with this iPhone over Wi-Fi**
setting is unrelated to USB displayservice availability and is not required.

## Development

Start Vite, Tauri, the local stream service, and automatic Rust/frontend reload:

```sh
npm run tauri:dev
```

To request a specific device at startup, pass its UDID after `--`:

```sh
npm run tauri:dev -- -- 00008110-001624E2013A801E
```

The development frontend runs at `http://127.0.0.1:5173` inside the Tauri
WebView. It obtains the authenticated random backend endpoint through Tauri IPC;
Vite does not proxy or expose the device API. Development Rust artifacts are
isolated under `src-tauri/target/tauri-dev`; packaged and standalone debug builds
load embedded assets from the Tauri protocol instead.

Useful environment variables:

| Variable | Default | Purpose |
| --- | --- | --- |
| `DEVICEHUB_ADDR` | `127.0.0.1:0` | Private backend bind address (`0` selects a random port) |
| `DEVICEHUB_PROFILE_DIR` | Tauri application data directory | Mapping profile storage |
| `DEVICEHUB_FFMPEG` | auto-detected | Absolute path to the ffmpeg executable |
| `RUST_LOG` | DeviceHub info logging | Rust log filter |
| `DEVICEHUB_HID_DUMP` | unset | Export the Universal HID service plist |

## Local Production Build

Build the optimized frontend, Rust binary, macOS application, and configured
bundles:

```sh
npm run tauri:build
```

Or invoke the local CLI directly when build flags are needed:

```sh
npm run tauri -- build --bundles app
```

The main outputs are:

```text
src-tauri/target/release/devicehub-mask
src-tauri/target/release/bundle/macos/DeviceHub Mask.app
src-tauri/target/release/bundle/dmg/DeviceHub Mask_0.1.0_aarch64.dmg
```

Output names can vary slightly by Tauri CLI version and host architecture.

To produce a standalone debug executable without creating an installer, run:

```sh
npm run tauri:build:debug
./src-tauri/target/debug/devicehub-mask
```

Do not use a binary emitted by `tauri dev` as a standalone build. Development
binaries are compiled to load `http://127.0.0.1:5173`; after Vite exits that
WebView has no document to load. The `tauri:dev` script keeps those artifacts in
`src-tauri/target/tauri-dev` so they cannot overwrite the standalone debug path.

### Windows Build

On Windows, build the application and the configured MSI/NSIS installers with:

```powershell
npm run tauri:build
```

The installers are written below `src-tauri/target/release/bundle/msi` and
`src-tauri/target/release/bundle/nsis`. FFmpeg and Apple Mobile Device Service remain
runtime prerequisites and are not bundled.

### Linux Build

Install the Tauri 2 and native-code build dependencies on Ubuntu/Debian:

```sh
sudo apt-get install build-essential cmake nasm pkg-config libssl-dev \
  libudev-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev \
  librsvg2-dev patchelf ffmpeg
npm run tauri -- build --bundles appimage,deb
```

The packages are written below `src-tauri/target/release/bundle/appimage` and
`src-tauri/target/release/bundle/deb`. Linux device connectivity depends on a working
usbmuxd/Apple pairing setup and has not received the same device coverage as the
macOS and Windows paths.

### Universal macOS Build

Install both targets and request a universal binary:

```sh
brew install nasm
rustup target add aarch64-apple-darwin x86_64-apple-darwin
npm run tauri -- build \
  --target universal-apple-darwin \
  --bundles app
```

Universal artifacts are written below:

```text
src-tauri/target/universal-apple-darwin/release/bundle/macos/
```

### Reproducible DMG Packaging

The same helper used by CI can stamp an existing `.app` and create a DMG:

```sh
APP_VERSION=0.1.0 \
BUILD_NUMBER=1 \
APP_PATH="src-tauri/target/release/bundle/macos/DeviceHub Mask.app" \
  scripts/package-dmg.sh
```

The output is `dist/devicehub-mask_0.1.0+1.dmg` plus its SHA-256 checksum.

## Validation

Run all source checks before committing:

```sh
npm run lint
npm test
npm run build
cargo test --manifest-path src-tauri/Cargo.toml --locked
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --locked -- -D warnings
npm run tauri:build:debug
bash -n scripts/package-dmg.sh scripts/generate-update-manifest.sh
```

The multi-touch path has been validated on an iPhone 13 Pro Max. A two-contact
frame sent through the production typed path entered `21` in Calculator.

### Localization

DeviceHub Mask currently supports Simplified Chinese (`zh-CN`) and English
(`en-US`). On the first launch, the frontend selects Chinese for any `zh-*`
system locale and English for all other locales. Change the language under
**Application Settings > Appearance and language**. The selection applies
immediately to DeviceHub Mask and Ant Design controls and is persisted in the
WebView local storage key `devicehub-mask.locale`.

Translation resources are kept in:

```text
src/locales/en-US.ts
src/locales/zh-CN.ts
```

To add or change UI text, add the same key to both resources and use
`useTranslation()` in the component. `src/i18n.test.ts` verifies that
both resource trees contain the same keys. Protocol identifiers, key codes,
profile names, and user-authored mapping labels are deliberately not translated.
New default mapping labels are localized when a profile is first created;
switching languages never rewrites an existing profile.

The shared system font stack is declared as `--system-font` in
`src/styles.css` and passed to Ant Design from
`src/AppProviders.tsx`. Keep typography on this stack rather than
adding remote or bundled fonts.

### Workspace Pages

- **Device** contains device selection, the aspect-ratio-preserving live view,
  control mode, rotation, direct touch, hardware buttons, and an inspector for
  device identity, installed apps, IPA installation/removal, and provisioning
  profiles.
- **Keyboard Mapping** contains the live placement canvas, mapping inspector,
  hardware shortcut bindings, and profile management. Its editor reports the
  source and contain-fitted display resolutions, can switch between the live
  stream and a direction-correct frozen screenshot, and can save the captured
  frame as a PNG. A frozen screenshot remains available for offline editing
  after the device disconnects.
- **Application Settings** contains window state and update controls.

Changing pages releases every active touch, hardware button, and keyboard usage.

### Profile Management And scrcpy-mask Compatibility

The keyboard mapping page follows scrcpy-mask's profile workflow: list profiles,
select the displayed profile, mark one profile active, create, duplicate, rename,
delete, import, and export. The active profile cannot be deleted. Profiles are
stored as validated JSON files in the DeviceHub Mask application data directory.

DeviceHub Mask can import scrcpy-mask `0.0.1` JSON files and export the current
profile back to that format. All thirteen controller types are preserved,
including nested sequence positions, bindings, release modes, timing, and
script fields. Saved profiles may contain more than five mappings; the runtime
selects at most five unique active contact identities for each Universal HID
frame.

### Hardware Button Shortcuts

The stage toolbar always exposes Home, Lock, Volume Up, Volume Down, Mute, Siri,
and Action controls. Configure keyboard shortcuts in **Hardware Button
Shortcuts** at the bottom of the mapping inspector, then save the profile. Click
a shortcut field and press a key to bind it; press Backspace or Delete to clear
it. Hardware shortcuts are active while edit mode is off and preserve key-down /
key-up timing for holds such as Siri. A key cannot be shared with a touch mapping
or another hardware button.

### Keyboard Input Mode

Use the **Mapping / Keyboard passthrough** segmented control above the device
view. Mapping mode routes keys to touch and hardware-button bindings. Keyboard
mode disables those mappings and forwards physical key-down/key-up events
directly to the iOS HID keyboard, including modifiers, arrows, navigation keys,
F1-F24, and the numeric keypad. Click the device view before typing.
`Ctrl+Shift+K` switches modes; switching modes, losing window focus, or closing
the WebSocket releases every active touch, hardware button, and keyboard usage
to prevent stuck input.

Keyboard mode represents physical HID keys. Text composition and CJK IME input
remain separate from this mode and should use clipboard synchronization until a
dedicated text-composition path is added.

### Desktop Controls

The title toolbar includes always-on-top, inspector visibility, and fullscreen
controls inspired by scrcpy-mask's compact desktop window. Hiding the inspector
gives the mirrored device the full workspace width without changing its aspect
ratio. Fullscreen transitions release active touch, hardware-button, and
keyboard state before resizing the window.

## CI And Nightly Releases

[`.github/workflows/nightly.yml`](.github/workflows/nightly.yml) runs only for
commits and manual dispatches. It does not use GitHub Environments or create
Deployment records:

- `verify` is a fail-independent macOS, Windows, and Linux matrix. Every leg
  runs frontend lint/tests/build, Rust format/tests/clippy, and a debug Tauri
  application build.
- `build-macos` produces a Universal Apple Silicon/Intel DMG and verifies the
  complete app signature and both executable architectures.
- `build-windows` produces x64 NSIS and MSI installers.
- `build-linux` produces x64 AppImage and DEB packages.
- `publish-nightly` waits for all packages, merges signed updater fragments into
  one `latest.json`, and atomically replaces the rolling nightly release assets.
  Per-platform and combined workflow artifacts are retained for 14 days.

Nightly artifacts are published at:

<https://github.com/boa-z/devicehub-mask/releases/tag/nightly>

Each release can contain:

```text
devicehub-mask_<base-version>+<build>_universal.dmg
devicehub-mask_<base-version>+<build>_universal.dmg.sha256
devicehub-mask_<base-version>-<build>_universal.app.tar.gz
devicehub-mask_<base-version>-<build>_universal.app.tar.gz.sig
devicehub-mask_<base-version>+<build>_x64-setup.exe
devicehub-mask_<base-version>+<build>_x64-setup.exe.sig
devicehub-mask_<base-version>+<build>_x64.msi
devicehub-mask_<base-version>+<build>_amd64.AppImage
devicehub-mask_<base-version>+<build>_amd64.AppImage.sig
devicehub-mask_<base-version>+<build>_amd64.deb
latest.json
```

Installer filenames retain the base version plus build number. The workflow run
number becomes `CFBundleVersion` on macOS, while all updater artifacts use the
shared `major.minor.<run-number>` version because SemVer build metadata such as
`0.1.0+12` does not participate in update ordering.

## Configure App Updates

Tauri updater packages are cryptographically signed independently of Apple code
signing. The committed updater public key is in `src-tauri/tauri.conf.json`; its private
key must never be committed.

Generate a replacement keypair only before publishing the first compatible
release:

```sh
mkdir -p .tauri
npm run tauri -- signer generate \
  --write-keys .tauri/devicehub-mask.key
```

Then update `plugins.updater.pubkey` in `src-tauri/tauri.conf.json` and configure these
repository-level Actions secrets under **Settings > Secrets and variables >
Actions**:

| Secret | Value |
| --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | Complete contents of `.tauri/devicehub-mask.key` |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Password used when generating the key, or empty |

With GitHub CLI authenticated for the repository:

```sh
gh secret set TAURI_SIGNING_PRIVATE_KEY < .tauri/devicehub-mask.key
gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD
```

If the private-key secret is absent, CI still publishes native installers for
all platforms but deliberately skips updater signatures and `latest.json`. Losing
or replacing the private key prevents existing installations from accepting
future updates.

At runtime the app checks the nightly endpoint shortly after startup. The
automatic check can be disabled under **Application Settings > Updates**; this
preference is stored locally under `devicehub-mask.updates.automatic`. The same
settings section always provides a manual **Check now** button. An accepted
update is downloaded, verified against the public key, installed, and followed
by an app restart.

## Apple Signing And Notarization

Updater signatures do not replace Apple Developer ID signing. Current nightly
applications receive a structurally valid ad-hoc signature after Universal
assembly and version stamping, so every executable slice and sealed resource can
be verified. Because ad-hoc signing does not establish publisher identity,
Gatekeeper may still require explicit local approval. Production distribution
should configure a Developer ID Application certificate, notarize the DMG, and
staple the notarization ticket.

## Troubleshooting

### A debug executable opens a blank window

`tauri dev` intentionally compiles a WebView that loads Vite from
`http://127.0.0.1:5173`. Running that development executable after the dev server
stops produces a blank window. Use `npm run tauri:dev` for hot reload, or run
`npm run tauri:build:debug` and then execute
`src-tauri/target/debug/devicehub-mask` for an embedded, standalone frontend.

The two commands use separate Cargo target directories, so switching between
development and standalone testing does not silently replace one binary with the
other.

### Private backend does not start

The default random loopback port avoids normal port conflicts. Stop stale
`devicehub-mask`, `devicehub_rs`, and ffmpeg processes if they still own the
CoreDevice session. `DEVICEHUB_ADDR` is intended for diagnostics; keep it bound
to a loopback address. The API remains token-authenticated and has no web root.

### No screen frames

- Install ffmpeg with `brew install ffmpeg`. Packaged applications do not inherit
  the shell's `PATH`, so DeviceHub Mask also checks `/opt/homebrew/bin/ffmpeg`,
  `/usr/local/bin/ffmpeg`, and `/opt/local/bin/ffmpeg` directly.
- On Windows, install FFmpeg with `winget install --id Gyan.FFmpeg --exact` and
  open a new terminal before launching the application.
- For a custom installation, launch with
  `DEVICEHUB_FFMPEG=/absolute/path/to/ffmpeg` or define that variable for the
  application process.
- Unlock and reconnect the iOS device.
- Close other CoreDevice display sessions.
- Check the status badge and Rust logs for displayservice or RSD failures.

### Display service is unavailable

If the log reports that RSD did not advertise
`com.apple.coredevice.displayservice`, the selected connection and RSD handshake
are already working, but the device is not exposing the screen-streaming
service. This does not mean that USB is unsupported. On Windows, keep the device
connected and unlocked and run:

```powershell
.\scripts\prepare-windows-device.ps1
```

The helper checks Developer Mode, mounts the Personalized Developer Disk Image,
then performs a fresh RSD handshake over USB and verifies the exact service name.
If preparation succeeds but the service remains absent, reconnect the device. A
persistent failure may require completing cable pairing once in Xcode 27 Device
Hub, or may indicate an incompatible iOS beta build.

The failure log includes the RSD service count and any advertised service names
containing `display`, `screen`, `media`, or `capture`. Set
`RUST_LOG=devicehub_mask::session=debug` to print the complete service-name list.

An iPhone address such as `192.168.9.147:62078` is a Lockdown endpoint, not the
RSD endpoint returned by CoreDeviceProxy. Supplying that address cannot replace
device preparation or make missing services appear.

### CoreDevice error 9021 requires a newer iOS version

If error 9021 is returned, the device itself rejected the remote-control
capability. Support depends on the hardware and iOS combination: this does not
mean every device below iOS 27 is unsupported, but the rejected device must be
updated to iOS 27 or replaced with a supported newer model. Changing between USB
and Wi-Fi, ffmpeg, app signing, and repeated retries cannot bypass this check.
DeviceHub Mask stops the failed session and reports a concise error instead of
printing the archived binary plist returned by CoreDevice. There is no screen-only
fallback in the current protocol because the initial audio media session also
establishes authorization for video and Universal HID control.

### Incorrect touch position

Do not force the canvas to fill an arbitrary width and height. The UI observes
the available stage size and contain-fits the rotated frame with one shared
width/height scale, including when landscape width is the limiting dimension.
Touch coordinates are normalized in that exact displayed space.

### Update check fails

- Confirm the nightly release contains `latest.json`, the updater artifact, and
  matching `.sig` file. Current Tauri 2 Windows and Linux updates use the NSIS
  installer and AppImage directly; macOS uses the app archive.
- Confirm the public key in `src-tauri/tauri.conf.json` matches the private CI key.
- Verify that the installed version is lower than the `version` in
  `latest.json`.

## Roadmap

- Add confirmed install and removal actions to the read-only Misagent
  provisioning profile inspector
- Expand Device Hub-style controls for location, appearance, and accessibility
  when stable `idevice` service APIs are available
- Continue profiling decode, frame transport, and WebView presentation on
  Windows, with platform-specific metrics kept visible in the device workspace
- Close remaining scrcpy-mask mapping editor and runtime compatibility gaps

## Credits

The mapping interaction model is inspired by `scrcpy-mask`, especially its live
overlay, direction pad, key capture, and inspector structure. Android transport
code is not used.
