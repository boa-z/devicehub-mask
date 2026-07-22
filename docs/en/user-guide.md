# User Guide

[简体中文](../zh-CN/user-guide.md) | [Documentation](README.md)

## Workspaces

### Device

The Device page contains device selection, the aspect-ratio-preserving live
view, control mode, rotation, direct touch, hardware buttons, and the device
inspector. The inspector exposes Lockdown metadata, installed apps, IPA
installation and removal, and provisioning profiles.

The stage toolbar always exposes Home, Lock, Volume Up, Volume Down, Mute, Siri,
and Action. Always-on-top, inspector visibility, and fullscreen controls are in
the title toolbar. Hiding the inspector gives the device view more room without
stretching it. Page and fullscreen transitions release all active input.

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

Settings contains appearance, language, window behavior, automatic update
checking, a manual **Check now** command, the installed app version, and the
GitHub repository link.

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

Keyboard passthrough represents physical HID keys, not composed text. CJK IME
and arbitrary text composition should use clipboard synchronization until a
dedicated text input path is implemented.

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
Lockdown Installation Proxy when necessary. It can launch apps, select an IPA
with the native file dialog, report installation stages, and remove confirmed
removable user apps.

Uninstall authorization is checked again on the backend against current device
metadata. Switching devices or ending the session cancels an active install or
remove operation. IPA upload progress is indeterminate until Installation Proxy
begins reporting device-side installation percentages.

Provisioning profiles are read through Misagent and decoded as CMS SignedData.
The UI receives normalized metadata but not raw payloads or provisioned device
identifiers. Malformed profiles appear as individual error rows.

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
swipes, text and key input, hardware buttons, rotation, device selection and
reconnection, virtual location, and session status.

Take a screenshot before sending coordinate-based input. Pass the returned
`image_width` and `image_height` to `tap` or `swipe` so coordinates remain
correct when screenshots are resized. MCP reuses the desktop application's
active device session; it does not open a second connection to the phone.

The endpoint has no authentication. Keep it on loopback unless the host is on a
trusted isolated network. Developers can change the bind address with
`DEVICEHUB_MCP_ADDR`; see [Development](development.md).
