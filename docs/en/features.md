# Feature Reference

[简体中文](../zh-CN/features.md) | [Documentation](README.md) | [User Guide](user-guide.md)

This page is the concise inventory of functionality already implemented in DeviceHub Mask. The [User Guide](user-guide.md) explains workflows and safety limits in more detail. Availability still depends on the services exposed by the connected device and iOS version.

## Desktop Workspaces

| Workspace | Implemented capabilities |
| --- | --- |
| **Device** | USB and Wi-Fi device selection, explicit reconnect, live screen, direct touch, mapping and keyboard-passthrough modes, rotation, native screenshot, WebView-supported screen recording, Unicode paste, device audio mute, hardware buttons, focused device fullscreen, and a device inspector with regular/system/App Clip scopes |
| **Key Mapping** | Visual placement and editing, live or frozen screenshot background, profile create/duplicate/rename/import/export, scrcpy-mask `0.0.1` compatibility, PlayCover `2.0.0` import, app-profile associations, and hardware-button shortcuts |
| **AFC** | Unified public AFC, App Documents, App Container, and Crash Reports workspace; searchable app selection; bounded browsing and transfer; create, rename, confirmed recursive delete, progress, cancellation, and read-only crash-report export |
| **Performance** | iPhone CPU/process/memory/energy data, Core Animation FPS, GPU memory, network rates, app activity, video-pipeline telemetry, service health, DVT network/thermal conditions, network PCAP, and Bluetooth HCI PCAP |
| **Device Logs** | On-demand structured Unified Log with SyslogRelay fallback, search, level filtering, pause, auto-scroll, copy, clear, bounded buffering, and recovery state |
| **Location** | DVT virtual-location set, numeric coordinate entry, built-in location presets, current state, and explicit restoration of the real device location |
| **Settings** | Language, always-on-top, system fullscreen, inspector visibility, display scale, mapping overlay, rotation-control lock, device-fullscreen toolbar behavior, decoder and pixel format, audio, clipboard sync, configurable performance HUD, updates, debug logging, and log-directory access |

System fullscreen and device fullscreen are different. System fullscreen changes the desktop window. Device fullscreen hides navigation and the inspector to give the phone picture and essential controls the available window area.

## Device Inspector

### Info

- Refreshes Lockdown identity, iOS/build versions, hardware model, storage, activation state, battery health and charging data.
- Renames the device through a paired Lockdown session and verifies the value.
- Shows Developer Mode and Developer Disk Image state; it can reveal the Developer Mode setting and explicitly mount, cancel, or unmount a compatible local image set.
- Lists paired Apple Watch metadata through CompanionProxy without controlling the Watch.
- Creates or resumes an unencrypted local MobileBackup2 backup, with progress, cancellation, and an optional forced full pass.
- Collects a bounded, cancellable CoreDevice sysdiagnose archive.
- Provides confirmed **Restart device** and **Shut down device** commands through Diagnostics Relay. Both intentionally terminate the current device session; shutdown requires manually turning the device on again.

Lock in the device toolbar is a hardware-button press/release toggle and can wake a locked device. The MCP `lock_device` tool is the separate one-way Diagnostics Relay sleep request and does not wake an already locked device.

### Apps

- Lists user apps and, on request, Apple default apps through CoreDevice AppService, with Installation Proxy fallback for the user-app catalog.
- Shows native icons, versions, signing type, removable state, reported storage, running state, and SpringBoard Dock/page/folder placement when available.
- Launches, restarts, stops, installs IPA files, and safely uninstalls eligible user apps. Operations are session-owned and report progress or failure.
- Opens Documents or the full container through House Arrest when iOS permits that scope, with bounded file and directory mutation and transfer.
- Associates an app with a saved key-mapping profile so launching it from the App list activates that profile.
- Explicitly starts and stops an installed developer-signed WebDriverAgent `.xctrunner`; DeviceHub Mask does not install or sign WDA.

### Profiles And Crashes

- Provisioning profiles are listed through Misagent. Local `.mobileprovision` installation validates CMS, UUID, size, and expiration; removal is confirmed and verified against a refreshed catalog.
- Crash reports are listed read-only through CrashReportCopyMobile and can be searched and exported. MCP can read a separately bounded text excerpt for agent diagnosis.

