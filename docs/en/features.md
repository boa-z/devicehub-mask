# Feature Reference

[简体中文](../zh-CN/features.md) | [Documentation](README.md) | [User Guide](user-guide.md)

This page is the concise inventory of functionality already implemented in DeviceHub Mask. The [User Guide](user-guide.md) explains workflows and safety limits in more detail. Availability still depends on the services exposed by the connected device and iOS version.

## Desktop Workspaces

| Workspace | Implemented capabilities |
| --- | --- |
| **Device** | USB and Wi-Fi device selection, in-app USB trust pairing, explicit reconnect, live screen, direct touch, mapping and keyboard-passthrough modes, optional injected-touch debug overlay with orientation-aware coordinates and trails, rotation, native screenshot, WebView-supported screen recording, Unicode paste, device audio mute, hardware buttons, focused device fullscreen, and a device inspector with regular/system/App Clip scopes |
| **Key Mapping** | Visual placement and editing, live or frozen screenshot background, profile create/duplicate/rename/import/export, scrcpy-mask `0.0.1` compatibility, PlayCover `2.0.0` import, app-profile associations, and hardware-button shortcuts |
| **AFC** | Unified public AFC, App Documents, App Container, and Crash Reports workspace; searchable app selection; bounded browsing and transfer; create, rename, confirmed recursive delete, progress, cancellation, and confirmed crash-report deletion |
| **Performance** | iPhone CPU/process/memory/energy data, bounded logical/physical core and physical-memory capacity, searchable on-demand running-process inventory, Core Animation FPS, GPU memory, network rates, app activity, video-pipeline telemetry, service health, DVT network/thermal conditions, all-device or per-process network PCAP, and Bluetooth HCI PCAP |
| **Device Logs** | On-demand structured Unified Log with SyslogRelay fallback, search, level filtering, pause, auto-scroll, copy, clear, bounded buffering, recovery state, and confirmed 1/6/24-hour offline Unified Log archive export |
| **Location** | DVT-first virtual location with legacy service fallback, numeric coordinate entry, built-in presets, backend status, and explicit restoration of the real device location |
| **Settings** | Language, always-on-top, system fullscreen, inspector visibility, display scale, mapping overlay, optional injected-touch debug, rotation-control lock, device-fullscreen toolbar behavior, audio, clipboard sync, configurable performance HUD, updates, debug logging, and log-directory access |

System fullscreen and device fullscreen are different. System fullscreen changes the desktop window. Device fullscreen hides navigation and the inspector to give the phone picture and essential controls the available window area.

## Device Inspector

### Info

- Refreshes Lockdown identity, iOS/build versions, bounded device class, CPU architecture, model number and chassis-color fields, normalized language/locale/time-zone settings, storage, activation state, and bounded battery health, temperature, and charging data.
- Refreshes the Info tab after normalized language/time-zone or Developer Disk Image mount notifications without exposing the vendor notification payload.
- Renames the device through a paired Lockdown session and verifies the value.
- Explicitly revokes USB Lockdown trust and removes the host pairing record, with confirmation and partial-success reporting.
- Shows Developer Mode and Developer Disk Image state; it can reveal the Developer Mode setting and explicitly mount, cancel, or unmount a compatible local image set.
- Lists paired Apple Watch metadata through CompanionProxy without controlling the Watch.
- Opens read-only home-screen and lock-screen wallpaper previews from SpringBoardServices only after an explicit click; previews are not persisted or exposed through MCP.
- Creates or resumes an unencrypted local MobileBackup2 backup, with progress, cancellation, and an optional forced full pass.
- Collects a bounded, cancellable CoreDevice sysdiagnose archive.
- Provides confirmed **Restart device** and **Shut down device** commands through Diagnostics Relay. Both intentionally terminate the current device session; shutdown requires manually turning the device on again.

