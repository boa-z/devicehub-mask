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

## Session Ownership

The CoreDevice session runs on a dedicated Tokio runtime because several
`idevice` service objects cannot move safely across a normal `tokio::spawn`
boundary. The session owns display, HID, AppService, and device-state resources.
Ending or replacing it cancels dependent work.

Lockdown metadata is read once at connection. App listing and launch prefer a
long-lived CoreDevice AppService client in the same session, avoiding a new RSD
tunnel per operation. Listing falls back to Installation Proxy when AppService
is absent.

IPA installation and app removal use independent Tokio tasks and fresh
Installation Proxy connections so uploads do not block video, HID, or app-list
requests. The backend re-queries an uninstall target and permits only removable,
non-first-party user apps. One shared operation state exposes stage and
device-reported progress.

The current `idevice` package helper buffers a selected IPA before AFC upload
and cannot report byte-level upload progress. The frontend therefore labels the
upload stage as indeterminate instead of displaying invented percentages.

## Video Pipeline

CoreDevice displayservice produces RTP/HEVC. FFmpeg receives Annex-B HEVC and
emits self-describing RGB24 PAM frames. A latest-frame slot drops stale decoded
frames rather than allowing an unbounded queue.

Axum JPEG-encodes the latest frame with a thread-local reusable TurboJPEG
compressor. One frame is allowed in flight per WebView; the frontend acknowledges
after image decode and canvas presentation. A 500ms credit lease prevents a lost
acknowledgement from permanently stalling video. This keeps send FPS close to
display FPS while retaining the 60 FPS ceiling.

Windows limits the decoded long edge to 1920 pixels by default. FFmpeg preserves
aspect ratio, never upscales, and emits even dimensions. Set
`DEVICEHUB_VIDEO_MAX_DIMENSION=0` for native resolution or choose a lower value
on slower systems.

The canvas contain-fits the rotated source with one shared scale. Pointer
coordinates are normalized in the exact displayed rectangle, which prevents
landscape stretching and touch offset.

## Input Pipeline

Mapping, direct pointer, and keyboard state are combined in React. Identical
touch frames are not resent. Rust converts validated typed contacts into one
fixed five-slot Universal HID multitouch report. Keyboard and hardware commands
preserve down/up state, and disconnect cleanup releases held usages.

Mapping mode and keyboard passthrough are mutually exclusive. This prevents a
single physical key from producing both a mapped touch and a keyboard usage.

## Provisioning Data

Profiles are read over a long-lived Misagent connection and decoded as CMS
SignedData before plist metadata enters the private API. Raw profile payloads
and provisioned device identifiers never cross into the frontend. A malformed
profile is isolated rather than failing the complete result.

If displayservice is unavailable, the backend preserves a reduced management
session when Lockdown remains usable. Screen control and AppService-only actions
are explicitly unavailable rather than hiding the whole device.

## Dependency Pin

The `idevice` dependency is temporarily pinned to reviewed revision `0371286`
from the project fork. It includes `requireContainerAccess=false`, required by
the iOS 27 AppService request decoder. Replace this pin after an equivalent fix
is merged and released upstream.

## Security Boundaries

- The private API remains loopback-only and token-authenticated.
- Frontend app metadata is never accepted as uninstall authorization.
- HID reports are built only after backend validation.
- Updater artifacts require a Tauri signature before installation.
- Apple Developer ID signing is separate from updater signing.
