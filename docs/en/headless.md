# Headless Service

[简体中文](../zh-CN/headless.md) | [Documentation](README.md)

The experimental headless host runs the same device runtime, authenticated HTTP/WebSocket API, WebCodecs transport, and React UI as the Tauri desktop application. It does not link Tauri or Wry.

## Nightly Package

Download the archive for your platform from the [nightly release](https://github.com/boa-z/devicehub-mask/releases/tag/nightly), verify it against the adjacent `.sha256` file, and extract the complete directory. Keep `devicehub-headless`, `dist/`, FFmpeg, netmuxd, and the license files together.

From the extracted directory, start the loopback-only service:

```sh
./devicehub-headless
```

On Windows, run `devicehub-headless.exe`. Open the URL printed by the process. The temporary access token is carried in the URL fragment and removed from the address bar after browser bootstrap.

## LAN Access

LAN binding is always explicit:

```sh
./devicehub-headless --listen 0.0.0.0:8080 --allow-lan
```

Replace `127.0.0.1` in the printed URL with the server's LAN address before opening it on another computer. The built-in server currently provides token authentication but not TLS or user accounts. Do not expose it directly to the Internet. Use `--token-file` with a URL-safe token of at least 24 characters when clients must reconnect with a stable credential; on Unix, protect that file with mode `0600`.

Run `./devicehub-headless --help` for data-directory, frontend-directory, device-selection, sidecar, usbmuxd, and optional loopback MCP settings.

## Host Integration

The browser UI reads build information, capabilities, settings, and diagnostics from the authenticated headless API. Headless preferences are stored in `<data-dir>/settings.json`, and browser errors are forwarded to the service log. Browser fullscreen is supported. Desktop-only capabilities such as always-on-top windows, installer updates, native file dialogs, opening server directories, host clipboard synchronization, and device audio are disabled instead of invoking unavailable Tauri commands. Browser-native file transfer and audio transport are tracked as the next parity stages.

DeviceHub Mask does not install, sideload, sign, or upgrade iOS applications. This remains outside the desktop and headless product scope.
