# DeviceHub Mask Documentation

[简体中文](../zh-CN/README.md) | English | [Project README](../../README.md)

Choose a path by what you need to do. Each detailed subject has one authoritative page; other pages link to it instead of repeating it.

## Use DeviceHub Mask

| Goal | Read |
| --- | --- |
| Install prerequisites and connect a first device | [Getting Started](getting-started.md) |
| Operate the desktop workspaces | [User Guide](user-guide.md) |
| Create, import, and bind key mappings | [Key Mapping Guide](key-mapping.md) |
| Check whether a capability exists | [Feature Reference](features.md) |
| Recover from a connection, media, or platform failure | [Troubleshooting](troubleshooting.md) |

## Run Services and Automation

| Goal | Read |
| --- | --- |
| Run the browser UI locally or on a LAN | [Headless Service](headless.md) |
| Let an agent control and inspect a device | [MCP Automation Guide](mcp.md) |

## Develop and Release

| Goal | Read |
| --- | --- |
| Understand process, crate, runtime, and data-flow boundaries | [Architecture](architecture.md) |
| Decide whether code belongs in core, runtime, server, host, or a composition root | [Core and Runtime Boundaries](core-runtime.md) |
| Set up the repository, validate changes, and build locally | [Development and Builds](development.md) |
| Run explicit hardware regression checks | [Physical Device Regression](device-regression.md) |
| Build CI artifacts, publish releases, or configure updates | [CI, Releases, and Updates](distribution.md) |

## Source of Truth

| Question | Authoritative page |
| --- | --- |
| What is implemented and intentionally excluded? | [Feature Reference](features.md) |
| How should a user complete a workflow? | [User Guide](user-guide.md) and its task-specific guides |
| Which layer owns behavior? | [Architecture](architecture.md) |
| What are the enforced Rust dependency rules? | [Core and Runtime Boundaries](core-runtime.md) |

## Support Summary

| Area | macOS | Windows | Linux |
| --- | --- | --- | --- |
| Tauri desktop UI | Supported | Supported | Supported |
| CoreDevice USB display | Primary development platform | Supported after device preparation | Depends on host pairing/usbmuxd setup |
| Universal HID control | Device capability dependent | Device capability dependent | Device and host capability dependent |
| CI desktop packages | Universal DMG | x64 NSIS and MSI | x64 and ARM64 AppImage and DEB |
| Headless packages | Universal tar.gz | x64 zip | x64 and ARM64 tar.gz |

Apple controls CoreDevice service availability. Pairing alone does not guarantee remote display, HID, diagnostics, or every management service.

## Documentation Rules

Commands run from the repository root unless stated otherwise. `nightly` is the rolling build from `main`. Service names, paths, and identifiers remain untranslated. Behavior changes must update the matching English and Simplified Chinese pages, and `npm run docs:check` must pass.
