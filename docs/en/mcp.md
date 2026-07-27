# MCP Automation Guide

[Simplified Chinese](../zh-CN/mcp.md) | English | [Documentation home](README.md)

DeviceHub Mask exposes connected iOS device sessions to agents through a built-in Model Context Protocol (MCP) server. This guide covers setup, reliable control workflows, diagnostics, WebDriverAgent (WDA), security, and the operations intentionally kept out of MCP.

## Before You Connect

Start the DeviceHub Mask desktop app and connect at least one device first. The MCP server exists only while the desktop app is running. Each MCP protocol connection selects its own device and reuses that device's CoreDevice session, video stream, input queue, performance services, and bounded log buffers. It does not open a competing connection or change the target selected by the desktop UI or another MCP client.

The default Streamable HTTP endpoint is:

```text
http://127.0.0.1:8009/mcp
```

For example, register it with Claude Code:

```sh
claude mcp add --transport http devicehub-mask http://127.0.0.1:8009/mcp
```

Call `status` after registration. If that MCP connection has no target, call `list_devices`, then pass the exact returned selection ID to `connect_device`. USB and Wi-Fi entries can represent the same physical device but have distinct selection IDs; only one transport for a UDID can run at a time.

The MCP endpoint has no authentication. Keep it bound to loopback. `DEVICEHUB_MCP_ADDR` can change the bind address, but exposing it beyond loopback gives network clients access to device screenshots, input, app control, process names, logs, crash reports, location simulation, and WDA output. A non-loopback bind emits a warning and should be used only on a trusted isolated network.

## Choose the Right Control Path

DeviceHub Mask exposes three different coordinate concepts. They are not interchangeable.

| Source | Coordinate meaning | Use it for |
| --- | --- | --- |
| `screenshot` | Pixels in the returned image | `tap`, `swipe`, and `multi_touch` |
| `home_screen_layout` | 1-based Dock, page, and folder positions | Finding where an app is organized, not tapping it |
| WDA element rectangles | WDA logical window units | WDA semantic inspection and actions |

Use screenshot-based Universal HID for games and visual interfaces. It has lower latency and works without accessibility metadata. Use WDA for forms, named controls, state inspection, and workflows where semantic selectors are more reliable than pixels.

## Screenshot and HID Workflow

1. Call `screenshot`. The coordinate grid is enabled by default and the longer edge defaults to 1,024 pixels; set `max_dim` to `0` only when native resolution is necessary.
2. Read `image_width` and `image_height` from the result.
3. Locate the target in that returned image.
4. Pass the same dimensions with `tap`, `swipe`, or `multi_touch`. DeviceHub Mask applies the current orientation transform and screenshot scale.
5. Inspect the next screenshot or use frame synchronization before making a dependent action.

`tap` holds for 100 ms by default and clamps `hold_ms` to 25 through 5,000 ms. `swipe` defaults to 300 ms and clamps its duration to 50 through 5,000 ms. `multi_touch` accepts one to five simultaneous paths, defaults to 250 ms, and clamps its duration to 25 through 5,000 ms. Identical start and end points represent held buttons.

For example, this moves a left joystick while holding a right action button:

```json
{
  "contacts": [
    { "x1": 180, "y1": 700, "x2": 240, "y2": 650 },
    { "x1": 850, "y1": 680, "x2": 850, "y2": 680 }
  ],
  "duration_ms": 250,
  "image_width": 1024,
  "image_height": 768
}
```

`type_text` sends printable HID text. Use `paste_text` for CJK or other Unicode text; it writes the device pasteboard and sends Cmd+V. `press_key` handles navigation keys such as Enter, Escape, arrows, Home, End, Page Up, and Page Down. `press_button` operates `home`, `lock`, `volume-up`, `volume-down`, `mute`, `siri`, or `action` as hardware buttons.

`lock_device` is different from `press_button` with `button="lock"`: `lock_device` sends a one-way Diagnostics Relay sleep request and cannot wake an already locked device, while the hardware lock button toggles physical-button behavior and may wake it.

## Low-Latency Game Workflow

`tap` and `swipe` wait for visual stability by default. That is convenient for ordinary UI automation but adds latency to repeated game actions. `multi_touch` defaults to no stability wait.

For latency-sensitive loops:

1. Send the action with `wait_for_settle=false`.
2. Save the returned `frame_version_after`.
3. Call `wait_for_frame` with that value as `after_version`.
4. Take the next screenshot only after a newer frame is reported.

`wait_for_frame` defaults to a two-second timeout and accepts 1 through 10,000 ms. A timeout means no newer decoded frame arrived within the requested interval; it does not by itself prove that the device session disconnected or that a visually static app is unhealthy. Check `status` and retry or reconnect only when the session state also indicates a failure.

Coordinate and WDA mutation tools share a gesture lock, so two agent actions do not interleave their touch streams. This serialization does not make an old screenshot current; take another screenshot whenever orientation or layout may have changed.

## Device and Session Workflow

Use `list_devices` for the current transport inventory and `status` for this MCP connection's selected session. `connect_device` selects or reuses the exact session without stopping sessions for other physical devices. `reconnect_device` tears down and rebuilds only that target. Both wait for a new video frame for a bounded period and may report that connection is still being established; follow with `status` or `screenshot` rather than repeatedly reconnecting.

