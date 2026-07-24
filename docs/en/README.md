# DeviceHub Mask Documentation

[简体中文](../zh-CN/README.md) | English | [Project README](../../README.md)

Use this documentation by task rather than reading it in order.

## Guides

- [Getting Started](getting-started.md): platform prerequisites, source setup,
  device preparation, and the first development run
- [User Guide](user-guide.md): device, mapping, application, keyboard, hardware
  button, screenshot, localization, and update workflows
- [Feature Reference](features.md): current desktop workspaces, device-management
  operations, idevice service coverage, MCP tools, and intentional boundaries
- [Architecture](architecture.md): process boundaries, private transport,
  CoreDevice sessions, video pipeline, HID validation, and data ownership
- [Development](development.md): repository layout, environment variables,
  validation, local production builds, and platform packaging
- [Distribution](distribution.md): GitHub Actions, nightly artifacts, updater
  signing, Apple signing, and release versioning
- [Troubleshooting](troubleshooting.md): blank windows, FFmpeg, Windows device
  preparation, CoreDevice errors, touch coordinates, and updater failures

## Support Matrix

| Area | macOS | Windows | Linux |
| --- | --- | --- | --- |
| Tauri desktop UI | Supported | Supported | Supported |
| CoreDevice USB display | Primary development platform | Supported with device preparation | Depends on host pairing/usbmuxd setup |
| Universal HID control | Supported when advertised by the device | Supported when advertised by the device | Depends on CoreDevice availability |
| CI packages | Universal DMG | x64 NSIS and MSI | x64 AppImage and DEB |
| In-app updates | Signed app archive | Signed NSIS installer | Signed AppImage |

Apple controls CoreDevice capability availability. A successful USB pairing does
not guarantee that a given hardware and iOS combination advertises remote
display or Universal HID services.

## Documentation Conventions

- Commands are run from the repository root unless a page says otherwise.
- `nightly` means the rolling release generated from updates to `main`.
- Paths and service identifiers are intentionally left untranslated.
- When changing behavior, update the matching page in both `docs/en` and
  `docs/zh-CN`.
