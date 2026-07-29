# Architecture

[简体中文](../zh-CN/architecture.md) | [Documentation](README.md)

DeviceHub Mask is one product with two native hosts: a Tauri 2 desktop application and a headless browser service. They share the React UI, domain model, Apple-device runtime, native adapters, and authenticated server graph. A host composes these pieces; it does not reimplement them.

## System Shape

```text
                 React UI (src)
                  HTTP / WebSocket
                         |
              devicehub-server
           HTTP + WS + MCP + SPA
                         |
                 RuntimeClient
                         |
              devicehub-runtime
       discovery + multi-device sessions
          Apple services + media + HID
                         |
                 devicehub-core
          bounded values + domain policy

Tauri host ---------------------------- Headless host
src-tauri                               devicehub-headless
desktop policy                         CLI/listener/LAN policy
           \                           /
                  devicehub-host
        files + FFmpeg + netmuxd adapters
```

The dependency direction is intentional. Core knows no runtime, transport, network, or UI framework. Runtime knows Apple-device behavior but no HTTP, Tauri, or host process discovery. Server knows wire protocols but neither starts a runtime nor binds a production listener. Composition roots own lifecycle and exposure policy.

## Layer Responsibilities

| Layer | Responsibility |
| --- | --- |
| `devicehub-core` | Normalized DTOs, validation, state slots, bounded policy, input/domain values |
| `devicehub-runtime` | Discovery, trust, per-device sessions, Apple protocols, service supervision, media/input, reconnect and cleanup |
| `devicehub-server` | Authenticated private HTTP, WebSocket media/control, MCP, SPA routing and wire validation |
| `devicehub-host` | Shared native filesystem, transfer, FFmpeg, netmuxd, pairing-storage and asset adapters |
| `devicehub-headless` | CLI configuration, data paths, token policy, listener and optional LAN/MCP exposure |
| `src-tauri` | Desktop process lifecycle, private loopback listener, native audio, clipboard, windows, dialogs, updater and Tauri permissions |
| `src` | Shared React workspaces, browser video/audio, input scheduling and host-capability presentation |

See [Core and Runtime Boundaries](core-runtime.md) for enforceable dependency rules and module ownership.

## Host Composition

Both hosts create one `RuntimeClient`, inject `devicehub-host` capabilities, and pass narrow clients to `devicehub-server`. The server graph is reusable and listener-free.

The desktop binds a random loopback private API used by its WebView and separately exposes loopback MCP. Its Tauri shell owns native audio output, clipboard integration, file dialogs, window state, update installation, and desktop-only permissions.

The headless binary serves the same built frontend and API. It defaults to `127.0.0.1:8080`; non-loopback binding requires `--allow-lan`. Browser clients authenticate with an access token bootstrapped through the URL fragment. Headless and desktop must not drift into separate endpoint implementations.

## Multi-Device Runtime

`devicehub-runtime` owns one dedicated device thread, its Tokio runtime and `LocalSet`, shared discovery/trust coordination, and a registry of isolated device sessions. Each selection ID has its own phase, errors, commands, media state, service workers, reconnect state, and observations.

Selecting a device changes UI focus only. It does not terminate another connected session. Explicit disconnect, reconnect, pairing, and trust removal are target-scoped. USB and Wi-Fi entries for one physical UDID remain distinct discovery choices, while the runtime prevents competing active transports for the same physical device.

The host facade is split conceptually into manager and session capabilities:

- `RuntimeManagerClient` controls discovery inventory, selection, pairing, trust and manager lifecycle.
- `DeviceSessionRegistry` resolves a `DeviceSessionClient` by exact selection ID.
- `DeviceSessionClient` exposes only one session's observations, media, input and operations.

Private HTTP uses `X-DeviceHub-Device`; WebSocket uses `device_id`; each MCP connection holds its own target. Unknown or omitted targets are rejected where ambiguity would otherwise select the wrong phone.

## Resource Governance

Video, audio, performance sampling, and device-log streaming are independent per-session demands. A connected but invisible device remains available without paying the full active-device cost.

- With no video consumer, RTP/RTCP remains drained and observable, while access-unit publication is skipped. Resuming clears stale state and requests a keyframe.
- Desktop audio decodes only the selected device. Headless audio decodes only while a browser client requests unmuted audio for that session.
- Performance and device logs start only while their workspaces or API consumers hold demand.
- Shutdown and session replacement release held HID input, consumers, sidecars, and supervised tasks with bounded cleanup.

## Media and Input Flow

Video follows one path: the runtime receives HEVC RTP, assembles complete Annex-B access units, applies bounded presentation credits, and publishes them through WebSocket. The browser configures WebCodecs, waits for a keyframe after resynchronization, decodes, and renders the device frame. FFmpeg is not a video decoder in the current architecture.

Audio RTP contains AAC-ELD. A host-provided FFmpeg sidecar decodes it to 48 kHz stereo PCM. Tauri sends PCM to native output; headless sends bounded audio frames to authenticated browser clients. Browser audio over LAN is subject to browser autoplay and secure-context policy.

Pointer, mapping, keyboard passthrough, and MCP input normalize into core input values. Runtime validates bounds and contact ownership, translates them to Universal HID reports, and serializes dispatch per device. A control lease and cleanup on blur, mode change, disconnect, or client loss prevent stuck contacts and buttons.

## Service and Failure Model

Each device service reports a normalized health phase and is supervised independently where recovery is safe. A location, logging, diagnostics, or performance channel failure should not tear down video and input. Transport-ending failures transition only the affected session and use bounded reconnect policy. Error projections retain enough target and operation context for a user or agent to act without exposing raw unbounded protocol data.

Long operations such as captures, backups, diagnostics, file transfers, and console streams have explicit limits, cancellation where supported, and session cleanup. Host files are accessed through injected capabilities so runtime never trusts or resolves local paths itself.

## Data Ownership

- Runtime observations are bounded in-memory slots and event streams.
- User preferences and key-mapping profiles are host-persisted through explicit repositories.
- Headless data lives under its configured data directory; desktop data uses platform application directories.
- Captures, backups, logs, crash reports and container transfers remain outside the WebView unless a bounded endpoint explicitly returns normalized content.
- Raw XPC, plist, CoreDevice clients, OS paths, and child-process handles never cross public domain APIs.

## Security Boundaries

Desktop API exposure is private loopback plus per-run authentication. Headless LAN exposure is explicit and token authenticated, but it has no built-in TLS, accounts, roles, Origin policy, rate limiting, or revocable sessions. It is suitable only for a trusted LAN and must not be published directly to the Internet. MCP has no authentication and should remain loopback-only.

Application installation, sideloading, signing, injection, and upgrading are outside the product boundary. DeviceHub Mask may inspect and manage existing apps and provisioning profiles, but profile management must never become a hidden installation route.

## Extending the System

Add domain policy to core, Apple execution to runtime, wire representation to server, OS capability to host, and lifecycle/exposure decisions to a composition root. Keep device identity explicit and consider all connected sessions. New expensive producers require demand gating and observable health. New long operations require bounds and cleanup. New UI must work in both hosts or clearly declare a host capability. Update the authoritative document and run the validation defined in [Development and Builds](development.md).
