# Development and Builds

[简体中文](../zh-CN/development.md) | [Documentation](README.md)

Read [Architecture](architecture.md) and [Core and Runtime Boundaries](core-runtime.md) before changing ownership or runtime behavior. This page is the command and build reference.

## Repository Layout

```text
devicehub-mask/
├── .github/workflows/       # verification and nightly publishing
├── docs/en/                 # English documentation
├── docs/zh-CN/              # Simplified Chinese documentation
├── crates/
│   ├── devicehub-core/      # host-independent domain policy and state
│   ├── devicehub-headless/  # standalone browser host binary
│   ├── devicehub-host/      # shared filesystem and process adapters
│   ├── devicehub-keymap/    # shared deterministic mapping and script runtime
│   ├── devicehub-runtime/   # Apple-device sessions and supervision
│   └── devicehub-server/    # reusable HTTP/WebSocket protocol adapters
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

## Headless Development

Build the shared React UI, then start the standalone native host from the repository root:

```sh
npm run headless:dev -- --listen 127.0.0.1:8080
```

Open the URL printed by the process. Its API token is placed in the URL fragment, which browsers do not send in HTTP requests, and is removed from the address bar after bootstrap. Use `--token-file` when a stable token is required; the file must already exist and contain one URL-safe token of at least 24 characters. On Unix, set its mode to `0600`.

The listener defaults to loopback. A non-loopback `--listen` value is rejected unless `--allow-lan` is also present. This opt-in does not provide TLS, user accounts, or Internet-safe deployment. MCP is disabled unless `--mcp-listen` is provided and remains loopback-only because it has no authentication. Run `npm run headless:dev -- --help` for all host paths and transport overrides.

## Development Mode

```sh
npm ci
npm run tauri:dev
```

Development artifacts use `target/tauri-dev` and load Vite from `http://127.0.0.1:5173`. Do not run that executable after Vite exits. Standalone debug and production builds embed frontend assets through the Tauri protocol.

## Environment Variables

| Variable | Default | Purpose |
| --- | --- | --- |
| `DEVICEHUB_ADDR` | `127.0.0.1:0` | Private backend address; port `0` selects a random port |
| `DEVICEHUB_MCP_ADDR` | `127.0.0.1:8009` | Streamable HTTP MCP bind address; endpoint path is `/mcp` |
| `DEVICEHUB_PROFILE_DIR` | Tauri application data directory | Mapping profile storage |
| `DEVICEHUB_FFMPEG` | Auto-detected | Absolute FFmpeg executable path used by device audio decoding |
| `DEVICEHUB_VIDEO_IN_FLIGHT_FRAMES` | `8` | Diagnostic A/B override for the bounded WebView ingress pipeline; accepts `1` through `8` |
| `DEVICEHUB_LOG` | DeviceHub info logging | Preferred Rust tracing filter; overrides `RUST_LOG` |
| `RUST_LOG` | DeviceHub info logging | Standard tracing filter fallback |
| `DEVICEHUB_HID_DUMP` | Unset | Export the Universal HID service plist for protocol diagnostics |

Keep `DEVICEHUB_ADDR` on a loopback address. Changing it does not remove token authentication, but external binding is outside the supported desktop model.

The MCP endpoint has no authentication and must remain on loopback unless the host is on a trusted, isolated network. A non-loopback bind emits a warning. An MCP bind failure is non-fatal and does not stop the desktop backend or session. Client setup, tool workflows, and security boundaries are documented in the [MCP Automation Guide](mcp.md).

Runtime logs are written as JSON Lines to the platform application log directory, rotate daily, and retain seven files. The active filter, run ID, dropped-line count, Debug switch, and an action to open the directory are in Settings > Diagnostics. The Debug switch affects only the current run. Set an explicit filter when narrower trace logging is required, for example:

```sh
DEVICEHUB_LOG=devicehub_mask=info,devicehub_mask::session=trace npm run tauri:dev
```

An environment filter takes precedence over the Settings switch. Invalid filters are rejected and the application falls back to the default filter.

