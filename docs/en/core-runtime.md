# Core and Runtime Boundaries

[简体中文](../zh-CN/core-runtime.md) | [Documentation](README.md)

The core/runtime extraction is complete. This page defines the current Rust boundaries; it is not a migration plan. `scripts/check-rust-boundaries.mjs` enforces the most important dependency and source rules.

## Dependency Direction

```text
devicehub-headless ----+--> devicehub-server ----+
                      |                          |
src-tauri -------------+--> devicehub-host ------+--> devicehub-runtime --> devicehub-core
```

Hosts may depend on every lower capability they compose. Server and host adapters may depend on runtime/core. Runtime depends on core. Reverse dependencies are forbidden.

## `devicehub-core`

Core owns stable, normalized, bounded domain behavior: device and application values, input commands, key-mapping validation, storage path policy, diagnostics/capture state, performance observations, service health, location, provisioning metadata, and reusable state slots.

Core must not depend on async/device/web/desktop/process frameworks, including `tokio`, `idevice`, `axum`, `rmcp`, `rodio`, Tauri, `wry`, or FFmpeg. It does not own host paths, raw plist/XPC values, protocol clients, retry loops, or task spawning. Core should contain real validation and transition policy, not only marker traits or transport-shaped DTOs.

## `devicehub-runtime`

Runtime is the only Apple-device execution layer. It owns discovery, pairing/trust coordination, the concurrent session registry, non-`Send` clients, command queues, Apple service translation, media negotiation/publication, Universal HID dispatch, demand leases, supervision, reconnect, and deterministic session cleanup.

Its public surface consists of typed clients, commands, bounded observations, and host capability ports. Raw protocol clients and transport handles remain private. Runtime may use `idevice`, Tokio, serialization, and platform-neutral networking, but not Axum, MCP, Tauri, `rodio`, `wry`, or desktop frontend assets. Production runtime code must not read process environment, resolve executables, launch FFmpeg/netmuxd, or choose host directories.

The important public split is:

- `RuntimeManagerClient`: inventory, frontend selection, pairing/trust and manager operations.
- `DeviceSessionRegistry`: exact session lookup.
- `DeviceSessionClient`: one target's observations, commands, media and demand.
- Host ports: clipboard, files, capture destinations, developer images, provisioning data, backups, diagnostic sinks, audio pipeline and sidecars.

## `devicehub-server`

Server owns bounded wire adapters: authenticated private HTTP, status projection, WebSocket video/audio/control, MCP handlers, SPA delivery, and API error mapping. It receives existing runtime clients, repositories and explicit configuration.

It must not own a listener, parse host environment, start a device runtime, open an Apple service, or use Tauri/desktop audio. Manager routes receive manager capability only; device routes resolve a target session; file/profile routes receive narrow host repositories. Adding an endpoint is not permission to reach into runtime internals.

## `devicehub-host`

Host contains reusable native implementations shared by desktop and headless: confined filesystem access, profile persistence, browser transfers, capture/diagnostic destinations, Developer Image/provisioning assets, backups, FFmpeg audio decoding, netmuxd, and Wi-Fi pairing storage.

It remains headless-compatible. It cannot depend on Tauri, `wry`, desktop clipboard/audio libraries, listener policy, or product UI. It implements runtime/server ports but does not decide device policy.

## Composition Roots

`devicehub-headless` owns CLI parsing, data/frontend directory resolution, token files, listener addresses, explicit LAN permission, optional MCP binding, signal handling, and process shutdown.

`src-tauri` owns the desktop process, loopback listener lifecycle, Tauri state/permissions, window/updater/dialog behavior, native audio and clipboard, desktop settings, and application shutdown. Desktop-specific adapters remain here rather than leaking into shared crates.

Neither composition root may create a second device session manager or duplicate server routes.

## Multi-Device Contract

The registry key is a transport-aware selection ID. Operations resolve that ID before accessing a session. Switching UI focus does not destroy sessions. Duplicate USB/Wi-Fi activity for one physical UDID is rejected. Disconnect and recovery affect only the target. MCP connections can select independent devices, and HTTP/WebSocket clients carry explicit device scope.

Demand leases for video, audio, performance and logs are session-scoped. A consumer must release its lease on disconnect or failure. New background services need an owner, bounded restart policy, health reporting, and deterministic shutdown.

## Visibility and Module Shape

Use `foo.rs + foo/*.rs` to keep a domain's facade and related implementation together. Public exports live in the owning crate's `lib.rs` only when another crate needs them. Prefer private items, then `pub(crate)`, then a narrow `pub` API. Do not use wildcard re-exports or generic shared modules to make an ownership violation compile.

## Acceptance Checks

Run `npm run rust:boundaries` whenever dependencies, imports, visibility, environment reads, process launch, FFmpeg resolution, listeners, or crate ownership changes. Follow with targeted tests and the normal `npm run verify` gate. A boundary change is acceptable only if both hosts still compose the same runtime/server behavior, multi-device scope remains explicit, cleanup remains bounded, and [Architecture](architecture.md) plus [Development and Builds](development.md) remain accurate.
