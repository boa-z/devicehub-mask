# Getting Started

[简体中文](../zh-CN/getting-started.md) | [Documentation](README.md)

## Requirements

All platforms require:

- A paired and trusted iPhone or iPad
- Developer Mode enabled when required by the iOS version
- Rust stable
- Node.js 22 or newer and npm
- For `tauri dev`, FFmpeg on `PATH` or through `DEVICEHUB_FFMPEG`; packaged
  builds prepare and include their own checksum-verified FFmpeg

The UI uses the native system font stack. No web font is downloaded or bundled.

### macOS

Install Xcode Command Line Tools and common dependencies:

```sh
xcode-select --install
brew install node ffmpeg rustup nasm
rustup-init
```

Open a new shell, then verify `rustc`, `node`, `npm`, and `ffmpeg`.

### Windows

Windows 10/11 requires WebView2, the Rust MSVC toolchain, Visual Studio Build
Tools with **Desktop development with C++**, CMake, NASM, and Apple
Mobile Device Service. The desktop iTunes package provides the Apple service and
the usbmuxd endpoint at `127.0.0.1:27015`.

The optional experimental Browser / WebCodecs video decoder may also require
Microsoft's HEVC Video Extensions. This is not required for the default native
decoder; the app probes support and falls back automatically.

```powershell
winget install --id Rustlang.Rustup --exact
winget install --id OpenJS.NodeJS.LTS --exact
winget install --id Kitware.CMake --exact
winget install --id NASM.NASM --exact
winget install --id 9NP83LWLPZ9K --source msstore
winget install --id Python.Python.3.12 --exact
rustup default stable-msvc
Get-Service "Apple Mobile Device Service"
```

Python 3.12 is used only by the preparation helper. CMake and NASM build the
bundled static libjpeg-turbo; a separate TurboJPEG DLL is not required at
runtime. Install a system FFmpeg only when using `tauri dev` without first
running `npm run ffmpeg:prepare`. Connect and trust the device once in iTunes.

### Linux

Ubuntu and Debian need the Tauri WebKitGTK and native build packages:

```sh
sudo apt-get install build-essential cmake nasm pkg-config libssl-dev \
  libudev-dev libasound2-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev \
  librsvg2-dev patchelf ffmpeg
```

Linux connectivity also requires a working usbmuxd and Apple pairing setup and
has less device coverage than macOS and Windows.

## Get the Source

```sh
git clone https://github.com/boa-z/devicehub-mask.git
cd devicehub-mask
npm ci
```

`npm ci` installs the repository-local Tauri CLI. A global `cargo-tauri` is not
required.

## Prepare the Device

1. Connect the device over USB.
2. Unlock it and accept the trust prompt.
3. Enable Developer Mode. If its Settings option is absent, connect once and
   use **Show in Settings** in the Device Info warning first.
4. On Windows, run `./scripts/prepare-windows-device.ps1` once.
5. Keep the device unlocked for the first connection.
6. Close other applications that may own the CoreDevice media session.

The Windows helper creates an isolated pymobiledevice3 runtime under
`%LOCALAPPDATA%\devicehub-mask\pymobiledevice3`, mounts the Personalized
Developer Disk Image, and checks for `com.apple.coredevice.displayservice` over
USB. It does not need elevation or a persistent helper process. Preparation may
need to be repeated after rebooting or upgrading iOS.

DeviceHub Mask lists USB and Wi-Fi as separate transports and defaults to USB
for legacy device selections. To authorize Wi-Fi discovery, connect the device
by USB once while it is unlocked and trusted. The app stores a private copy of
the pairing record in its application data directory (`0700` directory and
`0600` files on Unix), then authenticates `_apple-mobdev2._tcp` Bonjour records
before showing them. On current iOS versions, the first Wi-Fi control connection
also asks for approval on the unlocked device and creates separate RemotePairing
credentials for the `_remotepairing._tcp` CoreDevice tunnel. Keep USB connected
until that approval completes. After the Wi-Fi session starts, the cable can be
removed.

DeviceHub Mask uses its built-in authenticated Bonjour and RemotePairing path by
default on all platforms. `netmuxd` remains an optional compatibility provider;
set `DEVICEHUB_NETMUXD=/absolute/path/to/netmuxd` to force it. The supervised
process listens only on private loopback and is stopped with the app. DeviceHub
Mask never replaces or terminates the system usbmuxd. Set
`DEVICEHUB_NETMUXD=off` to explicitly keep the built-in path.

On older Apple stacks, enabling **Show this iPhone when on Wi-Fi** in Finder may
still be necessary. Unauthenticated nearby Bonjour devices are never exposed as
connectable devices; the status bar instead asks for the one-time USB setup.

## First Run

Start Vite, Tauri, the private stream service, and automatic reload:

```sh
npm run tauri:dev
```

Request a specific UDID by passing it after `--`:

```sh
npm run tauri:dev -- -- 00008110-001624E2013A801E
```

Development uses Vite at `127.0.0.1:5173` inside the Tauri WebView. Vite does
not proxy the device API. The frontend obtains the random authenticated backend
address through Tauri IPC.

Next: [User Guide](user-guide.md) or [Development](development.md).