Live video always sends complete Annex-B HEVC access units to the WebView and decodes them with WebCodecs. FFmpeg video decoding, raw RGB/YUV transport, JPEG encoding, decoder selection, and pixel-format settings are no longer part of the application. FFmpeg remains required for AAC-ELD device audio.

## Validation

Run the source gates before committing:

```sh
npm run verify
```

The production frontend build also checks the Vite manifest against committed budgets for initial JavaScript, initial CSS, total JavaScript, and the largest asynchronous chunk. Run `npm run budget:check` to inspect an existing `dist/` build. Do not raise a budget to hide a regression; first reduce or split the dependency graph and document any intentional baseline change.

The total JavaScript baseline is 1,522,000 bytes after adding the lazy-loaded device activity center. Its UI, response validation, and adaptive polling remain outside the initial dependency graph. Initial JavaScript, initial CSS, and per-chunk limits remain unchanged, so optional workspace growth cannot hide a startup or chunk-size regression.

This is the same cross-platform source gate used by GitHub Actions: documentation, frontend lint/tests/build, Rust formatting/tests, and Clippy with warnings denied. Run the full local gate before pushing a substantial change:

```sh
npm run verify:full
```

The full gate additionally builds the standalone debug application without launching, bundling, or installing it. Neither command runs physical-device tests; those remain an explicit `npm run verify:device -- --udid <UDID>` workflow.

Local verification disables Cargo incremental compilation and uses one build job by default. This keeps repeated test, Clippy, feature, and profile combinations from accumulating very large incremental caches. Explicit `CARGO_INCREMENTAL` and `CARGO_BUILD_JOBS` values are respected when temporary overrides are needed. Before compilation, `verify` requires 8 GiB of free space and `verify:full` requires 12 GiB so a build fails with an actionable diagnostic instead of failing midway while writing artifacts. If generated Rust artifacts have accumulated, remove all workspace Cargo targets with:

```sh
npm run clean:rust
```

This runs Cargo's official cleanup operation for both the workspace target and the separate development/legacy target. It deletes only rebuildable Cargo output and does not remove source files or application data. On macOS and Linux, release-script syntax can be checked separately with `bash -n scripts/package-dmg.sh scripts/generate-update-manifest.sh`.

The multitouch production path has been tested with a two-contact report on an iPhone 13 Pro Max. Cross-platform CI verifies compilation but cannot replace physical device testing.

After runtime or transport changes, run the explicit-UDID read-only checks and complete the manual USB/Wi-Fi checklist in [Physical Device Regression](device-regression.md).

## Localization

Translation resources are in `src/locales/en-US.json` and `src/locales/zh-CN.json`. Crowdin treats `en-US.json` as the source file and downloads target locale files through `.github/workflows/crowdin.yml`; do not add Crowdin credentials to the repository. Add each new UI key to the source file and use `useTranslation()` in components. `npm run locales:check` and `src/i18n.test.ts` enforce matching resource trees and interpolation tokens.

### Crowdin Setup

This repository uses a Crowdin JSON project. `crowdin.yml` manages exactly one source file, `src/locales/en-US.json`, and maps target languages to `src/locales/%locale%.json`. Keep locale files as plain JSON objects; do not add exports, executable code, or comments to them.

For the initial bootstrap, create the project with English as the source language and add the target languages that the application is ready to ship. Upload `en-US.json` as the source, then use the target language's **Upload Translations** action for each existing locale file. Confirm that the import report has a non-zero `Imported` count and that the Crowdin editor contains the expected keys before downloading translations.

Configure `CROWDIN_PROJECT_ID` and `CROWDIN_PERSONAL_TOKEN` as repository-level GitHub Actions secrets. The token must never appear in `crowdin.yml`, source files, commits, issues, or pull requests. The normal workflow keeps `upload_translations: false`: after bootstrap, Crowdin is the source of truth for translations. It uploads source changes, downloads translated files, and opens a review PR. Do not merge a localization PR whose target files contain source-language text unless the Crowdin import report has been checked.

