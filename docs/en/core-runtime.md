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

Device storage follows that ownership directly: core defines public AFC and application-container DTOs, transfer activity policy, cancellation classification, bundle-identifier validation, and confined device-path normalization. Runtime owns the AFC and House Arrest execution commands and transports, and hosts retain opaque local paths plus filesystem stream implementations.

Core also owns bounded observation ports whose behavior is independent of Apple transports, including capture and diagnostic status, device-condition state, and the normalized device-log ring buffer. Runtime owns the producers: protocol translation, demand gating, retries, deadlines, and command workers remain implementation details.

Developer Image mount state and version-to-image-type policy follow the same rule. Core exposes the normalized observation; runtime owns opaque asset requests, host-injected loading, personalization, device transport, and operation supervision.

Core owns the normalized service-health registry and restart-count transition policy. Runtime owns the reporters and all executable supervision behavior, including tracing, retry delays, shutdown signals, task spawning, and forced aborts.

Core also owns the merged performance observation slot and its bounded history and ranking policies. Runtime-specific converters accept Apple DVT and plist samples and emit typed normalized observations; demand signals, sampling workers, and device channels stay in runtime.

`devicehub-runtime` may depend on core, `idevice`, Tokio, serialization, media helpers, and platform-neutral filesystem and networking APIs. It must not depend on Tauri, Axum, rmcp, frontend assets, HTTP authentication, or window state. It does not read the host environment or resolve and launch operating-system processes. Raw XPC, plist, CoreDevice client, and device transport types never cross its public API.

Adapters depend on core services and cannot directly open CoreDevice, DVT, Lockdown, AFC, House Arrest, Installation Proxy, or diagnostics clients. During migration, existing bounded command sinks and slots may be re-exported as compatibility APIs, but new adapter behavior must use typed services. Input commands plus capture and diagnostic status values are imported from core directly; runtime no longer republishes those domain types.

## Ownership

The runtime owns the dedicated 16 MiB device thread, Tokio runtime and `LocalSet`, discovery, transport state, the single active session, reconnect policy, every non-`Send` device client, service supervision, command queues, held-input cleanup, media workers, and sidecar lifecycle policy. A host adapter performs concrete sidecar process resolution and execution behind the runtime port.

Its host-facing facade exposes typed commands, observations, and capability ports only. Concrete input dispatchers, service reporters and supervisors, retry helpers, protocol clients, and transport handles remain private so a host cannot create a second execution or recovery path around the session manager.

The host-facing `RuntimeClient` has two explicit ownership groups. `RuntimeManagerClient` exposes only discovery inventory, active selection, and manager lifecycle control. `DeviceSessionClient` exposes the media, input, observation, service, and device-operation surface associated with the currently selected session. The root client only combines these groups. The internal `CoreRuntimeState` mirrors the same split through private manager and device-session state groups, so manager views and host clients cannot accidentally project different ownership. The `runtime` facade keeps its owner-thread executor and state graph in separate private modules. This preserves the current single-session behavior while providing the boundary required for a later registry of isolated device runtimes.

The host owns directory selection, environment and command-line parsing, setting persistence, operating-system process resolution, Tauri capabilities, HTTP listeners, authentication, TLS and LAN policy, and the choice of local or remote audio consumers. Host-resolved paths, FFmpeg configuration, sidecar adapters, and diagnostic overrides enter through configuration or capability ports. Boundary checks prevent production runtime code from reintroducing environment reads, process launching, or FFmpeg path resolution.

## Target APIs

Core exposes cloneable, typed capabilities without runtime implementation types. Input commands and normalized touch contacts are core domain values; runtime translates them into Apple HID reports:

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
3. Separate domain DTOs, runtime commands and slots, and adapter response types, then remove the mixed `protocol.rs` and wildcard domain facades once adapters import their owner crates directly.
4. Create `devicehub-core` and move domain models, validation, policy, and typed service contracts.
5. Create `devicehub-runtime` and move session orchestration, device implementations, supervision, and media publication.
6. Keep the desktop entry point as the composition root for runtime, private server, MCP, and Tauri platform capabilities.
7. Add headless and LAN hosting only after desktop behavior and the library boundaries are stable.

Each step is a separate commit. Source-moving steps preserve behavior, pass `npm run verify:full`, build only the unpackaged Debug desktop application locally, and keep Windows, macOS, and Linux source compatibility. Hardware behavior is finally checked on an iPhone 13 Pro Max over USB and Wi-Fi.

## Next Host Milestones

After the module extraction is complete, the next repository-level target is a headless CLI service host. It will compose the same `devicehub-runtime` and core services without linking Tauri, window APIs, desktop audio output, or frontend assets. CLI configuration will own listener addresses, authentication material, data directories, pairing storage, sidecar resolution, logging, shutdown signals, and explicitly enabled HTTP/WebSocket/MCP adapters. The first version remains loopback-only by default; LAN publication still requires the security boundary below.

Multi-device connection support follows the headless host because that host provides the clearest lifecycle test. The current single-runtime graph will evolve into a host-owned runtime registry plus one isolated `DeviceRuntime` per selected physical device. Discovery and trust storage become shared coordination services, while each device retains its own owner thread, session, supervisor tree, commands, media flow control, demand counters, and deterministic shutdown. USB and Wi-Fi endpoints for the same physical device must resolve to one logical device and one active transport, and global CPU, memory, decoder, audio-output, and reconnect limits must be explicit rather than hidden in process-wide state.

## Boundaries And Acceptance

Neither library installs, sideloads, signs, upgrades, or injects applications. Provisioning-profile management remains a separately authorized device-management capability and cannot become an application installation path.

LAN publication is not implemented by binding the current private server to `0.0.0.0`. A later server boundary requires explicit enablement, TLS, paired clients, scoped roles, control leases, Origin restrictions, rate limits, and revocable sessions. MCP remains loopback-only until it has independent authentication.

Extraction is complete only when Tauri and a headless host can use the same runtime lifecycle, core imports no forbidden implementation dependency, only runtime owns device sessions and non-`Send` clients, core tests require neither Tauri nor a network port, failure paths leave no device task or sidecar, and existing USB/Wi-Fi, WebCodecs, audio, input, App management, AFC, diagnostics, and reconnect behavior remains verified.
