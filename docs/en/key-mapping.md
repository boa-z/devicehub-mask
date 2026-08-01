# Key Mapping Guide

[简体中文](../zh-CN/key-mapping.md) | [Documentation](README.md) | [User Guide](user-guide.md)

DeviceHub Mask translates desktop keyboard and pointer input into Universal HID touch contacts on the connected iPhone or iPad. A saved profile contains touch mappings, iOS hardware-button shortcuts, and optional application associations. This guide describes the behavior implemented by the current runtime, including compatibility fields that can be imported and exported but are not executed yet.

## Before You Start

- Connect an unlocked, trusted device and confirm that the live picture accepts direct pointer input.
- Open **Key Mapping** from the main navigation. Use the Mapping inspector visibility button in the title bar when more canvas space is needed.
- Author a profile in the orientation used by the game. Normalized positions survive display scaling, but a portrait layout does not automatically become a useful landscape layout.
- Keep the application window focused while testing physical keys. Input is released whenever the window loses focus, the page or input mode changes, fullscreen changes, or the device disconnects.

## First Working Profile

1. Select an existing profile or create one. Profile names are 1-80 characters and may contain only letters, numbers, `_`, and `-`.
2. Leave **Edit** enabled. Right-click the exact target on the device picture and choose **Single tap**, or use the add button in the inspector and move the new controller into place.
3. Select the controller. Click **Keyboard binding**, then press the desired physical key or chord. The editor stores `KeyboardEvent.code`, so the binding identifies a physical key position rather than a composed character.
4. Set a Contact ID from `0` to `4`. New and duplicated mappings choose a least-used identity, but verify it when actions may overlap.
5. Select **Save**. Saving writes the profile but does not activate an inactive profile.
6. Select **Set active**, or switch to the saved profile from the Device workspace profile selector.
7. Disable **Edit** before testing on the Key Mapping page. While editing is enabled, keyboard mappings and direct touch are intentionally suppressed so editing cannot operate the device.

Saving an active profile updates the current runtime immediately. For an inactive profile, save first and activate second. Switching profiles from the Device workspace always loads the saved copy, so unsaved editor changes are not carried across a switch.

## Canvas and Background

The editor shows both the source frame size and the contain-fitted viewport size. Mapping positions are stored as normalized `0..1` coordinates relative to the visible device picture, excluding unused stage space.

- **Live** uses the current WebCodecs picture and requests video only while this background is visible.
- **Screenshot** freezes the current decoded canvas. It remains available for offline editing during the current application run after the device disconnects.
- **Capture** replaces the frozen frame. **Save screenshot** writes the current live or frozen canvas as PNG; a saved PNG cannot currently be loaded back as an editor background.
- **Show guides** displays Swipe and MultipleTap paths plus configured control radii. The selected controller's guide remains visible when global guides are disabled.
- Drag a controller node to move its primary point. Use percentage fields for exact coordinates. Sequence points and cast centers are edited in the inspector.

Rotation changes the frame dimensions and coordinate transform. Use separate profiles when a game has materially different portrait and landscape layouts.

## Bindings and Chords

A button binding can contain one key or a chord. Every key in a chord must be held before the mapping becomes active. Modifier keys are captured with their left/right physical identity when the browser reports it. Backspace or Delete clears the focused binding field.

Do not assign the same physical key to multiple touch mappings. The runtime gives that key to the first active mapping in profile order and suppresses later mappings. Touch keys also cannot be reused by a hardware-button shortcut, and one hardware shortcut cannot be reused by another hardware button; the editor and backend reject those hardware conflicts.

The runtime mapping handler ignores keys while focus is inside buttons, fields, selectors, or other form controls; binding fields themselves still capture the key being assigned. Click the device surface before runtime testing. `Ctrl+Shift+K` is reserved for input-mode switching and should not be assigned to a controller.

## Inspector Fields