`device_details` refreshes normalized product, OS, hardware, storage, activation, Developer Mode, regional, and bounded battery information. Stable identifiers are deliberately omitted unless `include_identifiers=true`; UDID, serial number, and ECID should be requested only when identity is required.

`list_companion_devices` is a bounded read-only Apple Watch metadata query. An empty list is valid. It does not provide Watch control, service startup, or port forwarding.

## Apps and Processes

Call `list_apps` to discover an exact Bundle ID before using app tools. It returns user apps by default, accepts a case-insensitive name or Bundle ID query, and returns at most 100 entries by default and 200 at the limit. `include_system=true` adds Apple default apps and `include_app_clips=true` adds App Clips when CoreDevice AppService supports those scopes. Hidden and internal apps remain excluded.

Use `launch_app` and `stop_app` for lifecycle changes. Both wait for visual stability by default; disable that wait when the next step uses `wait_for_app` or explicit frame synchronization. `app_status` checks installed and running state. `wait_for_app` waits for `running` or `stopped`, defaults to five seconds, accepts at most ten seconds, and checks once when `timeout_ms=0`.

`list_processes` returns a bounded DVT inventory with PID, sanitized process/app names, and Apple's application classification. Use a fresh result before `process_status` or `wait_for_process`: operating systems can reuse PIDs. Process waits use the same five-second default, ten-second maximum, and zero-time single-check behavior as app waits. MCP cannot terminate an arbitrary PID or inspect process memory.

Per-app stdout/stderr capture remains desktop-only because console output can contain credentials and personal data. App installation, sideloading, signing, and upgrades are permanent project non-goals and must not be added to MCP or any other DeviceHub Mask interface. MCP also does not expose App removal.

## Event-Driven Waiting

`wait_for_device_event` avoids client polling for normalized changes to apps, storage, regional settings, device name, activation, Developer Disk Image mount state, and SpringBoard lock state. It defaults to ten seconds and accepts up to 30 seconds.

After receiving an event, pass its `sequence` as `after_sequence` on the next call. The cursor closes the read/subscribe race and lets the server return an already-retained newer event. When no cursor is provided, only an event occurring after the call starts is eligible.

Events report that a change occurred, not always the resulting value. After `regional_settings_changed` or `developer_image_mounted`, call `device_details`. After `lock_state_changed`, take a screenshot: Notification Proxy does not provide the final lock value. Raw Apple notification names and payloads do not cross the MCP boundary.

## Diagnostics Workflow

`performance_snapshot` temporarily requests the existing DVT samplers. It waits up to 2.5 seconds for a fresh sample by default; `wait_ms=0` returns the cached snapshot immediately. Results can include CPU capacity and usage, top processes, memory, relative energy, Core Animation, GPU-memory, and network metrics when those services are available.

`recent_device_logs` temporarily requests the existing device log service and returns at most 500 matching entries. Use `after` as an incremental sequence cursor, `level` for `notice`, `info`, `debug`, `error`, or `fault`, and `query` for case-insensitive matching across messages and metadata. Temporary MCP demand does not turn off sampling or logging already requested by the desktop workspaces.

For an app crash:

1. Call `list_crash_reports`, optionally with a report-name or path query. The default limit is 50 and the maximum is 200.
2. Select an exact returned `device_path`.
3. Call `read_crash_report`. It returns 256 KiB by default and never more than 1 MiB, with `truncated` and `lossy_utf8` flags.

Crash tools are read-only. Report reads reject relative paths, traversal, directories, and oversized requests. Screenshots, process names, logs, crash excerpts, and WDA trees can all contain sensitive data.

## Location and Device Conditions

`set_location` applies fixed latitude and longitude through the active DVT or legacy location service. Always call `clear_location` when the test no longer requires simulation.

Device conditions affect the entire phone, not one app. Call `list_device_conditions`, choose only a returned group/profile pair, then use `apply_device_condition`. Network or thermal profiles can interrupt the foreground game and the MCP connection itself. Put `clear_device_condition` in test cleanup, including failure handling. If cleanup is reported as pending after a transport failure, keep the device connected so the supervised DVT channel can restore normal conditions after recovery.

## WebDriverAgent Workflow

WDA is optional and separately prepared. DeviceHub Mask does not install or sign it. Before using WDA tools:

1. Enable Developer Mode on the device.
2. Mount a compatible Developer Disk Image. The desktop Device Info page reports readiness and provides the explicit mount workflow.
3. Install and sign a compatible WebDriverAgent `.xctrunner` for the device.
4. Start it externally, or discover its exact Bundle ID with `list_apps` and call `wda_start`.
5. Call `wda_status` before semantic automation.

`wda_start` uses XCTest and waits at most 30 seconds. `wda_runner_status` reports only a runner started by DeviceHub Mask, and `wda_stop` stops only that managed runner. It never terminates an externally managed WDA. DeviceHub Mask also never downloads or guesses a Developer Disk Image.

For semantic interaction, prefer accessibility IDs or names:

