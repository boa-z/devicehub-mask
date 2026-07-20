# DeviceHub Mask

DeviceHub Mask is a Tauri 2 desktop workspace for controlling iOS games from
macOS. It combines a React mapping editor inspired by
[scrcpy-mask](https://github.com/AkiChase/scrcpy-mask) with the CoreDevice screen
streaming, orientation handling, and Universal HID support developed in
`devicehub_rs`.

The project does not use eframe, iPhone Mirroring, ADB, or scrcpy's Android
transport.

## Features

- Native macOS window powered by Tauri 2 and WKWebView
- Live iOS screen at up to 60 FPS over CoreDevice displayservice and RTP/HEVC
- Latest-frame 60 FPS delivery with TurboJPEG caching and decode backpressure
- Aspect-ratio-preserving portrait and landscape rendering
- Up to five typed Universal HID contacts in one report
- Concurrent keyboard mappings, WASD direction pad, and direct pointer gestures
- Mutually exclusive mapping and raw keyboard HID modes with safe key release
- Drag-to-place overlays and profile persistence
- Device, keyboard mapping, and application settings workspaces
- Runtime Simplified Chinese and English localization with persistent selection
- scrcpy-mask profile import/export for taps and button direction pads
- Always-visible hardware buttons with user-defined keyboard shortcuts
- Desktop controls for always-on-top, fullscreen, and inspector visibility
- Live decode, transmit, render, JPEG latency, and bandwidth diagnostics
- Signed in-app nightly updates with download progress and automatic restart

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

## Requirements

- macOS with Xcode Command Line Tools
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

## Get The Source

```sh
git clone https://github.com/boa-z/devicehub-mask.git
cd devicehub-mask
npm --prefix frontend ci
```

`npm ci` installs both the frontend dependencies and the repository-local Tauri
CLI. A global `cargo-tauri` installation is not required.

## Prepare A Device

1. Connect the iOS device over USB.
2. Unlock it and accept the macOS trust prompt.
3. Keep the device unlocked for the first connection.
4. Close other DeviceHub processes that may already own the CoreDevice media
   session.

The device selector can show `Wi-Fi` when usbmuxd reports a paired network path;
this does not mean iPhone Mirroring is being used. When usbmuxd exposes both USB
and Wi-Fi records for the same UDID, DeviceHub Mask prefers Wi-Fi because current
iOS releases allow the remote-control media negotiation over that path more
consistently. USB remains the fallback when no paired network path is available.

## Development

Start Vite, Tauri, the local stream service, and automatic Rust/frontend reload:

```sh
npm --prefix frontend run tauri:dev
```

To request a specific device at startup, pass its UDID after `--`:

```sh
npm --prefix frontend run tauri:dev -- -- 00008110-001624E2013A801E
```

The development frontend runs at `http://127.0.0.1:5173` inside the Tauri
WebView. It obtains the authenticated random backend endpoint through Tauri IPC;
Vite does not proxy or expose the device API. The packaged application loads
assets from the Tauri protocol instead.

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
npm --prefix frontend run tauri:build
```

Or invoke the local CLI directly when build flags are needed:

```sh
frontend/node_modules/.bin/tauri build --bundles app
```

The main outputs are:

```text
target/release/devicehub-mask
target/release/bundle/macos/DeviceHub Mask.app
target/release/bundle/dmg/DeviceHub Mask_0.1.0_aarch64.dmg
```

Output names can vary slightly by Tauri CLI version and host architecture.

### Universal macOS Build

Install both targets and request a universal binary:

```sh
brew install nasm
rustup target add aarch64-apple-darwin x86_64-apple-darwin
frontend/node_modules/.bin/tauri build \
  --target universal-apple-darwin \
  --bundles app
```

Universal artifacts are written below:

```text
target/universal-apple-darwin/release/bundle/macos/
```

### Reproducible DMG Packaging

The same helper used by CI can stamp an existing `.app` and create a DMG:

```sh
APP_VERSION=0.1.0 \
BUILD_NUMBER=1 \
APP_PATH="target/release/bundle/macos/DeviceHub Mask.app" \
  scripts/package-dmg.sh
```

The output is `dist/devicehub-mask_0.1.0+1.dmg` plus its SHA-256 checksum.

## Validation

Run all source checks before committing:

```sh
npm --prefix frontend run lint
npm --prefix frontend test
npm --prefix frontend run build
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
frontend/node_modules/.bin/tauri build --debug --no-bundle
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
frontend/src/locales/en-US.ts
frontend/src/locales/zh-CN.ts
```

To add or change UI text, add the same key to both resources and use
`useTranslation()` in the component. `frontend/src/i18n.test.ts` verifies that
both resource trees contain the same keys. Protocol identifiers, key codes,
profile names, and user-authored mapping labels are deliberately not translated.
New default mapping labels are localized when a profile is first created;
switching languages never rewrites an existing profile.

The shared system font stack is declared as `--system-font` in
`frontend/src/styles.css` and passed to Ant Design from
`frontend/src/AppProviders.tsx`. Keep typography on this stack rather than
adding remote or bundled fonts.

### Workspace Pages

- **Device** contains device selection, the aspect-ratio-preserving live view,
  control mode, rotation, direct touch, and hardware buttons.
- **Keyboard Mapping** contains the live placement canvas, mapping inspector,
  hardware shortcut bindings, and profile management.
- **Application Settings** contains window state and update controls.

Changing pages releases every active touch, hardware button, and keyboard usage.

### Profile Management And scrcpy-mask Compatibility

The keyboard mapping page follows scrcpy-mask's profile workflow: list profiles,
select the displayed profile, mark one profile active, create, duplicate, rename,
delete, import, and export. The active profile cannot be deleted. Profiles are
stored as validated JSON files in the DeviceHub Mask application data directory.

DeviceHub Mask can import scrcpy-mask `0.0.1` JSON files and export the current
profile back to that format. `SingleTap` and button-based `DirectionPad` mappings
are converted bidirectionally, including pixel-to-normalized coordinates, Bevy
keyboard codes, and contact identities. Chord bindings and Android-specific
mapping types such as Swipe, CastSpell, FPS, Fire, RawInput, and Script are
reported as skipped because the current iOS mapping model has no behaviorally
equivalent representation.

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

[`.github/workflows/nightly.yml`](.github/workflows/nightly.yml) contains two
jobs and does not use GitHub Environments or create Deployment records:

- `verify` runs for every push and pull request. It checks localization and
  frontend tests, lint, production assets, Rust formatting/tests/clippy, and
  release-script syntax, then builds the debug desktop application without
  access to signing secrets.
- `build-macos` runs only for pushes to `main` and manual dispatches. It builds
  a Universal macOS application and rolling nightly
  release. The packaging step signs the complete bundle, verifies its sealed
  resources, and checks that both macOS architectures are present. Workflow
  artifacts are retained for 14 days.

Nightly artifacts are published at:

<https://github.com/boa-z/devicehub-mask/releases/tag/nightly>

Each release can contain:

```text
devicehub-mask_<base-version>+<build>.dmg
devicehub-mask_<base-version>+<build>.dmg.sha256
devicehub-mask_<base-version>-<build>_universal.app.tar.gz
devicehub-mask_<base-version>-<build>_universal.app.tar.gz.sig
latest.json
```

The workflow run number becomes `CFBundleVersion`. Nightly updater versions use
`major.minor.<run-number>` because SemVer build metadata such as `0.1.0+12` does
not participate in update ordering.

## Configure App Updates

Tauri updater packages are cryptographically signed independently of Apple code
signing. The committed updater public key is in `tauri.conf.json`; its private
key must never be committed.

Generate a replacement keypair only before publishing the first compatible
release:

```sh
mkdir -p .tauri
frontend/node_modules/.bin/tauri signer generate \
  --write-keys .tauri/devicehub-mask.key
```

Then update `plugins.updater.pubkey` in `tauri.conf.json` and configure these
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

If the private-key secret is absent, CI still publishes a DMG but deliberately
skips updater archives and `latest.json`. Losing or replacing the private key
prevents existing installations from accepting future updates.

At runtime the app checks the nightly endpoint shortly after startup. The
toolbar download button also performs a manual check. An accepted update is
downloaded, verified against the public key, installed, and followed by an app
restart.

## Apple Signing And Notarization

Updater signatures do not replace Apple Developer ID signing. Current nightly
applications receive a structurally valid ad-hoc signature after Universal
assembly and version stamping, so every executable slice and sealed resource can
be verified. Because ad-hoc signing does not establish publisher identity,
Gatekeeper may still require explicit local approval. Production distribution
should configure a Developer ID Application certificate, notarize the DMG, and
staple the notarization ticket.

## Troubleshooting

### Private backend does not start

The default random loopback port avoids normal port conflicts. Stop stale
`devicehub-mask`, `devicehub_rs`, and ffmpeg processes if they still own the
CoreDevice session. `DEVICEHUB_ADDR` is intended for diagnostics; keep it bound
to a loopback address. The API remains token-authenticated and has no web root.

### No screen frames

- Install ffmpeg with `brew install ffmpeg`. Packaged applications do not inherit
  the shell's `PATH`, so DeviceHub Mask also checks `/opt/homebrew/bin/ffmpeg`,
  `/usr/local/bin/ffmpeg`, and `/opt/local/bin/ffmpeg` directly.
- For a custom installation, launch with
  `DEVICEHUB_FFMPEG=/absolute/path/to/ffmpeg` or define that variable for the
  application process.
- Unlock and reconnect the iOS device.
- Close other CoreDevice display sessions.
- Check the status badge and Rust logs for displayservice or RSD failures.

### CoreDevice error 9021 requires a newer iOS version

The device can advertise separate USB and paired Wi-Fi records with the same
UDID, so DeviceHub Mask chooses the Wi-Fi record deterministically when both
exist. Confirm the device menu says `Wi-Fi`, keep USB attached for pairing/trust
if needed, and enable **Show this iPhone when on Wi-Fi** in Finder. This transport
is usbmuxd/CoreDevice, not iPhone Mirroring.

If error 9021 is returned even over Wi-Fi, the device itself rejected the
remote-control capability. Support depends on the hardware and iOS combination:
this does not mean every device below iOS 27 is unsupported, but the rejected
device must be updated to iOS 27 or replaced with a supported newer model. USB,
re-pairing, ffmpeg, app signing, and repeated retries cannot bypass this check.
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

- Confirm the nightly release contains `latest.json`, the updater archive, and
  matching `.sig` file.
- Confirm the public key in `tauri.conf.json` matches the private CI key.
- Verify that the installed version is lower than the `version` in
  `latest.json`.

## Credits

The mapping interaction model is inspired by `scrcpy-mask`, especially its live
overlay, direction pad, key capture, and inspector structure. Android transport
code is not used.
