# 快速开始

简体中文 | [English](../en/getting-started.md) | [文档首页](README.md)

## 设备版本要求

当前版本要求 iPhone 或 iPad 运行 iOS/iPadOS 27 或更高版本。DeviceHub Mask 依赖 Rust [idevice](https://github.com/jkcoxson/idevice) 提供底层设备服务和传输能力；较低版本的 iOS 不是当前项目的支持目标。

## 选择运行方式

| 目标 | 推荐入口 | 是否需要 Rust/Node.js |
| --- | --- | --- |
| 直接使用桌面端 | [Nightly 安装包](https://github.com/boa-z/devicehub-mask/releases/tag/nightly) | 不需要 |
| 在没有桌面窗口的主机运行，并从浏览器访问 | [Headless 归档](headless.md) | 不需要 |
| 修改代码或参与开发 | 从源码构建 | 需要 |

普通用户优先使用发布页中的安装包。Nightly 是当前主要分发渠道；使用前请阅读项目的[状态与安全说明](../../README.zh-CN.md#状态与安全)。

## 使用发布版

从 [Nightly 发布页](https://github.com/boa-z/devicehub-mask/releases/tag/nightly)选择对应平台的文件，并同时下载相邻的 `.sha256` 文件。

### macOS

1. 下载 Universal DMG 并校验 SHA-256。
2. 打开 DMG，将应用拖入 Applications。
3. 首次启动时，如果系统提示无法验证开发者，请按[故障排查](troubleshooting.md#macos-提示无法验证应用是否包含恶意软件)中的步骤允许打开。
4. 启动应用后继续完成下方的[设备准备](#准备设备)。

### Windows

1. 下载 x64 NSIS 或 MSI 安装包并校验 SHA-256。
2. 安装 WebView2、Apple Mobile Device Service 和系统 HEVC 支持；普通用户不需要安装 Rust 或 Node.js。
3. 启动 DeviceHub Mask，并继续完成[设备准备](#准备设备)。

### Linux

AppImage 不需要安装到系统目录：

```sh
chmod +x ./devicehub-mask_<version>+<build>_amd64.AppImage
./devicehub-mask_<version>+<build>_amd64.AppImage
```

Debian/Ubuntu 可以安装 DEB：

```sh
sudo apt install ./devicehub-mask_<version>+<build>_amd64.deb
```

Linux 仍需要可用的 `usbmuxd` 和 Apple 配对环境。安装包不会安装或配置主机侧守护进程及配对记录，完整的 USB 信任流程见 [Linux USB 配对](headless.md#linux-usb-配对)。

### Headless

如果需要从浏览器访问无桌面主机，下载对应平台的 Headless 归档。必须保留归档内的可执行文件、`dist/`、sidecar、许可证和启动文档的相对位置；详细命令见 [Headless 服务](headless.md)。

## 从源码运行

### 源码构建要求

所有平台都需要：

- 已配对并信任电脑的 iPhone 或 iPad
- 在 iOS 版本要求时启用开发者模式
- Rust stable
- Node.js 22 或更高版本和 npm
- 使用 `tauri dev` 时可通过 `PATH` 或 `DEVICEHUB_FFMPEG` 找到 FFmpeg；安装包构建会 自动准备并内置经过校验的 FFmpeg

界面统一使用系统字体，不下载或捆绑 Web 字体。

### macOS

安装 Xcode Command Line Tools 和常用依赖：

```sh
xcode-select --install
brew install node ffmpeg rustup nasm
rustup-init
```

打开新终端并检查 `rustc`、`node`、`npm` 和 `ffmpeg`。

### Windows

Windows 10/11 需要 WebView2、Rust MSVC 工具链、带 **Desktop development with C++** 工作负载的 Visual Studio Build Tools、CMake、NASM 和 Apple Mobile Device Service。桌面版 iTunes 会安装 Apple 服务，并在 `127.0.0.1:27015` 提供 usbmuxd 端点。

实时视频要求 WebView2 通过 WebCodecs 暴露 HEVC。许多 Windows 系统需要 Microsoft HEVC Video Extensions；GPU 支持 HEVC 并不充分。应用不再包含 Native / FFmpeg 视频回退。

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

Python 3.12 只供准备脚本使用。CMake 和 NASM 是内置原生 sidecar 的构建依赖。只有在未先运行 `npm run ffmpeg:prepare` 而直接使用 `tauri dev` 时才需要系统 FFmpeg；FFmpeg 用于设备音频，不参与实时视频。首次启动前应在 iTunes 中连接并信任设备。

### Linux

Ubuntu 和 Debian 需要 Tauri WebKitGTK 和原生编译依赖：

```sh
sudo apt-get install build-essential cmake nasm pkg-config libssl-dev \
  libudev-dev libasound2-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev \
  librsvg2-dev patchelf ffmpeg
```

Linux 设备连接还需要正常工作的 `usbmuxd` 和 Apple 配对环境，设备覆盖程度低于 macOS 和 Windows。开始桌面端或 Headless 构建前，请先阅读 [Linux USB 配对](headless.md#linux-usb-配对)。

## 获取源码

```sh
git clone https://github.com/boa-z/devicehub-mask.git
cd devicehub-mask
npm ci
```

`npm ci` 会安装仓库内的 Tauri CLI，不需要全局安装 `cargo-tauri`。

## 准备设备

1. 通过 USB 连接设备。
2. 解锁并接受电脑信任提示。
3. 启用开发者模式。若设置中尚未显示该选项，请先连接一次，并使用设备信息警告中的 “在设置中显示”。
4. Windows 上运行一次 `./scripts/prepare-windows-device.ps1`。
5. 首次连接时保持设备解锁。
6. 关闭可能占用 CoreDevice 媒体会话的其他应用。

Windows 脚本会在 `%LOCALAPPDATA%\devicehub-mask\pymobiledevice3` 创建隔离环境， 挂载 Personalized Developer Disk Image，并通过 USB 检查 `com.apple.coredevice.displayservice`。脚本不需要管理员权限，也不需要常驻进程。 重启电脑或升级 iOS 后可能需要重新准备。

DeviceHub Mask 会将 USB 和 Wi-Fi 显示为同一设备的两个独立传输；旧版仅传入 UDID 的选择仍默认使用 USB。首次授权 Wi-Fi 发现时，请通过 USB 连接已解锁且受信任的设备。 App 会在自己的应用数据目录中保存一份私有配对记录（Unix 下目录权限为 `0700`、文件 权限为 `0600`），并用它验证 `_apple-mobdev2._tcp` Bonjour 记录。列表出现 **iPhone · Wi-Fi** 后，当前 iOS 版本在首次建立 Wi-Fi 控制连接时还会要求在已解锁的 设备上确认授权，并为 `_remotepairing._tcp` CoreDevice 隧道创建独立的 RemotePairing 凭据。授权完成且 Wi-Fi 会话启动后即可拔掉数据线。

如果设备后来拒绝这些 Wi-Fi 凭据，不要因为瞬时断连直接删除配对数据。请按定向的 [Wi-Fi 重新授权流程](troubleshooting.md#remote-pairing-验证出现-early-eof)处理：从已配对 USB 条目进入设备检查器，并通过应用清理两层信任凭据。

DeviceHub Mask 默认在所有平台使用内置的已认证 Bonjour 与 RemotePairing 路径。 `netmuxd` 保留为可选兼容提供者；设置 `DEVICEHUB_NETMUXD=/absolute/path/to/netmuxd` 可强制使用它。受监督的进程只监听 私有 loopback 端口并随 App 一同退出；DeviceHub Mask 不会替换或终止系统 usbmuxd。 设置 `DEVICEHUB_NETMUXD=off` 可明确保持使用内置路径。

较旧的 Apple 组件仍可能要求在 Finder 中启用“连接 Wi-Fi 时显示此 iPhone”。未经验证 的附近 Bonjour 设备不会作为可连接设备显示；状态栏会提示先完成一次 USB 授权。

## 首次运行

启动 Vite、Tauri、私有串流服务和自动重载：

```sh
npm run tauri:dev
```

在 `--` 后传入 UDID 可指定设备：

```sh
npm run tauri:dev -- -- 00008110-001624E2013A801E
```

开发模式在 Tauri WebView 内使用 `127.0.0.1:5173` 的 Vite。Vite 不代理设备 API，前端通过 Tauri IPC 获取随机端口和启动级鉴权令牌。

下一步：[使用指南](user-guide.md)或[开发与构建](development.md)。
