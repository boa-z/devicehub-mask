# Architecture

[简体中文](../zh-CN/architecture.md) | [Documentation](README.md)

## System Overview

```text
Tauri 2 desktop shell (WKWebView / WebView2 / WebKitGTK)
        |
React 19 + Ant Design workspace
        |
Tauri IPC bootstrap
        |
Authenticated private loopback WebSocket and HTTP API
        |
Rust / Axum service
        |
idevice: CoreDevice, Lockdown, Installation Proxy, Misagent, Universal HID
```

The repository follows the standard Tauri 2 layout. Vite builds the React UI
from `src/`; Rust desktop code and Tauri configuration live in `src-tauri/`.
Tauri embeds production frontend assets and owns the application lifecycle.

## Desktop and Private Transport

Axum is an internal transport, not a separately deployed web server. It binds a
random loopback port by default, has no browser entry point, does not serve the
frontend, and requires a per-launch bearer token obtained through Tauri IPC.
Device-management routes return `503` when no session is active.

The WebSocket carries JPEG frames and typed control messages. The frontend
sends normalized contacts rather than raw HID reports. Rust validates contact
identities, the five-contact limit, coordinate ranges, and orientation before
dispatch.

The MCP service is a separate Streamable HTTP endpoint on
`127.0.0.1:8009/mcp` by default. It shares the manager's latest-frame slot,
input sink, device state, and control channel, so automation and the WebView use
one CoreDevice session. Coordinate tools include the screenshot dimensions and
are transformed through the same orientation model as direct touch. Game
gestures serialize one-to-five-contact HID frames through the shared input
queue. Screenshot and action results expose frame versions so an agent can skip
the visual-stability delay and explicitly wait for the next decoded frame. MCP
has no authentication; binding it beyond loopback is an explicit deployment
decision and emits a warning.

## Session Ownership

The CoreDevice session runs on a dedicated Tokio runtime because several
`idevice` service objects cannot move safely across a normal `tokio::spawn`
boundary. The session owns display, HID, AppService, and device-state resources.
Ending or replacing it cancels dependent work.

Optional device services run under a shared supervisor inside a Tokio
`LocalSet`. This keeps non-`Send` DVT channel objects on the CoreDevice owner
thread while the HTTP, WebSocket, and MCP transports continue using the
multi-thread runtime. Each service publishes a common health record with phase,
attempt count, restart count, last error, and update time. Location, sysmontap,
condition-inducer, graphics, network-monitor, and energy-monitor channels
reconnect independently with bounded exponential backoff; one broken channel
cannot terminate video or HID.

Device condition simulation owns an isolated DVT Condition Inducer channel and a
bounded command queue. The backend bounds and sanitizes the device-provided
catalog, and accepts only group/profile pairs from that catalog. Every channel
connection first disables any residual condition to establish a known baseline.
Apply failures are treated as potentially active because the device may have
committed before the reply failed. Session shutdown performs a bounded cleanup;
if it cannot confirm success, the shared state retains `cleanup_pending` until a
later connection clears the condition. No simulated condition is automatically
restored after reconnecting.

Performance monitoring reuses cloned handles to the active software tunnel and
creates isolated DVT connections. Sysmontap, graphics, network, and energy sampling
are demand-driven by the Performance workspace or selected HUD metrics, and stop
when neither needs them. NetworkMonitor uses its own RemoteServer connection;
one-second receive/send rates are derived from per-connection counter deltas.
Connections expire after one minute without updates and the tracker has a fixed
entry limit. The latest normalized snapshot is exposed through the authenticated
private API; short-term chart history remains frontend-local and is discarded on
device changes.
Sysmontap process arrays are decoded against the attribute order negotiated for
that session rather than fixed field indices. Per-process CPU is divided by the
reported logical CPU count; the snapshot retains the union of the ten highest
CPU and ten largest physical-footprint processes, bounded to twenty rows.
EnergyMonitor follows at most the first sixteen processes from that bounded list
on another RemoteServer connection. It updates the device subscription when the
PID set changes, cancels sampling on demand loss, and exposes Apple's relative
total, CPU, GPU, networking, display, location, and app-state energy scores.