| Field | Meaning |
| --- | --- |
| Name | User-visible overlay and list label; it does not affect dispatch |
| Type | Controller behavior; changing it rebuilds the controller and resets incompatible settings |
| Position / Cast center | X/Y percentages in the current oriented source picture |
| Contact ID | Universal HID identity `0..4`; simultaneous contacts must be unique |
| Keyboard binding | One physical key or an all-keys-required chord |
| Direction bindings | Independent Button chords for up, down, left, and right; imported JoyStick axes are read-only compatibility data |
| Radius / range | Source-frame pixels, converted to normalized coordinates at runtime; DirectionPad has independent horizontal and vertical ranges |
| Duration / interval / wait | Milliseconds used by taps, repeats, swipes, sequences, or stored Script metadata |
| Sensitivity | Pointer delta multiplier relative to source-frame width or height |
| Release mode | Imported and editable cast metadata; only ordinary release-on-binding-up behavior is reliable in the current runtime |
| Sequence | Ordered normalized points; MultipleTap also stores each point's duration and preceding wait |
| Script fields | Compatibility text retained in the profile and export; never executed by this version |

Changing a field edits the in-memory profile. Select **Save** to persist it. The type selector can rebuild a controller as another type after confirmation. The conversion keeps its ID, name, position, and compatible binding and contact fields, while target-specific settings start from valid defaults. Incompatible data such as swipe paths, cast-only options, FPS touch modes, or Script text is discarded. Legacy Button and Direction pad mappings can be upgraded to Single tap and Direction pad through the same selector.

Select the folder button in the profile toolbar to open the desktop-local directory containing saved key mapping JSON files. The application creates the directory first when necessary. Files exported through the download menu are separate copies in the user-selected download location.

The profile toolbar provides Undo and Redo for unsaved editor changes. `Ctrl+Z` / `Cmd+Z` undo from the device surface; `Ctrl+Shift+Z`, `Cmd+Shift+Z`, and `Ctrl+Y` redo. Text fields keep their native editing shortcuts while focused, so use the toolbar buttons to undo the complete profile change from inside a field. Consecutive typing and controller dragging are coalesced into useful history steps. Loading another profile resets the history so changes never cross profile boundaries.

## Contact IDs and Simultaneous Input

Universal HID reports contain at most five contacts with identities `0` through `4`. Profiles may store more than five mappings, and mappings that can never overlap may reuse an identity. If two active mappings claim the same identity, only the first one in profile order owns the contact and receives the active highlight. Direct pointer input also consumes an available identity while held.

For reliable combinations such as movement plus two skills, assign different IDs to every action that can be held simultaneously. A dual-contact field imported for an FPS controller is preserved for compatibility but the current FPS runtime emits only its primary contact.

## Controller Reference

| Controller | Current runtime behavior | Important limits |
| --- | --- | --- |
| Single tap | Holds one contact while the complete binding remains pressed, for at most `duration` | Releasing early releases the contact; holding the key does not repeat the tap |
| Press and hold | Starts touching on key-down and releases at the same point on key-up | The screen touch lasts exactly as long as the complete keyboard binding is held |
| Repeat tap | Alternates contact down for `duration` and up for `interval` while the binding remains held | Use positive intervals; device and game timing still determine acceptance |
| Multiple tap | Runs the ordered points once, applying each point's wait and duration | It does not loop while held after the sequence ends |
| Swipe | Interpolates through the ordered points over `duration` | The final point remains held until the binding is released |
| Direction pad | Converts Button up/down/left/right chords into a normalized diagonal-safe drag | Imported JoyStick axis bindings are preserved but not evaluated |
| Mouse cast spell | Holds the primary contact and moves it with pointer deltas using horizontal/vertical scale | Advanced release, center, radius, randomization, and script-hook semantics are not fully executed |
| Pad cast spell | Holds the primary binding and offsets the contact with Button direction bindings | JoyStick axes, release-mode, blocking, randomization, and script hooks are compatibility-only |
| Cancel cast | Releases currently held MouseCastSpell and PadCastSpell bindings | It emits no contact; use one key because any member of a chord can trigger this special action |
| Observation | Moves a held contact with pointer deltas using X/Y sensitivity | `max_radius`, randomization, and script hooks are not enforced by the runtime |
| FPS camera | Moves a held contact with pointer deltas using X/Y sensitivity | Pointer lock, max offsets, interval, and dual-contact strategies are not implemented |
| Fire | Moves a held contact with pointer deltas using X/Y sensitivity | `preserve_fps_control`, randomization, and script hooks are not implemented |
| Raw input | Releases mapped input and switches the application to Keyboard passthrough | It emits no contact; use one key because any member of a chord can trigger this special action |
| Script | Stores and round-trips pressed, held, and released script text | Script execution is not implemented; do not use it for active control |

