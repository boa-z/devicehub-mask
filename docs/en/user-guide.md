# User Guide

[简体中文](../zh-CN/user-guide.md) | [Documentation](README.md)

## Workspaces

### Device

The Device page contains device selection, the aspect-ratio-preserving live
view, control mode, rotation, direct touch, hardware buttons, and the device
inspector. The inspector exposes Lockdown metadata, installed apps, IPA
installation and removal, and provisioning profiles.

The inspector's Info tab refreshes privacy-safe battery diagnostics through
Diagnostics Relay: charge level and state, cycle count, full-charge versus
design capacity, voltage/current, remaining time, and power-adapter rating.
Battery and adapter serial numbers are never returned by the private API.
It also queries Developer Mode through MobileImageMounter. When disabled, the
tab points to the required iPhone setting because DVT diagnostics and
performance services depend on it.

The stage toolbar always exposes Home, Lock, Volume Up, Volume Down, Mute, Siri,
and Action. Always-on-top, inspector visibility, and fullscreen controls are in
the title toolbar. Hiding the inspector gives the device view more room without
stretching it. Page and fullscreen transitions release all active input.
The active saved control profile is shown in the stage toolbar and can be
switched there without opening the mapping editor. It remains available in the
focused device-view toolbar.

### Keyboard Mapping

The mapping page contains the placement canvas, mapping inspector, hardware
shortcut bindings, and profile manager. It reports source and contain-fitted
display resolutions. The background can use the live stream or a correctly
oriented frozen screenshot; captured frames can be saved as PNG and remain
available for offline editing after disconnecting.

Drag controls on the canvas, edit their properties in the inspector, then save
the profile. The runtime supports at most five unique simultaneous contact
identities even when a profile stores more mappings.

### Application Settings

Settings contains appearance, language, window behavior, device audio, clipboard synchronization, automatic
update checking, a manual **Check now** command, the installed app version, and
the GitHub repository link. Device audio is disabled by default; enabling it
takes effect after reconnecting the device. Playback mute and volume apply
immediately, and mute is also available in the Device toolbar. Clipboard synchronization is also disabled
by default and takes effect after reconnecting. When enabled, text and images copied on either the host or
device replace the clipboard on the other side. A transient message identifies the direction and content
type; text previews are whitespace-collapsed and limited to 48 characters.

The Device Info tab includes confirmed **Restart device** and **Shut down
device** actions. Both end the current control session. Restart reconnects only
after iOS and the USB services are available again; a shut-down device must be
turned on manually.

### Performance

The Performance workspace combines device-side DVT telemetry with the desktop
video pipeline. Device metrics include system CPU, process count, Core
Animation FPS, GPU memory, aggregate network receive/send rates, and connections
observed during the last minute. The process-energy table breaks down the
relative Apple Instruments energy score for up to sixteen current CPU and
memory leaders into CPU, GPU, and network components. These scores are useful
for comparison over the same session but are not watts. Pipeline metrics include
source, decode and presentation rates, transport bandwidth, JPEG encoding time,
and frame age.
System CPU is the iPhone-wide percentage derived from DVT's aggregate
`CPU_TotalLoad` divided by the device's reported logical CPU count; it is not
desktop-host CPU usage or a per-process value.
Top Processes shows the union of the ten busiest CPU processes and ten largest
physical-memory processes. Switch between CPU and Memory sorting. Process CPU
uses the same logical-core normalization as system CPU, so 100% represents the
device's total processing capacity rather than one fully occupied core.

Sampling starts only while the Performance workspace is open and stops when it
is left, so monitoring does not add permanent device load. The service-health
section reports whether virtual location, system monitoring, graphics monitoring,
network monitoring, and energy monitoring are connecting, ready, recovering,
unavailable, or stopped. A service reconnect does not tear down video or input.

## Control Modes

Use the **Mapping / Keyboard passthrough** segmented control above the device
view.

- **Mapping** routes physical keys to touch mappings and hardware-button
  shortcuts.
- **Keyboard passthrough** disables those mappings and forwards HID key-down and
  key-up events to iOS. Modifiers, arrows, navigation keys, F1-F24, and the
  numeric keypad are supported. Focus the device view before typing.

`Ctrl+Shift+K` switches modes. Switching mode, changing page, losing window
focus, entering fullscreen, or disconnecting releases every touch, hardware
button, and keyboard usage to prevent stuck input.

Keyboard passthrough represents physical HID keys, not composed text. The text
tool in the Device toolbar writes up to 1,024 Unicode characters to the device
pasteboard and sends Cmd+V, so focus an editable field before using it. This
one-shot action works even when continuous clipboard synchronization is disabled.

## Direct Touch and Orientation

Pointer input is normalized inside the contain-fitted displayed frame. Black or
unused stage areas are excluded. Rotation uses one shared width/height scale, so
landscape and portrait touch coordinates stay aligned with the visible image.