Lockdown metadata is read at connection and refreshed for Device Info requests,
so push notifications for storage or device-name changes surface current values.
App listing and lifecycle control
prefer a long-lived CoreDevice AppService client in the same session, avoiding
a new RSD tunnel per operation. Process URLs are matched only when their direct
parent is the selected app bundle; stop resolves fresh device state and sends a
fixed SIGTERM without accepting a client PID or signal. Listing falls back to
Installation Proxy when AppService is absent, with running state left unknown.
App icons are fetched through a separate request-driven SpringBoardServices RSD
channel, so icon reads never occupy the HID dispatch loop. The worker validates
PNG headers and dimensions, limits each response to 4 MiB, and uses a 256-entry,
32 MiB FIFO cache. The frontend requests only rows near the visible viewport.
Native screenshots use a separate bounded CoreDevice ScreenCaptureService
channel. The worker accepts one queued request, validates the PNG and dimensions,
and caps the response at 32 MiB; capture never occupies the HID dispatch loop.

Device packet capture is a separate, user-initiated pcapd worker over a cloned
RSD tunnel. It writes normalized Ethernet records directly to a same-directory
temporary host file, caps packets at the negotiated 256 KiB snapshot size and
the complete capture at 256 MiB, then atomically replaces the selected `.pcap`
destination. Stop, timeout, stream failure, and session shutdown all finalize
the writer. Only bounded counters and state reach the private API; packet bytes
never enter WebView or MCP transports.

Restart and shutdown are separate fixed private-API commands rather than a
client-supplied DiagnosticsRelay operation. Each opens an independent relay
connection in a bounded task, so waiting for the device acknowledgement does
not stall HID dispatch. The frontend requires a device-named confirmation.

App Documents uses a dedicated supervised House Arrest worker over a cloned RSD
tunnel. Each command vends only the selected application's Documents root and
opens a fresh AFC session. Remote paths reject traversal and separators in item
names. Downloads and uploads stream between AFC and host files; downloads use a
rollback-capable local replacement, while uploads write a uniquely named remote
temporary file and rename it only after the stream closes. Uploads do not
silently replace an existing item, and deletes are non-recursive.

Clipboard synchronization connects CoreDevice Pasteboard Service only when its
persisted opt-in setting is enabled for a newly connected session. Device changes
are push-driven when available, while host changes use a bounded-rate poll with
echo suppression. The disabled default performs no background clipboard access or
transfer; an explicit one-shot paste still writes the requested text. Activity is
published through an eight-entry broadcast channel to authenticated WebSocket
clients, so UI feedback cannot backpressure the service.
One-shot Unicode paste commands share that Pasteboard Service owner through a
four-entry command queue and issue Cmd+V only after receiving the SET reply.

IPA installation and app removal use independent Tokio tasks and fresh
Installation Proxy connections so uploads do not block video, HID, or app-list
requests. The backend re-queries an uninstall target and permits only removable,
non-first-party user apps. One shared operation state exposes stage and
device-reported progress.

The current `idevice` package helper buffers a selected IPA before AFC upload
and cannot report byte-level upload progress. The frontend therefore labels the
upload stage as indeterminate instead of displaying invented percentages.

Crash-report listing and export open fresh CrashReportCopyMobile/AFC sessions in
independent Tokio tasks, so recursive directory reads and file transfer cannot
block HID dispatch. Listing is bounded by depth and entry counts. Export
revalidates an absolute device path and regular-file metadata, caps allocation
at 128 MiB, and returns only metadata to the WebView.

Device logs use a supervised SyslogRelay connection only while the log workspace
is open. Log messages are sanitized and capped at 16 KiB before entering a
2,000-entry in-memory ring buffer. The private API returns at most 500 entries
per poll and reports cursor gaps. Device log content is never forwarded to the
application tracing subsystem or persisted automatically.

## Video Pipeline

