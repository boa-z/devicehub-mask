# Core And Runtime Extraction

[简体中文](../zh-CN/core-runtime.md) | [Documentation](README.md)

Status: accepted for incremental implementation.

## Decision

DeviceHub Mask remains one repository and separates its Rust backend into two host-independent libraries:

- `devicehub-core` defines stable domain data, validation, errors, events, and typed service handles.
- `devicehub-runtime` owns the concrete Apple-device implementation and implements the services defined by core.

Tauri, a future headless process, HTTP/WebSocket, and MCP are hosts or adapters around one runtime. They must not create independent device sessions or duplicate device policy. Extraction is incremental: first establish an internal `DeviceRuntime` boundary in the existing desktop crate, then create workspace crates after its lifecycle is covered by tests. Mechanical moves and behavior changes do not share a commit.

## Dependency Direction

```text
devicehub-desktop -----> devicehub-runtime -----> devicehub-core
devicehub-headless ----> devicehub-runtime -----> devicehub-core
devicehub-server --------------------------------> devicehub-core
devicehub-mcp -----------------------------------> devicehub-core
```

`devicehub-core` must not depend on `idevice`, Tauri, Axum, tower-http, rmcp, FFmpeg, rodio, React assets, native dialogs, or updater and window APIs. It owns normalized DTOs, bounded validation, business rules, control leases, stable errors, events, and service contracts. It must contain real policy rather than becoming an empty collection of traits.

`devicehub-runtime` may depend on core, `idevice`, Tokio, serialization, media helpers, and platform-neutral filesystem, networking, and process APIs. It must not depend on Tauri, Axum, rmcp, frontend assets, HTTP authentication, or window state. Raw XPC, plist, CoreDevice client, and device transport types never cross its public API.

Adapters depend on core services and cannot directly open CoreDevice, DVT, Lockdown, AFC, House Arrest, Installation Proxy, or diagnostics clients. During migration, existing bounded command sinks and slots may be re-exported as compatibility APIs, but new adapter behavior must use typed services.

## Ownership

The runtime owns the dedicated 16 MiB device thread, Tokio runtime and `LocalSet`, discovery, transport state, the single active session, reconnect policy, every non-`Send` device client, service supervision, command queues, held-input cleanup, media workers, and sidecar lifecycle.

The host owns directory selection, environment and command-line parsing, setting persistence, Tauri capabilities, HTTP listeners, authentication, TLS and LAN policy, and the choice of local or remote audio consumers. Host-resolved paths and diagnostic overrides enter through configuration. Deep `DEVICEHUB_*` reads and the global FFmpeg resource directory are migration debt and must be removed before the crate boundary is final.

## Target APIs

Core exposes cloneable, typed capabilities without runtime implementation types:

```rust
pub struct DeviceHubServices {
    pub devices: DeviceService,
    pub input: InputService,
    pub applications: ApplicationService,
    pub storage: StorageService,
    pub diagnostics: DiagnosticsService,
    pub media: MediaService,
}
```

Runtime owns startup and deterministic shutdown:

```rust
pub struct RuntimeConfig;
pub struct DeviceRuntime;

impl DeviceRuntime {
    pub fn start(config: RuntimeConfig) -> Result<Self, RuntimeError>;
    pub fn services(&self) -> DeviceHubServices;
    pub fn shutdown(self) -> Result<(), RuntimeError>;
}
```

Starting runtime creates no HTTP, MCP, Tauri, or frontend task. Starting an adapter creates no device session. Shutdown rejects new commands, releases held input, ends the active session and supervised work, stops owned sidecars, and joins the device thread. Explicit shutdown, repeated shutdown, and cleanup after partial startup must be safe.

## Migration

1. Add an internal `DeviceRuntime`, configuration, services, and shutdown boundary without changing desktop behavior.
2. Inject paths, FFmpeg, netmuxd, preferences, logging, and audio publication decisions from the host.
3. Separate domain DTOs, runtime commands and slots, and adapter response types currently concentrated in `protocol.rs`.
4. Create `devicehub-core` and move domain models, validation, policy, and typed service contracts.
5. Create `devicehub-runtime` and move session orchestration, device implementations, supervision, and media publication.
6. Keep the desktop entry point as the composition root for runtime, private server, MCP, and Tauri platform capabilities.
7. Add headless and LAN hosting only after desktop behavior and the library boundaries are stable.

Each step is a separate commit. Source-moving steps preserve behavior, pass `npm run verify:full`, build only the unpackaged Debug desktop application locally, and keep Windows, macOS, and Linux source compatibility. Hardware behavior is finally checked on an iPhone 13 Pro Max over USB and Wi-Fi.

## Boundaries And Acceptance

Neither library installs, sideloads, signs, upgrades, or injects applications. Provisioning-profile management remains a separately authorized device-management capability and cannot become an application installation path.

LAN publication is not implemented by binding the current private server to `0.0.0.0`. A later server boundary requires explicit enablement, TLS, paired clients, scoped roles, control leases, Origin restrictions, rate limits, and revocable sessions. MCP remains loopback-only until it has independent authentication.

Extraction is complete only when Tauri and a headless host can use the same runtime lifecycle, core imports no forbidden implementation dependency, only runtime owns device sessions and non-`Send` clients, core tests require neither Tauri nor a network port, failure paths leave no device task or sidecar, and existing USB/Wi-Fi, WebCodecs, audio, input, App management, AFC, diagnostics, and reconnect behavior remains verified.