Up to five typed contacts can be sent in one Universal HID report. Duplicate
identities and out-of-range coordinates are rejected by the Rust backend before
device dispatch.

## Profiles and scrcpy-mask Compatibility

Profiles are validated JSON files in the Tauri application data directory. You
can select, activate, create, duplicate, rename, delete, import, and export them.
The active profile cannot be deleted.

A profile can be associated with app bundle IDs. Launching an associated app
from the DeviceHub Mask app list activates that saved control profile. The app
list shows the current association and can associate an unassigned app with the
active profile or remove that association. Conflicting or cross-profile
associations must be resolved explicitly in the profile editor.

DeviceHub Mask imports and exports scrcpy-mask `0.0.1` JSON. All thirteen
controller types are preserved, including nested sequence positions, bindings,
release modes, timing, and script fields. Import compatibility does not imply
Android transport support.

Hardware shortcuts are stored with the profile. Click a shortcut field and
press a key to bind it; use Backspace or Delete to clear it. One key cannot be
shared by a touch mapping and a hardware button or by two hardware buttons.
Hold timing is preserved for controls such as Siri.

## Applications and Provisioning Profiles

The Apps tab lists applications through CoreDevice AppService and falls back to
Lockdown Installation Proxy when necessary. When AppService process data is
available, running apps are marked and can be stopped or restarted. It can also
select an IPA with the native file dialog, report installation stages, and
remove confirmed removable user apps.
App icons are read on demand from SpringBoardServices as their rows approach the
visible area. If an icon is unavailable, the list keeps its letter fallback and
all management actions remain usable.

The folder button on an application opens its **App Documents** workspace when
the application exposes Documents through iOS File Sharing. You can browse
folders, upload a new local file, download a regular file, create folders,
rename items, and delete files or empty folders. Uploads never overwrite an
existing name; rename the existing item or the local file first. House Arrest
confines every operation to that application's vended Documents root.

Uninstall authorization is checked again on the backend against current device
metadata. Switching devices or ending the session cancels an active install or
remove operation. IPA upload progress is indeterminate until Installation Proxy
begins reporting device-side installation percentages.

Provisioning profiles are read through Misagent and decoded as CMS SignedData.
The UI receives normalized metadata but not raw payloads or provisioned device
identifiers. Malformed profiles appear as individual error rows.

The Crashes tab refreshes and recursively lists reports exposed by
CrashReportCopyMobile. Search operates on report names and device paths. Export
uses the native save dialog and streams the selected regular file to the chosen
host path. Browsing is read-only: the app does not delete reports from the
device, does not send report contents to the WebView, and rejects traversal,
non-file entries, or reports larger than 128 MiB.

## Device Logs

The Device Logs workspace displays the active iPhone's live SyslogRelay stream.
It supports live search, pausing the display, automatic scrolling, copying the
visible result, and clearing the bounded in-memory buffer. Collection starts
only while this workspace is open and stops after leaving it. A warning appears
if high log volume causes older entries to leave the buffer.

## Language and Fonts

The UI supports Simplified Chinese (`zh-CN`) and English (`en-US`). First launch
uses Chinese for a `zh-*` system locale and English otherwise. Language changes
apply immediately and persist in `devicehub-mask.locale`.

Both the React UI and Ant Design use the native system font stack. Existing
profile names and user-authored labels are never rewritten when language
changes.

## Updates

Automatic nightly checks can be disabled in Settings. The manual check remains
available. An accepted update is downloaded, signature-verified, installed, and
followed by an application restart. See [Distribution](distribution.md) for
signing and release details.

## MCP Automation

While DeviceHub Mask is running, MCP clients can connect to the Streamable HTTP
endpoint at `http://127.0.0.1:8009/mcp`. The server exposes screenshots, taps,
swipes, simultaneous multi-touch, text and key input, hardware buttons, app
discovery, launch/restart and stop, rotation, device selection and reconnection,
virtual location, frame synchronization, and session status.

Use `type_text` for printable ASCII HID keystrokes. Use `paste_text` for CJK or
other Unicode text; it waits for the device pasteboard write and Cmd+V before
returning success.

Take a screenshot before sending coordinate-based input. Pass the returned
`image_width` and `image_height` to `tap`, `swipe`, or `multi_touch` so
coordinates remain correct when screenshots are resized. For latency-sensitive
gameplay, disable `wait_for_settle` on an action and pass its
`frame_version_after` to `wait_for_frame` before taking the next screenshot.
MCP reuses the desktop application's active device session; it does not open a
second connection to the phone.

For example, `multi_touch` can move a left-side joystick while holding a
right-side action button in the same 250ms HID gesture:

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

The endpoint has no authentication. Keep it on loopback unless the host is on a
trusted isolated network. Developers can change the bind address with
`DEVICEHUB_MCP_ADDR`; see [Development](development.md).
