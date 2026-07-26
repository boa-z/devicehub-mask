# Physical Device Regression

[简体中文](../zh-CN/device-regression.md) | [Documentation](README.md)

Use this checklist after runtime, transport, media, or host-boundary changes. Record the commit, device model, iOS version, UDID, transport, date, and result. Passing CI does not prove physical-device behavior.

## Automated Read-Only USB Checks

Connect exactly one unlocked, trusted device by USB. Obtain its UDID with `idevice_id -l`, then run:

```sh
npm run verify:device -- --udid <UDID>
```

The command fails closed when `idevice_id` is unavailable, the connected USB device count is not one, or the supplied UDID does not match. It does not launch the desktop application and runs only these read-only checks, serially:

- heartbeat acknowledgement
- device details and Developer Mode status
- native screenshot capture
- provisioning profile listing
- syslog read
- public AFC root listing
- installed App discovery and icon read
- sysmontap process schema and sample normalization

It intentionally excludes pairing and trust changes, Developer Image mounting, network capture, App lifecycle operations, restart and shutdown, AFC writes, and provisioning profile mutations.

## Manual Desktop Regression

Only launch the current source-built application when the test operator explicitly authorizes it. Confirm the executable path first; do not use an installed release application by accident.

### USB Session and Media

- Connect the expected device over USB and confirm its model, iOS version, and UDID.
- Confirm WebCodecs receives, decodes, and presents HEVC frames without falling back to a native video decoder.
- Leave the device on a static screen and confirm it does not show a false video-stall or reconnect prompt.
- Confirm device audio is audible and that mute and volume controls work.
- Reconnect explicitly and confirm video, audio, input, and read-only services recover.

Relevant logs include `selected CoreDevice transport`, `selected video decoder backend decoder_backend=Browser`, keyframe receipt, browser presentation metrics, audio RTP detection, and session reconnect transitions.

### Input

- Verify tap, press-and-hold, drag, and two-contact multitouch.
- Verify keyboard mapping presses only the intended mapping and releases cleanly.
- Verify Home, volume, lock, and other exposed hardware buttons.

### Apps and AFC

- Load the App list and icons, then launch, stop, and restart an existing App.
- Start and stop an application console and confirm output belongs to the selected App.
- List and read AFC content. Perform write and cancellation checks only when the operator explicitly authorizes modifying test data.

### Wi-Fi Continuity

- Complete USB authorization, then confirm the same physical device appears as Wi-Fi-capable.
- Remove the cable and confirm the active session continues or reconnects over Wi-Fi.
- Interrupt Wi-Fi briefly and confirm supervised reconnection restores video, audio, input, Apps, and AFC.
- Confirm USB and Wi-Fi discovery entries resolve to the expected UDID and never create simultaneous sessions for the same physical device.

## Evidence Record

Record each run in the issue, pull request, or release notes using this minimum template:

```text
Commit:
Date:
Device model:
iOS version:
UDID fingerprint:
Transport: USB / Wi-Fi
Automated read-only checks:
WebCodecs:
Audio:
Input:
Apps:
AFC:
Reconnect:
Relevant log path or excerpt:
Result and failures:
```
