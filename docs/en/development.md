# Development and Builds

[简体中文](../zh-CN/development.md) | [Documentation](README.md)

## Repository Layout

```text
devicehub-mask/
├── .github/workflows/       # verification and nightly publishing
├── docs/en/                 # English documentation
├── docs/zh-CN/              # Simplified Chinese documentation
├── scripts/                 # device preparation and packaging helpers
├── src/                     # React application
├── src-tauri/
│   ├── capabilities/        # Tauri permissions
│   ├── icons/
│   ├── src/                 # Rust desktop backend
│   ├── Cargo.toml
│   └── tauri.conf.json
├── package.json
└── README.md
```

Generated `dist/` and Cargo `target/` directories are not source documentation.

## Development Mode

```sh
npm ci
npm run tauri:dev
```

Development artifacts use `target/tauri-dev` and load Vite from
`http://127.0.0.1:5173`. Do not run that executable after Vite exits. Standalone
debug and production builds embed frontend assets through the Tauri protocol.

## Environment Variables

| Variable | Default | Purpose |
| --- | --- | --- |
| `DEVICEHUB_ADDR` | `127.0.0.1:0` | Private backend address; port `0` selects a random port |
| `DEVICEHUB_MCP_ADDR` | `127.0.0.1:8009` | Streamable HTTP MCP bind address; endpoint path is `/mcp` |
| `DEVICEHUB_PROFILE_DIR` | Tauri application data directory | Mapping profile storage |
| `DEVICEHUB_FFMPEG` | Auto-detected | Absolute FFmpeg executable path |
| `DEVICEHUB_VIDEO_MAX_DIMENSION` | `1920` on Windows; native elsewhere | Maximum decoded width or height; preserves aspect ratio and never upscales; `0` disables the limit |
| `DEVICEHUB_VIDEO_PIXEL_FORMAT` | Settings value | Override the app's video pixel-format setting with `rgb24` or experimental `yuv420p` |
| `DEVICEHUB_VIDEO_IN_FLIGHT_FRAMES` | `2` | Diagnostic A/B override for the bounded WebView frame pipeline; accepts `1` or `2` |
| `DEVICEHUB_LOG` | DeviceHub info logging | Preferred Rust tracing filter; overrides `RUST_LOG` |
| `RUST_LOG` | DeviceHub info logging | Standard tracing filter fallback |
| `DEVICEHUB_HID_DUMP` | Unset | Export the Universal HID service plist for protocol diagnostics |

Keep `DEVICEHUB_ADDR` on a loopback address. Changing it does not remove token
authentication, but external binding is outside the supported desktop model.

The MCP endpoint has no authentication and must remain on loopback unless the
host is on a trusted, isolated network. A non-loopback bind emits a warning. An
MCP bind failure is non-fatal and does not stop the desktop backend or session.

Runtime logs are written as JSON Lines to the platform application log
directory, rotate daily, and retain seven files. The active filter, run ID,
dropped-line count, Debug switch, and an action to open the directory are in
Settings > Diagnostics. The Debug switch affects only the current run. Set an
explicit filter when narrower trace logging is required, for example:

```sh
DEVICEHUB_LOG=devicehub_mask=info,devicehub_mask::session=trace npm run tauri:dev
```

An environment filter takes precedence over the Settings switch. Invalid
filters are rejected and the application falls back to the default filter.

Settings > Video exposes RGB24 and the experimental YUV420P path. RGB24 remains
the default. The selection is persisted in the platform application config
directory and applies on the next device connection. An explicit
`DEVICEHUB_VIDEO_PIXEL_FORMAT` value makes the setting read-only for that run.

The same section exposes the default, experimental Browser / WebCodecs decoder.
It sends complete Annex-B HEVC access units directly to the WebView and removes FFmpeg,
raw-frame transport, and JPEG encoding from the live video path. A WebCodecs
capability, output timeout, or runtime failure is reported in Settings and
automatically reconnects the current device with the native decoder for the rest
of the run.

## Validation

Run the source gates before committing:

```sh
npm run verify
```

This is the same cross-platform source gate used by GitHub Actions:
documentation, frontend lint/tests/build, Rust formatting/tests, and Clippy with
warnings denied. Run the full local gate before pushing a substantial change:

```sh
npm run verify:full
```

The full gate additionally builds the standalone debug application without
launching, bundling, or installing it. On macOS and Linux, release-script syntax
can be checked separately with
`bash -n scripts/package-dmg.sh scripts/generate-update-manifest.sh`.

The multitouch production path has been tested with a two-contact report on an
iPhone 13 Pro Max. Cross-platform CI verifies compilation but cannot replace
physical device testing.

## Localization

Translation resources are in `src/locales/en-US.ts` and
`src/locales/zh-CN.ts`. Add each UI key to both files and use
`useTranslation()` in components. `src/i18n.test.ts` enforces matching resource
trees.

Protocol identifiers, key codes, profile names, and user-authored labels remain
untranslated. New default labels are localized only when a profile is created.
The shared `--system-font` token is defined in `src/styles.css` and passed to Ant
Design by `src/AppProviders.tsx`; do not add remote or bundled fonts.

Documentation changes should preserve matching page names and navigation in
`docs/en` and `docs/zh-CN`. `npm run docs:check` verifies page parity and local
Markdown links; CI runs it on macOS, Windows, and Linux.

## Production Builds

Build all bundles configured for the current host:

```sh
npm run tauri:build
```

This command first downloads checksum-pinned netmuxd and LGPL FFmpeg sidecars
for the current host. Sidecar executables are generated under
`src-tauri/resources` and remain ignored by Git. Packaged applications prefer
the bundled FFmpeg; `DEVICEHUB_FFMPEG` remains an explicit override for testing.
An existing FFmpeg is reused only after its architecture and required capabilities
pass validation; use `npm run ffmpeg:prepare -- --force` to rebuild it explicitly.

Pass explicit Tauri build flags after `--` when needed:

```sh
npm run tauri:build -- --bundles app
```

Typical macOS outputs are the release executable, `.app`, and DMG below
`src-tauri/target/release`. Names vary by architecture and Tauri version.

### Windows

```powershell
npm run tauri:build
```

NSIS and MSI packages are written under
`src-tauri/target/release/bundle/nsis` and `bundle/msi`. FFmpeg is bundled and
starts without a console window. Apple Mobile Device Service remains a runtime
prerequisite.

### Linux

After installing the packages from [Getting Started](getting-started.md):

```sh
npm run tauri:build -- --bundles appimage,deb
```

Outputs are under `bundle/appimage` and `bundle/deb`.

### Universal macOS

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
npm run tauri:build -- --target universal-apple-darwin --bundles app
```

The build wrapper derives the sidecar platform from `--target` and builds an
LGPL-only universal FFmpeg executable from the checksum-pinned upstream source.
Windows and Linux preparation downloads pinned LGPL static builds and verifies
their SHA-256 hashes. `THIRD_PARTY_NOTICES.txt` and the complete FFmpeg license
are included beside the binary.

Artifacts are written under
`src-tauri/target/universal-apple-darwin/release/bundle/macos`.

### Reproducible DMG

Use the same helper as CI to stamp an existing app and generate a checksum:

```sh
APP_VERSION=0.1.0 \
BUILD_NUMBER=1 \
APP_PATH="src-tauri/target/release/bundle/macos/DeviceHub Mask.app" \
  scripts/package-dmg.sh
```

This produces `dist/devicehub-mask_0.1.0+1.dmg` and its SHA-256 file.

Release automation is described in [Distribution](distribution.md).
