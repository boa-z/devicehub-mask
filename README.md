# DeviceHub Mask

[简体中文](README.zh-CN.md) | English

DeviceHub Mask controls and inspects Developer Mode iOS devices from macOS, Windows, and Linux. The same React application runs in a Tauri 2 desktop host or an experimental headless service, backed by a shared multi-device Rust runtime. It provides CoreDevice HEVC streaming through WebCodecs, Universal HID input, key mapping, device/app/file/diagnostic workspaces, and MCP automation.

## Product Surfaces

| Surface | Purpose |
| --- | --- |
| Tauri desktop | Native daily-use application with desktop audio, clipboard, dialogs, updates, and private loopback services |
| Headless service | Browser UI and authenticated API on loopback or an explicitly enabled trusted LAN |
| MCP | Loopback agent interface for target selection, screenshots, HID, app workflows, waits and diagnostics |

The runtime can keep multiple devices connected. UI selection changes focus without destroying other sessions, and API/MCP clients resolve explicit device targets.

DeviceHub Mask deliberately does not install, sideload, sign, inject, or upgrade iOS applications. This boundary applies to future feature work; use a dedicated signing and deployment tool before managing an app here.

## Quick Start

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

## Documentation

| Audience | Start here |
| --- | --- |
| Desktop user | [Documentation home](docs/en/README.md), then [User Guide](docs/en/user-guide.md) |
| Headless/LAN operator | [Headless Service](docs/en/headless.md) |
| Agent user | [MCP Automation Guide](docs/en/mcp.md) |
| Developer | [Architecture](docs/en/architecture.md) and [Development and Builds](docs/en/development.md) |

Complete English and Simplified Chinese documentation is available from the [documentation home](docs/en/README.md) and [中文文档首页](docs/zh-CN/README.md).

## Status and Security

The project is in active early development. CoreDevice services are private Apple capabilities and vary by iOS, hardware, transport, host preparation, and policy. Pairing does not guarantee display, HID, diagnostics, or every management service.

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