Pointer-driven controllers use ordinary WebView pointer movement and do not currently capture an infinite relative pointer. Movement stops at window boundaries. Imported random offsets, script hooks, and other fields not listed as active above remain data compatibility fields rather than an execution promise.

## Hardware Button Shortcuts

Open **Hardware button shortcuts** in the mapping inspector to bind Home, Lock, Volume Up, Volume Down, Mute, Siri, or Action. A shortcut sends distinct button-down and button-up events, so hold duration is retained for controls such as Siri. Clear a shortcut with Backspace or Delete.

Hardware shortcuts belong to the profile. The Device toolbar always exposes the same seven commands independently of shortcut bindings. Losing focus or changing modes releases every held hardware button.

## Edit, Mapping, and Keyboard Modes

- **Edit on**: move and configure controllers; mapped input and direct device touch are disabled.
- **Edit off + Mapping**: physical keys run the active profile, hardware shortcuts work, and pointer input directly touches the device.
- **Keyboard passthrough**: touch mappings and hardware shortcuts are disabled; supported physical HID keyboard down/up events are sent to iOS.

Use `Ctrl+Shift+K` to switch between Mapping and Keyboard passthrough. RawInput performs the same switch when its binding is pressed. Printable HID input is not composed Unicode text; use the Device toolbar paste action for CJK and other Unicode text.

## Profile Management

Profiles are validated JSON files in the application data directory. The profile toolbar supports save, activate, create, duplicate, rename, delete, app association, import, export, and browsing the online catalog.

- A new profile is empty. A duplicate copies mappings and hardware shortcuts but intentionally clears App associations.
- Renaming preserves mappings and associations. Renaming the active profile keeps it active.
- The active profile cannot be deleted. Activate another saved profile first.
- A profile may contain at most 512 mappings and 32 unique valid Bundle IDs. Any profile with App associations must also store one explicit target frame resolution.
- Imported profiles are saved and opened for editing but are not automatically activated.

## Online Keymap Catalog

Select **Browse online keymaps** in the profile toolbar to view the official catalog. The catalog is a download source, not a profile-sync service: its files never replace an existing local profile, and all editing, saving, activation, and offline use continue to use the local profile directory.

- **Refresh** fetches the current catalog over HTTPS and saves a validated local cache. If refresh fails, the last valid cache is shown when available.
- Select **Repository** to set a different HTTPS catalog JSON address, or restore the official address. The selected address is stored locally and gets its own cache; it never changes or synchronizes the user's local profiles.
- Search by keymap name, Bundle ID, or device target. Choose an installed App Bundle ID and a device target from the current device or the catalog's declared targets to narrow results. Choosing a device target requires the catalog item's exact stream width, height, orientation, and optional Apple product type to match; choose **All catalog devices** to browse without a device filter.
- Selecting **Download** verifies the published byte count and SHA-256, validates the native `version: 2` profile, writes a new local profile name without overwriting an existing one, and opens that profile in the editor.
- A download is never activated automatically. Review the mappings, save any changes, and select **Set active** when ready.

