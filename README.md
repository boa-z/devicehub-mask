# DeviceHub Mask

[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/boa-z/devicehub-mask)
[![LINUX DO](https://shorturl.at/ggSqS)](https://linux.do)

[简体中文](README.zh-CN.md) | English

DeviceHub Mask is an independent open-source project. It uses Apple's developer-device services but is not affiliated with Apple, Xcode, or Apple's Device Hub product.

DeviceHub Mask controls and inspects Developer Mode iOS devices from macOS, Windows, and Linux. Its runtime depends on the Rust [idevice](https://github.com/jkcoxson/idevice) library for the underlying device services and transport capabilities; the current device target requires iOS/iPadOS 27 or newer. The same React application runs in a Tauri 2 desktop host or an experimental headless service, backed by a shared multi-device Rust runtime. It provides CoreDevice HEVC streaming through WebCodecs, Universal HID input, key mapping, device/app/file/diagnostic workspaces, and MCP automation.

## Is This a Fit?

DeviceHub Mask is aimed at developers and device-lab operators who need to inspect, control, or automate one or more physical iPhone or iPad devices from a desktop, browser, or local agent. It complements Apple's development tools; it is not a replacement for Xcode and does not install, sideload, sign, inject, or upgrade iOS applications.

## Product Surfaces

| Surface | Purpose |
| --- | --- |
| Tauri desktop | Native daily-use application with desktop audio, clipboard, dialogs, updates, and private loopback services |
| Headless service | Browser UI and authenticated API on loopback or an explicitly enabled trusted LAN |
| MCP | Loopback agent interface for target selection, screenshots, HID, app workflows, waits and diagnostics |

The runtime can keep multiple devices connected. UI selection changes focus without destroying other sessions, and API/MCP clients resolve explicit device targets.

DeviceHub Mask deliberately does not install, sideload, sign, inject, or upgrade iOS applications. This boundary applies to future feature work; use a dedicated signing and deployment tool before managing an app here.

## Download a Package

The [nightly release](https://github.com/boa-z/devicehub-mask/releases/tag/nightly) provides ready-to-run packages:

- macOS Universal DMG
- Windows x64 NSIS and MSI installers
- Linux x64 and ARM64 AppImage and DEB packages
- macOS, Windows, and Linux headless archives

Nightly is a rolling early-development build. Verify the adjacent SHA-256 file before using an archive. macOS nightly packages currently use ad-hoc signing; see [Troubleshooting](docs/en/troubleshooting.md) if Gatekeeper blocks the app.

Choose [Getting Started](docs/en/getting-started.md) for package installation and device preparation.

## Build from Source

Install Rust stable, Node.js 22 or newer, FFmpeg, and the native prerequisites for your platform. Connect, unlock, trust, and enable Developer Mode on the iOS device.

```sh
git clone https://github.com/boa-z/devicehub-mask.git
cd devicehub-mask
npm ci
npm run tauri:dev
```

Windows also requires Apple Mobile Device Service, Visual Studio Build Tools, CMake, and NASM. Prepare it once with:

```powershell
.\scripts\prepare-windows-device.ps1
```

For headless development:

```sh
npm run headless:dev -- --listen 127.0.0.1:8080
```

See [Getting Started](docs/en/getting-started.md) for platform setup and [Headless Service](docs/en/headless.md) for LAN/token policy.

## First Use

After launch, open the connection center, choose an authenticated USB or Wi-Fi transport, and keep the device unlocked during the first session. Open the Device workspace to view the stream and use touch or hardware controls. For browser access on another host, use a Headless package and follow the token and LAN rules in [Headless Service](docs/en/headless.md).

## Related Projects

- [devicehub-mobile](https://github.com/boa-z/devicehub-mobile): React Native companion client for connecting to a DeviceHub Mask headless/LAN service.
- [devicehub-mask-keymaps](https://github.com/boa-z/devicehub-mask-keymaps): public catalog of downloadable key-mapping profiles.
- [idevice](https://github.com/boa-z/idevice): the Rust iOS service library used by the runtime.

## Documentation

| Audience | Start here |
| --- | --- |
| Desktop user | [Documentation home](docs/en/README.md), then [User Guide](docs/en/user-guide.md) |
| Headless/LAN operator | [Headless Service](docs/en/headless.md) |
| Agent user | [MCP Automation Guide](docs/en/mcp.md) |
| Developer | [Architecture](docs/en/architecture.md) and [Development and Builds](docs/en/development.md) |

Complete English and Simplified Chinese documentation is available from the [documentation home](docs/en/README.md) and [中文文档首页](docs/zh-CN/README.md).

## Status and Security

The project is in active early development. CoreDevice services are Apple capabilities and vary by iOS, hardware, transport, host preparation, and policy. Pairing does not guarantee display, HID, diagnostics, or every management service.

Desktop services stay on loopback. Headless LAN mode requires explicit enablement and token authentication, but provides no built-in TLS, accounts, roles, or Internet-safe perimeter. MCP has no authentication and should remain loopback-only.

Nightly packages: [GitHub nightly release](https://github.com/boa-z/devicehub-mask/releases/tag/nightly)

## Validation

Run the same source gate used by CI before committing:

```sh
npm run verify
```

It checks documentation, frontend lint/tests/build, Rust formatting/tests, Clippy, and crate boundaries without running physical-device tests. See [Development and Builds](docs/en/development.md) for targeted, full, packaging, and explicit device validation.

## Credits

The mapping interaction model is inspired by [scrcpy-mask](https://github.com/AkiChase/scrcpy-mask). Android transport code is not used.

## License

Copyright (c) 2026 boa-z. DeviceHub Mask is licensed under the [GNU Affero General Public License v3.0 only](LICENSE). Modified versions made available over a network must offer their corresponding source code under the same license.
