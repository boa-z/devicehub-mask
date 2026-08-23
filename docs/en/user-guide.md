# User Guide

[简体中文](../zh-CN/user-guide.md) | [Documentation](README.md)

This guide covers everyday desktop workflows. Complete installation and device preparation first in [Getting Started](getting-started.md). For every implemented field and service, use the [Feature Reference](features.md).

## Connect and Switch Devices

The Devices Overview is the default workspace. It groups USB and authenticated Wi-Fi transports by physical device and shows connection phase, active resource demands, last activity, and copyable session errors. It does not open video, audio, or control WebSockets for its device rows. Unlock and trust the device when prompted, connect the desired transport, then use **Open control** to enter the realtime Device workspace.

The compact connection center in the top bar provides the same lifecycle operations from any workspace. USB and Wi-Fi entries for one device are separate choices, but only one may be active for that physical device.

Multiple devices can remain connected. Selecting another device changes the visible workspace without disconnecting existing sessions. Connection phase and errors are shown per device. Use reconnect for a failed session; remove trust only when you intend to pair again. A metadata timeout does not necessarily mean the media session failed, and recoverable metadata is loaded again in the background.

The clock button in the top bar opens **Device activity** for the selected transport. It shows active and recent bounded operations, progress, stage, completion time, and copyable failures. Cancellable work has a confirmed **Cancel task** action; **Open tool** returns to its detailed workspace. The Devices Overview shows active-operation counts from the shared status snapshot without polling every device separately.

## Control the Device

The Device workspace contains the live frame, direct touch surface, stream state, display controls, hardware buttons, and feature tools. Right-click the frame to send Home. Select **Mapping** to convert configured keys into touch/hardware actions, or **Keyboard passthrough** to forward physical HID key events. Mouse direct touch remains available in both modes; keyboard passthrough sends it through the raw multi-touch path without key mapping. Changing mode, page, focus, fullscreen, or connection releases held input.

Pointer coordinates are confined to the displayed phone frame, excluding letterbox areas, and follow orientation. Up to five simultaneous contacts are supported. The text action writes bounded Unicode text to the device pasteboard and sends paste; focus an editable field first.

Hardware and feature toolbar groups can be arranged for the available space. In focused/fullscreen presentation they can dock to edges, use vertical columns on side docks, and attach as a compact pair. Layout choices persist; Settings can restore defaults. Hardware controls retain priority when space is constrained. Device focused presentation and operating-system fullscreen are separate controls.

## Manage the View

Use fit and scale controls to size the frame. Hide sidebars when the device needs more room. A static device screen is not considered a stopped stream solely because pixels do not change; connection and media progress determine health. If the transport actually stalls, use the copyable error details and reconnect action rather than repeatedly opening new sessions.

Screenshots use a lossless device service fallback chain and are separate from WebCodecs video frames. The optional performance HUD appears over the Device workspace and can be configured in Settings.

Enable **Injected touch debug** from the device toolbar or Settings when diagnosing host input. It overlays the final touch frames produced by the current browser control session, including contact IDs, down/move/release trails, display coordinates, native coordinates after orientation conversion, and active mapping IDs. It is disabled by default and reports host-generated frames only; iOS touch events are not read back.

## Use Key Mapping

Open Keyboard Mapping to create or select a profile, choose a live/captured background, add controls, assign unique keys, and test in Mapping mode. Profiles may bind to an app bundle ID and an exact device frame resolution so the same app can use different iPhone and iPad layouts. DeviceHub Mask, scrcpy-mask, and PlayCover imports are explicitly selected in the import dialog.

The editor supports undo/redo, controller type changes where valid, hardware-button shortcuts, import/export, and opening the profile directory. See [Key Mapping Guide](key-mapping.md) for controller semantics, contact ownership, file format and troubleshooting.

## Inspect and Manage a Device

The inspector groups device identity, battery/storage/region data, apps, provisioning profiles, crashes, app containers, and supported management actions. Optional data can be unavailable even when control works because Apple exposes services independently.

To repair a rejected pairing, connect and select the device's paired **USB** transport, open the **Info** tab, then scroll to **Computer trust > Forget computer trust**. This action is intentionally hidden for Wi-Fi and unpaired selections. It ends the current session and removes both the Lockdown trust record and DeviceHub Mask's RemotePairing credentials; reconnect the cable afterwards and approve **Trust This Computer** on the unlocked device. Do not use it for an ordinary disconnect or a single transient EOF. See [Remote Pairing Verification Ends With Early EOF](troubleshooting.md#remote-pairing-verification-ends-with-early-eof) for the complete recovery flow.

The Apps view can inspect existing apps, launch/restart supported apps, force-quit a running app after resolving its current process, show bounded signing/runtime metadata, open a developer app with a session-only console, remove confirmed removable apps, and open permitted app storage. Force quit does not ask the app to save state. It deliberately cannot install, sideload, sign, inject, or upgrade applications.

App storage uses House Arrest for one app's Documents or permitted container. The AFC workspace uses the device-wide public media container. They are different roots and permissions are controlled by iOS. Transfers expose progress and cancellation, reject unsafe paths and special entries, and keep temporary data busy until cleanup completes.

Provisioning-profile operations validate bounded `.mobileprovision` data and require confirmation for trust/removal. They are not an app installation path. Crash reports can be listed and exported; deletion requires confirmation. Diagnostic and capture outputs may contain private information and should be handled as sensitive files.

## Performance, Logs, and Diagnostics

The Performance workspace enables sampling only while needed. It shows normalized CPU, memory, graphics, energy, process, network-interface and service-health observations when the device supplies them. Values are device observations, not host Activity Monitor percentages. Some DVT services require Developer Mode or may be prohibited by iOS.

Device Logs prefers structured Unified Log and falls back to SyslogRelay. Filtering and pause are local; the bounded buffer may drop old entries under load. Log archive, sysdiagnose, backup, network PCAP and Bluetooth HCI capture are explicit long operations with limits and privacy warnings. Use the unified copy button when reporting an error, and include the relevant JSONL runtime log section.

## Virtual Location and Device Conditions

Virtual Location prefers DVT and may use a compatible legacy service. Starting simulation changes the selected device until stopped or session cleanup restores real location. Device network/thermal conditions are developer diagnostics and should be reset after testing. Both are target-scoped in a multi-device session.

## Settings and Updates

Settings controls language, startup/update channel, diagnostics, clipboard behavior, performance HUD, device-view preferences, and toolbar reset. Language changes immediately and preserves user-authored names. Diagnostics shows the current run ID, log filter and log directory; Debug logging affects the current run unless an environment filter overrides it.

Stable and Nightly are separate update channels. An accepted desktop update is signature-verified before installation and restart. See [CI, Releases, and Updates](distribution.md) for version and signing details.

## Browser and Agent Operation

The same UI can run from the headless service. Host-only operations are capability-gated, and LAN use requires explicit publication and a token. See [Headless Service](headless.md).

Agents use the built-in MCP service to select an independent target, take screenshots, send HID input, operate apps, wait on state, inspect diagnostics, and optionally use WDA semantics. MCP does not change desktop selection. See [MCP Automation Guide](mcp.md) for setup, safety and tool workflows.

## When Something Fails

Copy the complete presented error, note the selected device and transport, then collect the matching runtime log interval. Avoid removing trust or deleting pairing data as a first step. Use [Troubleshooting](troubleshooting.md) for platform preparation, remote-pairing EOF, missing audio, stalled video, CoreDevice errors, CPU load and update failures.
