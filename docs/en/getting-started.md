# Getting Started

[简体中文](../zh-CN/getting-started.md) | [Documentation](README.md)

## Device Requirement

The current target requires an iPhone or iPad running iOS/iPadOS 27 or newer. DeviceHub Mask depends on the Rust [idevice](https://github.com/jkcoxson/idevice) library for the underlying device services and transport capabilities. Older iOS versions are not a supported target for the current project.

## Choose an Entry Point

| Goal | Recommended entry point | Rust/Node.js required? |
| --- | --- | --- |
| Use the desktop application directly | [Nightly packages](https://github.com/boa-z/devicehub-mask/releases/tag/nightly) | No |
| Run without a desktop window and use a browser | [Headless archive](headless.md) | No |
| Change code or contribute | Build from source | Yes |

Most users should start with a package from the release page. Nightly is the current primary distribution channel; read the project's [status and security notes](../../README.md#status-and-security) before using it.

## Use a Release Package

Choose the file for your platform on the [Nightly release page](https://github.com/boa-z/devicehub-mask/releases/tag/nightly), and download the adjacent `.sha256` file as well.

### macOS

1. Download the Universal DMG and verify its SHA-256 checksum.
2. Open the DMG and drag the application to Applications.
3. On first launch, if macOS says it cannot verify the developer, follow [Troubleshooting](troubleshooting.md#macos-cannot-verify-the-app-is-free-of-malware) to allow the app to open.
4. Launch the app and continue with [Prepare the Device](#prepare-the-device) below.

### Windows

1. Download the x64 NSIS or MSI installer and verify its SHA-256 checksum.
2. Install WebView2, Apple Mobile Device Service, and system HEVC support; Rust and Node.js are not required for package users.
3. Launch DeviceHub Mask and continue with [Prepare the Device](#prepare-the-device).

### Linux

An AppImage does not need to be installed into a system directory:

```sh
chmod +x ./devicehub-mask_<version>+<build>_amd64.AppImage
./devicehub-mask_<version>+<build>_amd64.AppImage
```

Debian/Ubuntu can install the DEB package:

```sh
sudo apt install ./devicehub-mask_<version>+<build>_amd64.deb
```

Linux still requires a working `usbmuxd` and Apple pairing environment. A package does not install or configure the host-side daemon or pairing record. Follow [Linux USB Pairing](headless.md#linux-usb-pairing) for the complete USB trust flow.

### Headless

To use a browser on a host without a desktop window, download the matching Headless archive. Keep the executable, `dist/`, sidecars, licenses, and startup guides in their relative locations inside the extracted archive; see [Headless Service](headless.md) for the complete commands.

## Build from Source

### Source Build Requirements

All platforms require:

- A paired and trusted iPhone or iPad
- Developer Mode enabled when required by the iOS version
- Rust stable
- Node.js 22 or newer and npm
- For `tauri dev`, FFmpeg on `PATH` or through `DEVICEHUB_FFMPEG`; packaged builds prepare and include their own checksum-verified FFmpeg

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

Windows 10/11 requires WebView2, the Rust MSVC toolchain, Visual Studio Build Tools with **Desktop development with C++**, CMake, NASM, and Apple Mobile Device Service. The desktop iTunes package provides the Apple service and the usbmuxd endpoint at `127.0.0.1:27015`.

Live video requires WebView2 to expose HEVC through WebCodecs. On many Windows systems this requires Microsoft's HEVC Video Extensions; GPU HEVC support alone is not sufficient. The app no longer includes a Native / FFmpeg video fallback.

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

Python 3.12 is used only by preparation helpers. CMake and NASM are build-time dependencies for bundled native sidecars. Install a system FFmpeg only when using `tauri dev` without first running `npm run ffmpeg:prepare`; FFmpeg is used for device audio, not live video. Connect and trust the device once in iTunes.

### Linux

Ubuntu and Debian need the Tauri WebKitGTK and native build packages:

```sh
sudo apt-get install build-essential cmake nasm pkg-config libssl-dev \
  libudev-dev libasound2-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev \
  librsvg2-dev patchelf ffmpeg
```

Linux connectivity also requires a working `usbmuxd` and Apple pairing setup and has less device coverage than macOS and Windows. See [Linux USB Pairing](headless.md#linux-usb-pairing) before starting a desktop or Headless build.

## Get the Source

```sh
git clone https://github.com/boa-z/devicehub-mask.git
cd devicehub-mask
npm ci
```

`npm ci` installs the repository-local Tauri CLI. A global `cargo-tauri` is not required.

## Prepare the Device

1. Connect the device over USB.
2. Unlock it and accept the trust prompt.
3. Enable Developer Mode. If its Settings option is absent, connect once and use **Show in Settings** in the Device Info warning first.
4. On Windows, run `./scripts/prepare-windows-device.ps1` once.
5. Keep the device unlocked for the first connection.
6. Close other applications that may own the CoreDevice media session.

The Windows helper creates an isolated diagnostic runtime under `%LOCALAPPDATA%\devicehub-mask\pymobiledevice3` and checks the current Developer Disk Image and `com.apple.coredevice.displayservice` state over USB. It does not mount an image. Import and mount the matching set through DeviceHub Mask as described below. The helper does not need elevation or a persistent process.

### Developer Disk Images

DeviceHub Mask manages Developer Disk Images as validated sets and never accepts host file paths from HTTP clients. Open **Device > Device Info > Developer Disk Images** to refresh the catalog, import a set, mount it, or remove a managed import. Apple images are not bundled or redistributed with DeviceHub Mask.

On macOS, refresh discovers Personalized DDI bundles under `/Library/Developer/DeveloperDiskImages/iOS_DDI`, the active `xcode-select -p` developer directory, and installed `Xcode*.app` bundles. Xcode-owned entries are read-only. The desktop **Settings > Developer Disk Image** section can add up to 16 custom local directories on any platform; each selected directory and at most 256 immediate child directories are scanned for complete flat or Xcode-layout image sets. Custom entries are read-only and paths never become mount API parameters. On every platform, import one complete set:

- iOS 16 and earlier: one `.dmg` and its `.signature`.
- iOS 17 and later: one `.dmg`, its `.trustcache`, and the source bundle's `BuildManifest.plist`.

Imports are copied into the host data directory under `developer-images/<set-id>/`. Filenames, regular-file status, sizes, manifest identities, and image/trust-cache associations are validated before the atomic import becomes visible. For Xcode bundles with several hardware variants, the runtime selects the pair referenced by the connected device's `BoardId` and `ChipID`; directory ordering is never used.

The **Settings > Developer Image mount policy** controls session startup. **Manual** never starts a mount, **Ask** exposes missing-image state for an explicit user action, and **Automatic** mounts a compatible catalog set after a device session connects. Automatic mode also applies to Headless and MCP-driven sessions and may contact Apple's TSS service for personalization.

DeviceHub Mask lists USB and Wi-Fi as separate transports and defaults to USB for legacy device selections. To authorize Wi-Fi discovery, connect the device by USB once while it is unlocked and trusted. The app stores a private copy of the pairing record in its application data directory (`0700` directory and `0600` files on Unix), then authenticates `_apple-mobdev2._tcp` Bonjour records before showing them. On current iOS versions, the first Wi-Fi control connection also asks for approval on the unlocked device and creates separate RemotePairing credentials for the `_remotepairing._tcp` CoreDevice tunnel. Keep USB connected until that approval completes. After the Wi-Fi session starts, the cable can be removed.

If the device later rejects those Wi-Fi credentials, do not delete pairing data for a transient disconnect. Follow the targeted [Wi-Fi reauthorization procedure](troubleshooting.md#remote-pairing-verification-ends-with-early-eof), which starts from the paired USB entry and clears both trust layers through the Device inspector.

DeviceHub Mask uses its built-in authenticated Bonjour and RemotePairing path by default on all platforms. `netmuxd` remains an optional compatibility provider; set `DEVICEHUB_NETMUXD=/absolute/path/to/netmuxd` to force it. The supervised process listens only on private loopback and is stopped with the app. DeviceHub Mask never replaces or terminates the system usbmuxd. Set `DEVICEHUB_NETMUXD=off` to explicitly keep the built-in path.

On older Apple stacks, enabling **Show this iPhone when on Wi-Fi** in Finder may still be necessary. Unauthenticated nearby Bonjour devices are never exposed as connectable devices; the status bar instead asks for the one-time USB setup.

## First Run

Start Vite, Tauri, the private stream service, and automatic reload:

```sh
npm run tauri:dev
```

Request a specific UDID by passing it after `--`:

```sh
npm run tauri:dev -- -- 00008110-001624E2013A801E
```

Development uses Vite at `127.0.0.1:5173` inside the Tauri WebView. Vite does not proxy the device API. The frontend obtains the random authenticated backend address through Tauri IPC.

Next: [User Guide](user-guide.md) or [Development](development.md).