## Streaming And Input

| Area | Current behavior |
| --- | --- |
| Video | CoreDevice HEVC, up to 60 FPS; Native/FFmpeg decoding is the compatibility path and experimental WebCodecs decoding falls back automatically |
| Recording | Records the rendered canvas at up to 60 FPS through the system WebView's MediaRecorder and downloads MP4 or WebM; it stops on page or device changes and does not include the native device-audio output |
| Pixel format | RGB24 is the default; YUV420P is experimental and selectable unless an environment override is active |
| Audio | Optional CoreDevice AAC-ELD capture, native host playback, volume and mute; enabling capture requires reconnecting |
| Clipboard | One-shot Unicode paste always remains available; optional bidirectional text/image sync requires reconnecting |
| Touch | Direct mouse input and mapping output share a validated five-contact Universal HID report |
| Keyboard | Mapping mode and raw HID keyboard passthrough are mutually exclusive; losing focus, changing page/mode, fullscreen transitions, and disconnect release held input |
| Hardware buttons | Home, Lock, Volume Up/Down, Mute, Siri, and Action, plus profile-specific keyboard shortcuts |

## idevice Service Coverage

| Capability | Primary service |
| --- | --- |
| Device identity, name, storage fallback | Lockdown |
| Screen, audio, orientation, clipboard, HID | CoreDevice display, orientation, Pasteboard, and HID services |
| App list, process state, launch, stop | CoreDevice AppService |
| IPA installation and user-app fallback | Installation Proxy |
| App Documents/container | House Arrest and AFC |
| Public media files | Standard AFC / remote AFC shim |
| Battery and power actions | Diagnostics Relay |
| Developer Mode and image | AMFI and MobileImageMounter |
| Provisioning profiles | Misagent |
| Backup | MobileBackup2 |
| Sysdiagnose | CoreDevice DiagnosticsService |
| Device logs | OsTraceRelay / SyslogRelay |
| Performance and conditions | DVT Sysmontap, Graphics, Energy, Network Monitor, Notifications, and Condition Inducer |
| Virtual location | DVT Location Simulation |
| Network/Bluetooth capture | pcapd and BTPacketLogger |
| Watch metadata | CompanionProxy |
| Home-screen layout and app icons | SpringBoardServices |
| Crash reports | CrashReportCopyMobile |
| Semantic automation | WebDriverAgent and XCTest runner services |

## MCP Tool Coverage

The Streamable HTTP MCP endpoint exposes the following tools while the desktop app is running:

- Screen and input: `screenshot`, `tap`, `swipe`, `multi_touch`, `wait_for_frame`, `type_text`, `paste_text`, `press_key`, `press_button`, and `rotate`.
- Device/session: `status`, `device_details`, `list_devices`, `connect_device`, `reconnect_device`, `lock_device`, `wait_for_device_event`, `list_companion_devices`, and `home_screen_layout`.
- Apps and diagnosis: `list_apps`, `launch_app`, `stop_app`, `list_crash_reports`, `read_crash_report`, `performance_snapshot`, and `recent_device_logs`.
- Location and conditions: `set_location`, `clear_location`, `list_device_conditions`, `apply_device_condition`, and `clear_device_condition`.
- WDA: `wda_runner_status`, `wda_start`, `wda_stop`, `wda_status`, `wda_ui_tree`, `wda_find_elements`, and `wda_click`.

MCP currently exposes one-way device locking, but not device restart or shutdown. Restart and shutdown are available in the desktop Device Info tab and require an interactive confirmation. MCP also does not expose AFC mutation, backup, sysdiagnose, provisioning-profile mutation, packet capture, or Developer Disk Image mutation.

## Intentional Boundaries

- No device restore, erase, backup-password management, or background backup.
- No AFC2/root filesystem access and no traversal of symbolic links.
- No Apple Watch control or port forwarding.
- No automatic WDA installation/signing and no automatic Developer Disk Image download or version guessing.
- No automatic device conditions; every profile is explicitly selected and normal conditions must be restored after testing.
- No claim of 120 FPS screen streaming: the current negotiated and rendered pipeline is capped at 60 FPS.
- Wi-Fi and remote-service availability remains dependent on pairing, host discovery, Apple services, and iOS policy.