Lock in the device toolbar is a hardware-button press/release toggle and can wake a locked device. The MCP `lock_device` tool is the separate one-way Diagnostics Relay sleep request and does not wake an already locked device.

### Apps

- Lists user apps and, on request, Apple default apps through CoreDevice AppService, with Installation Proxy fallback for the user-app catalog.
- Explicitly launches developer and third-party apps with a bounded, session-only stdout/stderr console through CoreDevice OpenStdioSocket.
- Shows native icons, versions, signing type, removable state, reported storage, running state, and SpringBoard Dock/page/folder placement when available.
- Launches, restarts, force-quits, and safely uninstalls eligible user apps. Force quit resolves the app's fresh main-process PID, sends SIGKILL through CoreDevice AppService, and verifies exit. Uninstall authorization is re-checked against current device metadata and the session reports progress or failure.
- Opens Documents or the full container through House Arrest when iOS permits that scope, with bounded file and directory mutation and transfer.
- Associates an app with a saved key-mapping profile so launching it from the App list activates that profile.
- Explicitly starts and stops an installed developer-signed WebDriverAgent `.xctrunner`; DeviceHub Mask does not install or sign WDA.

### Profiles And Crashes

- Provisioning profiles are listed through Misagent. Local `.mobileprovision` installation validates CMS, UUID, size, and expiration; removal is confirmed and verified against a refreshed catalog. Valid development profiles can explicitly request AMFI app-signer trust after confirmation.
- Crash reports are listed through CrashReportCopyMobile and can be searched, exported, or individually deleted after confirmation. MCP remains read-only and can inspect only a separately bounded text excerpt for agent diagnosis.

## Streaming And Input

| Area | Current behavior |
| --- | --- |
| Video | CoreDevice HEVC transported as compressed access units and decoded exclusively with WebCodecs |
| Recording | Records the rendered canvas at up to 60 FPS through the system WebView's MediaRecorder and downloads MP4 or WebM; it stops on page or device changes and does not include the native device-audio output |
| Audio | Optional CoreDevice AAC-ELD capture, native host playback, volume and mute; enabling capture requires reconnecting |
| Clipboard | One-shot Unicode paste always remains available; optional bidirectional text/image sync requires reconnecting |
| Touch | Direct mouse input and mapping output share a validated five-contact Universal HID report |
| Keyboard | Mapping mode and raw HID keyboard passthrough are mutually exclusive; losing focus, changing page/mode, fullscreen transitions, and disconnect release held input |
| Keymap scripts | Bounded virtual-time programs share the Rust desktop/MCP runtime; no shell, file, environment, process, or network access |
| Hardware buttons | Home, Lock, Volume Up/Down, Mute, Siri, and Action, plus profile-specific keyboard shortcuts |
| System controls | App Switcher through the native-compatible double Home HID sequence |

## idevice Service Coverage

| Capability | Primary service |
| --- | --- |
| Device identity, name, regional settings, storage fallback | Lockdown |
| Live screen, audio, orientation, clipboard, HID | CoreDevice display, orientation, Pasteboard, and HID services |
| Native screenshot | CoreDevice ScreenCaptureService with screenshotr and final DVT Screenshot fallbacks |
| App list, process state, stop; launch fallback | CoreDevice AppService |
| App launch | DVT ProcessControl, with pre-dispatch CoreDevice fallback |
| Explicit per-app console launch | CoreDevice AppService + OpenStdioSocket |
| User-app metadata fallback and safe removal | Installation Proxy |
| App Documents/container | House Arrest and AFC |
| Public media files | Standard AFC / remote AFC shim |
| Bounded battery health/temperature and power actions | Diagnostics Relay |
| Developer Mode and image | AMFI and MobileImageMounter |
| Provisioning profiles and explicit signer trust | Misagent and AMFI |
| Backup | MobileBackup2 |
| Sysdiagnose | CoreDevice DiagnosticsService |
| Device logs and offline archive | OsTraceRelay / SyslogRelay |
| Performance, processes, and conditions | DVT DeviceInfo, Sysmontap, Graphics, Energy, Network Monitor, Notifications, and Condition Inducer |
| Read-only network-interface catalog | DVT DeviceInfo, without IP or MAC addresses |
| Virtual location | DVT Location Simulation with `com.apple.dt.simulatelocation` fallback |
| All-device/per-process network and Bluetooth capture | pcapd packet PID/effective PID metadata and BTPacketLogger |
| Watch metadata | CompanionProxy |
| App icons | CoreDevice AppService with SpringBoardServices fallback |
| Home-screen layout and on-demand wallpaper previews | SpringBoardServices |
| Crash reports and normalized summaries | CrashReportCopyMobile |
| Semantic automation | WebDriverAgent and XCTest runner services |

