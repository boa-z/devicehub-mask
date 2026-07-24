# DeviceHub Mask

[简体中文](README.zh-CN.md) | English

DeviceHub Mask is a Tauri 2 desktop application for controlling iOS games from macOS, Windows, and Linux. It combines CoreDevice screen streaming and Universal HID control with a mapping editor inspired by [scrcpy-mask](https://github.com/AkiChase/scrcpy-mask).

The project uses neither iPhone Mirroring nor scrcpy's Android transport. Its private Axum service is bound to loopback, authenticated per launch, and is not exposed as a browser application.

## Highlights

- Live CoreDevice HEVC streaming at up to 60 FPS with portrait and landscape aspect ratios preserved
- Lossless native screenshots through a bounded CoreDevice, screenshotr, and final DVT Screenshot fallback chain
- Up to five concurrent Universal HID touch contacts, direct pointer gestures, keyboard passthrough, and configurable hardware-button shortcuts
- Complete scrcpy-mask `0.0.1` controller profile import/export and a visual mapping editor with live or captured-screen backgrounds
- Editable device identity, bounded model/architecture/color metadata, activation readiness and bounded battery health/temperature diagnostics, regular/system/App Clip browsing, running-state detection, launch, restart and stop controls, native app icons, device-aware IPA install/upgrade preflight, safe app removal, unified public AFC/App storage/crash-report browsing with privacy-bounded summaries, bounded file and directory management, hardware-button lock, confirmed device restart and shutdown, and provisioning profile inspection, validated installation and confirmed removal, plus cancellable CoreDevice sysdiagnose collection
- One-shot Unicode text paste and optional bidirectional text/image clipboard synchronization through the CoreDevice Pasteboard Service
- On-demand structured iPhone Unified Log console with level/context filtering, supervised SyslogRelay fallback, bounded buffering, copy and recovery status
- Explicit per-app launch console for developer and third-party apps, with bounded session-only stdout/stderr capture
- On-demand normalized iPhone CPU, bounded core and physical-memory capacity, top-process CPU, memory and relative energy rankings, a searchable DVT running-process inventory, Core Animation FPS, GPU-memory, and device network telemetry with supervised DVT service recovery, device-wide DVT network/thermal condition simulation, plus bounded all-device or per-process network PCAP and Bluetooth HCI PCAP export
- Read-only, bounded DVT network-interface inventory without IP or MAC addresses for connection diagnostics
- Built-in Streamable HTTP MCP server for screenshots, low-latency multi-touch, bounded WDA logical device state, element inspection/waiting and semantic typing/double-tap/hold/scroll actions, app lifecycle control, frame synchronization, device switching, DVT virtual location, bounded process/performance inspection, filtered device logs, and crash report diagnosis, with refreshed device details and event-driven app, storage, regional-setting, developer-image, name, activation, and lock waits
- Native Tauri 2 desktop controls, Simplified Chinese and English UI, and signed nightly updates
- macOS, Windows, and Linux verification and packaging through GitHub Actions

## Quick Start

Install Rust stable, Node.js 22 or newer, FFmpeg, and the native prerequisites for your platform. Then connect, unlock, and trust a Developer Mode-enabled iOS device.

```sh
git clone https://github.com/boa-z/devicehub-mask.git
cd devicehub-mask
npm ci
npm run tauri:dev
```

Windows also requires Apple Mobile Device Service, Visual Studio Build Tools, CMake, and NASM. Run the device preparation helper once before connecting:

```powershell
.\scripts\prepare-windows-device.ps1
```

See the [Getting Started guide](docs/en/getting-started.md) for complete platform-specific prerequisites and device preparation.

The app also exposes MCP on `http://127.0.0.1:8009/mcp` while it is running:

```sh
claude mcp add --transport http devicehub-mask http://127.0.0.1:8009/mcp
```

## Documentation

| Topic | English | 简体中文 |
| --- | --- | --- |
| Documentation home | [English docs](docs/en/README.md) | [中文文档](docs/zh-CN/README.md) |
| Installation and first run | [Getting Started](docs/en/getting-started.md) | [快速开始](docs/zh-CN/getting-started.md) |
| App workflows and controls | [User Guide](docs/en/user-guide.md) | [使用指南](docs/zh-CN/user-guide.md) |
| Implemented feature inventory | [Feature Reference](docs/en/features.md) | [功能参考](docs/zh-CN/features.md) |
| System design and protocols | [Architecture](docs/en/architecture.md) | [架构说明](docs/zh-CN/architecture.md) |
| Development and local builds | [Development](docs/en/development.md) | [开发与构建](docs/zh-CN/development.md) |
| CI, releases, and updates | [Distribution](docs/en/distribution.md) | [发布与更新](docs/zh-CN/distribution.md) |
| Common failures | [Troubleshooting](docs/en/troubleshooting.md) | [故障排查](docs/zh-CN/troubleshooting.md) |

## Project Status

The live screen, HID control, mapping, app management, and update paths are functional. Device and iOS support still depends on Apple's CoreDevice service availability. Current priorities are Windows video-pipeline profiling, broader Device Hub management capabilities, and closing remaining scrcpy-mask runtime compatibility gaps.

Nightly packages: [GitHub nightly release](https://github.com/boa-z/devicehub-mask/releases/tag/nightly)

## Validation

```sh
npm run lint
npm test
npm run build
cargo test --manifest-path src-tauri/Cargo.toml --locked
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --locked -- -D warnings
```

Detailed build and packaging checks are documented in [Development](docs/en/development.md).

## Credits

The mapping interaction model is inspired by scrcpy-mask, especially its live overlay, direction pad, key capture, and profile workflow. Android transport code is not used.