When adding a source string, add it only to `en-US.json`, preserve every interpolation token such as `{{name}}` or `{{count}}`, and keep protocol identifiers, bundle IDs, file paths, product names, and key codes unchanged. Review the generated target-file diff and run `npm run locales:check`, `npm run lint`, `npm test`, and `npm run build` before merging the PR. The frontend workflow runs these checks automatically for pull requests.

### Adding Languages

Crowdin files are not auto-discovered by the runtime. Adding a target language requires all of the following:

1. Add the target language in Crowdin and verify its locale code maps to `src/locales/<locale>.json`.
2. Add the locale code and its dynamic loader to `src/i18n.ts`, and update `normalizeLanguage()` when regional aliases need special handling.
3. Add the language to the selector in `src/components/SettingsPage.tsx`.
4. Add the corresponding Ant Design locale mapping in `src/AppProviders.tsx`.
5. Keep `npm run locales:check` and the key parity test passing, verify the language through the Settings UI, and confirm that the new locale is loaded as a separate chunk.

The application currently registers only `zh-CN` and `en-US`; a new file downloaded by Crowdin is not usable until these runtime registration points are updated. English is bundled as the fallback and target locales are loaded on demand. Recheck `npm run build` and the frontend budget after adding languages.

Protocol identifiers, key codes, profile names, and user-authored labels remain untranslated. New default labels are localized only when a profile is created. The shared `--system-font` token is defined in `src/styles.css` and passed to Ant Design by `src/AppProviders.tsx`; do not add remote or bundled fonts.

Documentation changes should preserve matching page names and navigation in `docs/en` and `docs/zh-CN`. `npm run docs:check` verifies page parity and local Markdown links; CI runs it on macOS, Windows, and Linux.

## Production Builds

Build all bundles configured for the current host:

```sh
npm run tauri:build
```

This command first downloads checksum-pinned netmuxd and LGPL FFmpeg sidecars for the current host. Windows and Linux use the `n8.1` LGPL assets from BtbN's rolling [`latest` Release](https://github.com/BtbN/FFmpeg-Builds/releases/tag/latest). The preparation script resolves the exact asset and GitHub SHA-256 digest through the Releases API, then falls back to the `latest/checksums.sha256` manifest if the API is unavailable. It never downloads the bare `releases/download/latest` path; a concrete asset filename is required. Set `DEVICEHUB_FFMPEG_BTB_TAG` to an immutable BtbN release tag when reproducing a specific build.

Desktop sidecars are generated under `src-tauri/resources` and remain ignored by Git. `ffmpeg-target.json` records the FFmpeg version and target triple; the preparation script reuses a sidecar only when that metadata matches. Direct preparation of a foreign target must use a target-specific staging directory, for example `node scripts/prepare-ffmpeg.mjs --target aarch64-unknown-linux-gnu --output-dir release-artifacts/sidecars/ffmpeg/aarch64-unknown-linux-gnu`. Headless packaging does this automatically, so it cannot replace the desktop host resource. Packaged applications prefer the bundled FFmpeg; `DEVICEHUB_FFMPEG` remains an explicit override for testing. Use `npm run ffmpeg:prepare -- --force` to rebuild the current host resource explicitly.

Pass explicit Tauri build flags after `--` when needed:

```sh
npm run tauri:build -- --bundles app
```

Typical macOS outputs are the release executable, `.app`, and DMG below `src-tauri/target/release`. Names vary by architecture and Tauri version.

### Windows

```powershell
npm run tauri:build
```

NSIS and MSI packages are written under `src-tauri/target/release/bundle/nsis` and `bundle/msi`. FFmpeg is bundled and starts without a console window. Apple Mobile Device Service remains a runtime prerequisite.

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

The build wrapper derives the sidecar platform from `--target` and builds an LGPL-only universal FFmpeg executable from the checksum-pinned upstream source. Windows and Linux preparation downloads the current `n8.1` LGPL static assets and verifies their SHA-256 hashes. `THIRD_PARTY_NOTICES.txt` and the complete FFmpeg license are included beside the binary.

Artifacts are written under `src-tauri/target/universal-apple-darwin/release/bundle/macos`.

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