## MCP Tool Coverage

For setup, coordinate rules, recommended agent workflows, WDA prerequisites, and troubleshooting, see the [MCP Automation Guide](mcp.md).

The Streamable HTTP MCP endpoint exposes the following tools while the desktop app is running:

- Screen and input: `screenshot`, `observe_game`, `tap`, `swipe`, `multi_touch`, `wait_for_frame`, `type_text`, `paste_text`, `press_key`, `press_button`, `app_switcher`, and `rotate`. `observe_game` supplies an ungridded frame and optional normalized region of interest for the Agent loop.
- Key mapping: `list_keymap_profiles`, `get_keymap_profile`, `save_keymap_profile`, `run_keymap`, `start_game_session`, `set_game_input`, and `stop_game_session`. Agents can create native v2 profiles and run persistent 60Hz mapping playback on their selected device; the renewable lease releases controls automatically when the Agent stops updating. Bounded scripts require explicit MCP opt-in.
- Device/session: `status`, `device_details`, `list_devices`, `connect_device`, `reconnect_device`, `lock_device`, `wait_for_device_event`, `list_companion_devices`, and `home_screen_layout`.
- Apps and diagnosis: `list_apps`, `launch_app`, `stop_app`, `app_status`, `wait_for_app`, `list_processes`, `process_status`, `wait_for_process`, `list_crash_reports`, `read_crash_report` with a normalized summary, `performance_snapshot`, and `recent_device_logs`.
- Location and conditions: `set_location`, `clear_location`, `list_device_conditions`, `apply_device_condition`, and `clear_device_condition`.
- WDA: `wda_runner_status`, `wda_start`, `wda_stop`, `wda_status`, `wda_device_state`, `wda_unlock`, `wda_ui_tree`, `wda_find_elements`, `wda_inspect_element`, `wda_wait_for_element`, `wda_click`, `wda_type_text`, `wda_double_tap`, `wda_touch_and_hold`, `wda_scroll`, and `wda_background_app`.

MCP currently exposes one-way device locking, but not device restart or shutdown. Restart and shutdown are available in the desktop Device Info tab and require an interactive confirmation. App installation and upgrades do not exist anywhere in DeviceHub Mask; MCP additionally does not expose App removal, AMFI signer trust, AFC mutation, backup, sysdiagnose, Unified Log archive export, provisioning-profile mutation, packet capture, or Developer Disk Image mutation.

## Intentional Boundaries

- App installation, sideloading, signing, and IPA-based upgrades are explicit non-goals. Future feature work must not add them; prepare and deploy applications with a dedicated tool.
- No device restore, erase, backup-password management, or background backup.
- No AFC2/root filesystem access and no traversal of symbolic links.
- No Apple Watch control or port forwarding.
- No automatic WDA installation/signing and no automatic Developer Disk Image download or version guessing.
- No automatic device conditions; every profile is explicitly selected and normal conditions must be restored after testing.
- No claim of 120 FPS screen streaming: the current negotiated and rendered pipeline is capped at 60 FPS.
- Wi-Fi and remote-service availability remains dependent on pairing, host discovery, Apple services, and iOS policy.