The built-in source is the public [DeviceHub Mask Keymaps repository](https://github.com/boa-z/devicehub-mask-keymaps). Custom addresses must be direct HTTPS catalog-document URLs without embedded credentials. A live catalog entry is a compatibility suggestion, not a guarantee that a game UI has not changed; verify mappings before use.

## Associate a Profile with an App

Open **Associate apps** and add exact Bundle IDs such as `com.example.game`. The editor also stores the current device frame width and height; select **Use current size** to replace it, then save. The association action on an App row uses the current live frame size.

Launching that App from DeviceHub Mask activates a saved profile only when both its Bundle ID and target frame width and height exactly match the current stream. The same Bundle ID can therefore select separate iPhone and iPad profiles. A conflict is reported only when multiple profiles repeat both the Bundle ID and target resolution. Launches performed directly on the device or by another program do not trigger this desktop workflow. The Device workspace profile selector can always switch the saved active profile manually.

The native profile format is now `version: 2`. This early-development project does not load or migrate `version: 1` profiles; move old files out of the profile directory and recreate them or import them again from a supported source.

## Import Formats

Choose the source explicitly in the import dialog; format guessing is intentionally disabled.

| Source | Accepted input | Conversion behavior |
| --- | --- | --- |
| DeviceHub Mask | Profile `version: 2` JSON, up to 4 MiB | Preserves mappings, hardware shortcuts, Bundle IDs, and target resolution; backend validation runs before saving |
| scrcpy-mask | `0.0.1` JSON with `original_size`, up to 4 MiB | Normalizes coordinates, converts Super to Meta key names, preserves all 13 recognized controller records and nested fields; hardware shortcuts and App associations start empty |
| PlayCover | XML `2.0.0` `.playmap`, up to 1 MiB | Converts keyboard buttons to SingleTap, draggable buttons to MouseCastSpell, and keyboard joysticks to DirectionPad; stores its Bundle ID association with the current frame size at import time |

PlayCover mouse areas, unsupported or negative mouse/controller codes, malformed positions, and incomplete joystick bindings are skipped and counted in the result. The parser rejects XML entities and nonstandard document declarations and enforces nesting, node, and model limits.

The imported profile name is derived from the file name, sanitized, limited to 80 characters, and given an `-import-N` suffix when needed.

## Export and Portability

The export menu has two different contracts:

- **DeviceHub Mask** exports the complete profile, including hardware shortcuts and Bundle ID associations. Use this for backup or transfer between DeviceHub Mask installations.
- **scrcpy-mask** exports `0.0.1` mapping JSON using the editor's current source dimensions. It carries controller mappings but not DeviceHub Mask hardware shortcuts or App associations.

PlayCover export is not implemented. Import/export compatibility does not add scrcpy Android transport or PlayCover controller/mouse input sources.

## Troubleshooting

| Symptom | Check |
| --- | --- |
| A key does nothing | Confirm Mapping mode, disable Edit, focus the application, save and activate the intended profile, and verify the binding is not empty |
| The overlay highlights but the game does not react | Check duration, target position, Contact ID ownership, the five-contact limit, and whether another earlier mapping uses the same key |
| Several controls seem related to one key | Remove duplicate physical-key bindings; only the first active mapping owns a reused key |
| One of two simultaneous actions disappears | Give them different Contact IDs and leave capacity for direct pointer input |
| Coordinates are wrong after rotation | Edit in the game's current orientation or maintain separate portrait and landscape profiles |
| An imported controller is visible but ineffective | Consult the controller table; some scrcpy-mask fields and Script/JoyStick behaviors are preserved only for round-trip compatibility |
| PlayCover reports skipped mappings | Mouse areas, unsupported key codes, invalid positions, and incomplete joysticks have no safe equivalent and are intentionally skipped |
| Input appears stuck | Release the keys, change mode or page, or refocus the window; each transition sends a full release. Reconnect if the device session itself ended |

When reporting a reproducible runtime problem, include the profile export, controller type, device orientation, source frame dimensions, the exact physical key codes shown in the editor, and a Debug log captured only for the reproduction window. Profiles can reveal application choices and control habits, so review them before sharing.