CoreDevice displayservice produces RTP/HEVC. The backend assembles complete HEVC
access units into a 16 MiB byte-bounded queue before FFmpeg; overflow discards
dependent frames until an IRAP and requests PLI/FIR recovery. FFmpeg emits
self-describing RGB24 PAM frames by default. The experimental YUV420P setting
(also selectable with `DEVICEHUB_VIDEO_PIXEL_FORMAT=yuv420p`) emits YUV4MPEG2
and sends planar YUV420P directly to TurboJPEG, avoiding the RGB conversion and
halving decoded frame bandwidth. A `watch` channel publishes only the latest
frame and wakes WebSocket consumers without a fixed-rate polling loop; lagging
consumers drop stale decoded frames by construction.

Axum JPEG-encodes the latest frame with a thread-local reusable TurboJPEG
compressor. At most two frames are allowed in flight per WebView, so backend JPEG
encoding can overlap WebView JPEG decoding without forming an unbounded queue.
The frontend acknowledges decoded, presented, or deliberately replaced frames.
A 500ms credit lease prevents a lost acknowledgement from permanently stalling
video.

Windows limits the decoded long edge to 1920 pixels by default. FFmpeg preserves
aspect ratio, never upscales, and emits even dimensions. Set
`DEVICEHUB_VIDEO_MAX_DIMENSION=0` for native resolution or choose a lower value
on slower systems.

The canvas contain-fits the rotated source with one shared scale. Pointer
coordinates are normalized in the exact displayed rectangle, which prevents
landscape stretching and touch offset.

## Audio Pipeline

CoreDevice negotiates AAC-ELD at 48 kHz stereo with one 10 ms access unit per
RTP packet. The device sends bare access units, so the backend adds RFC 3640 AU
headers before forwarding RTP to FFmpeg. FFmpeg decodes to interleaved S16LE;
the backend publishes bounded 20 ms PCM chunks and never waits for consumers.

Audio uses a versioned `DHAP` binary WebSocket envelope while JPEG messages keep
their existing format. The WebView schedules PCM with Web Audio, starts with a
small jitter buffer, and resets if queued latency exceeds 250 ms. Audio is off
by default and audio decoder failure falls back to draining the negotiated
stream without terminating video or input.

## Input Pipeline

Mapping, direct pointer, and keyboard state are combined in React. Identical
touch frames are not resent. Rust converts validated typed contacts into one
fixed five-slot Universal HID multitouch report. Keyboard and hardware commands
preserve down/up state, and disconnect cleanup releases held usages.

Mapping mode and keyboard passthrough are mutually exclusive. This prevents a
single physical key from producing both a mapped touch and a keyboard usage.

## Provisioning Data

Profiles are managed by an independently supervised, bounded Misagent command
service, so profile operations do not block the HID input loop. CMS SignedData
is decoded before plist metadata enters the private API. Raw profile payloads
and provisioned device identifiers never cross into the frontend. A malformed
profile is isolated rather than failing the complete result.

Install and removal commands carry request deadlines so a timed-out HTTP request
cannot apply later from the queue. Installs validate the local file and profile
metadata before device mutation; removals refresh the device catalog before and
after mutation. Input, not-found, conflict, transport, and timeout failures retain
typed semantics through the private API. Only transport and timeout failures
cause the supervisor to rebuild the Misagent channel.

If displayservice is unavailable, the backend preserves a reduced management
session when Lockdown remains usable. Screen control and AppService-only actions
are explicitly unavailable rather than hiding the whole device.

## Dependency Pin

The `idevice` dependency is temporarily pinned to reviewed revision `a64b886`
from the project fork. It includes the iOS 27 CoreDevice fixes and typed DVT
NetworkMonitor and EnergyMonitor clients used by the performance workspace.
Replace this pin after equivalent fixes are merged and released upstream.

## Security Boundaries

- The private API remains loopback-only and token-authenticated.
- MCP is loopback-only by default, is unauthenticated, and warns on non-loopback binds.
- Frontend app metadata is never accepted as uninstall authorization.
- HID reports are built only after backend validation.
- Updater artifacts require a Tauri signature before installation.
- Apple Developer ID signing is separate from updater signing.