1. Call `wda_device_state` when coordinate-space or lock state matters.
2. Use `wda_find_elements` or a bounded `wda_ui_tree` to discover controls.
3. Use `wda_inspect_element` when displayed, enabled, or selected state matters.
4. Use `wda_wait_for_element` instead of client-side polling.
5. Act with `wda_click`, `wda_double_tap`, `wda_touch_and_hold`, `wda_type_text`, or `wda_scroll`.

Supported selector strategies are `accessibility id`, `name`, `class name`, `xpath`, `-ios predicate string`, and `-ios class chain`. Find returns at most 20 zero-based matches. Wait states are `present`, `absent`, `displayed`, `hidden`, `enabled`, `disabled`, `selected`, and `unselected`; waits default to five seconds, accept at most ten seconds, and check once at zero. A missing element satisfies `absent` and `hidden`, but not `disabled` or `unselected`.

`wda_type_text` accepts Unicode text up to 1,024 characters and 4,096 UTF-8 bytes. `wda_touch_and_hold` accepts 100 through 10,000 ms. `wda_scroll` accepts `up`, `down`, `left`, or `right`. `wda_background_app` leaves the foreground app in the background when no delay is supplied or asks WDA to restore it after 100 through 5,000 ms.

`wda_unlock` takes no passcode, cannot bypass authentication, and succeeds only after WDA confirms the resulting unlocked state. `wda_ui_tree` may expose passwords, messages, and other visible text. WDA logical rectangles are not screenshot pixels, so do not pass them directly to HID coordinate tools.

## Tool Reference

| Area | Tools | Notes |
| --- | --- | --- |
| Screen and input | `screenshot`, `tap`, `swipe`, `multi_touch`, `wait_for_frame`, `type_text`, `paste_text`, `press_key`, `press_button`, `lock_device`, `rotate` | Screenshot dimensions define HID coordinates; one to five simultaneous contacts |
| Device and session | `status`, `device_details`, `list_devices`, `connect_device`, `reconnect_device`, `wait_for_device_event`, `list_companion_devices`, `home_screen_layout` | Exact selection IDs preserve USB/Wi-Fi identity; stable identifiers are opt-in |
| Apps and processes | `list_apps`, `launch_app`, `stop_app`, `app_status`, `wait_for_app`, `list_processes`, `process_status`, `wait_for_process` | Use exact Bundle IDs and fresh PIDs |
| Diagnostics | `list_crash_reports`, `read_crash_report`, `performance_snapshot`, `recent_device_logs` | Bounded, read-only diagnostic output |
| Location and conditions | `set_location`, `clear_location`, `list_device_conditions`, `apply_device_condition`, `clear_device_condition` | Clear simulations after every test |
| WDA | `wda_runner_status`, `wda_start`, `wda_stop`, `wda_status`, `wda_device_state`, `wda_unlock`, `wda_ui_tree`, `wda_find_elements`, `wda_inspect_element`, `wda_wait_for_element`, `wda_click`, `wda_type_text`, `wda_double_tap`, `wda_touch_and_hold`, `wda_scroll`, `wda_background_app` | Requires separately prepared WDA and developer prerequisites |

## Intentional Boundaries

MCP does not expose device restart, shutdown, restore, or erase; App removal; AMFI signer trust; AFC or App-container mutation; backup or backup-password management; sysdiagnose collection; Unified Log archive export; provisioning-profile mutation; packet capture; Developer Disk Image mount/unmount; Apple Watch control; or automatic WDA installation and signing. App installation, sideloading, signing, and upgrades are unavailable throughout DeviceHub Mask by permanent product policy, rather than merely omitted from MCP.

Restart and shutdown are available only through the confirmed desktop Device Info actions. File mutation, signing trust, image management, captures, and destructive operations remain interactive so an agent cannot silently broaden its authority.

## Troubleshooting

- **The client cannot connect:** confirm the desktop app is running, check its log for the MCP listening address, and call the exact `/mcp` path. A port bind failure does not stop the desktop device session.
- **No device is active:** call `list_devices`, connect with an exact returned selection ID, then call `status`. Keep the device unlocked, trusted, and Developer Mode-enabled where required.
- **A tap misses:** take a fresh screenshot and pass its exact `image_width` and `image_height`. Do not use native device resolution, SpringBoard ordinal positions, or WDA logical coordinates as screenshot pixels.
- **A frame wait expires on a static screen:** treat it as “no newer frame,” not an automatic disconnect. Check `status`; request a new screenshot before reconnecting.
- **An app or process wait expires:** verify the exact Bundle ID or obtain a fresh PID. A normal wait result can report that the target state was not reached without implying transport failure.
- **WDA is unavailable:** verify Developer Mode, the matching mounted Developer Disk Image, the installed/signed `.xctrunner`, and `wda_status`. DeviceHub Mask cannot repair signing or install WDA for you.
- **A condition disrupts connectivity:** reconnect the transport if necessary, keep the device attached, and call `clear_device_condition` until normal conditions are confirmed.

For server configuration and logs, see [Development](development.md). For device transport, CoreDevice, and video failures, see [Troubleshooting](troubleshooting.md).
