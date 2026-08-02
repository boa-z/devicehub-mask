# DeviceHub Mask

[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/boa-z/devicehub-mask)
[![LINUX DO](https://shorturl.at/ggSqS)](https://linux.do)

简体中文 | [English](README.md)

DeviceHub Mask 是独立的开源项目。项目使用 Apple 的设备开发服务，但不隶属于 Apple、Xcode 或 Apple 的 Device Hub 产品。

DeviceHub Mask 用于在 macOS、Windows 和 Linux 上控制与检查已启用开发者模式的 iOS 设备。运行时依赖 Rust [idevice](https://github.com/jkcoxson/idevice) 提供底层设备服务和传输能力，当前目标设备需要运行 iOS/iPadOS 27 或更高版本。同一套 React 应用可以运行在 Tauri 2 桌面宿主或实验性 headless 服务上，并共用支持多设备的 Rust 运行时。项目提供 CoreDevice HEVC/WebCodecs 画面、Universal HID 输入、按键映射、设备/App/文件/诊断工作区和 MCP 自动化。

## 适用范围

DeviceHub Mask 面向需要从桌面、浏览器或本地 agent 检查、控制和自动化一台或多台 iPhone/iPad 的开发者与设备实验室使用者。它补充 Apple 的开发工具，不是 Xcode 的替代品，也不会安装、侧载、签名、注入或升级 iOS App。

## 产品形态

| 形态 | 用途 |
| --- | --- |
| Tauri 桌面端 | 日常原生应用，包含桌面音频、剪贴板、对话框、更新和私有 loopback 服务 |
| Headless 服务 | 在 loopback 或显式启用的可信 LAN 提供浏览器 UI 与认证 API |
| MCP | 面向 agent 的 loopback 接口，用于选择目标、截图、HID、App 流程、状态等待和诊断 |

运行时可同时保持多台设备连接。UI 选择只切换焦点，不销毁其他会话；API/MCP 客户端会解析明确设备目标。

DeviceHub Mask 明确不安装、侧载、签名、注入或升级 iOS App。该边界同样约束后续功能开发；请先使用专用签名部署工具，再在本项目中管理已有 App。

## 下载安装包

[Nightly 发布页](https://github.com/boa-z/devicehub-mask/releases/tag/nightly)提供可以直接运行的安装包：

- macOS Universal DMG
- Windows x64 NSIS 和 MSI 安装包
- Linux x64 和 ARM64 AppImage、DEB 安装包
- macOS、Windows 和 Linux Headless 归档

Nightly 是滚动的早期开发版本。使用归档前请校验相邻的 SHA-256 文件。当前 macOS Nightly 使用 ad-hoc 签名；如果 Gatekeeper 阻止启动，请参考[故障排查](docs/zh-CN/troubleshooting.md)。

普通用户请从[快速开始](docs/zh-CN/getting-started.md)了解安装包安装和设备准备。

## 从源码构建

安装 Rust stable、Node.js 22 或更高版本、FFmpeg 和平台原生依赖。连接、解锁并信任 iOS 设备，同时开启开发者模式。

```sh
git clone https://github.com/boa-z/devicehub-mask.git
cd devicehub-mask
npm ci
npm run tauri:dev
```

Windows 还需要 Apple Mobile Device Service、Visual Studio Build Tools、CMake 和 NASM，并执行一次：

```powershell
.\scripts\prepare-windows-device.ps1
```

Headless 开发启动方式：

```sh
npm run headless:dev -- --listen 127.0.0.1:8080
```

平台配置见[快速开始](docs/zh-CN/getting-started.md)，LAN/token 策略见 [Headless 服务](docs/zh-CN/headless.md)。

## 首次使用

启动后打开连接中心，选择已经完成认证的 USB 或 Wi-Fi 传输；首次建立会话时保持设备解锁。进入设备工作区即可查看画面并使用触控或硬件控制。如果要从其他主机通过浏览器访问，请使用 Headless 归档，并遵循 [Headless 服务](docs/zh-CN/headless.md)中的 token 和 LAN 规则。

## 相关项目

- [devicehub-mobile](https://github.com/boa-z/devicehub-mobile)：连接 DeviceHub Mask Headless/LAN 服务的 React Native 配套客户端。
- [devicehub-mask-keymaps](https://github.com/boa-z/devicehub-mask-keymaps)：公开的按键映射配置目录。
- [idevice](https://github.com/boa-z/idevice)：运行时使用的 Rust iOS 服务库。

## 文档

| 读者 | 从这里开始 |
| --- | --- |
| 桌面用户 | [文档首页](docs/zh-CN/README.md)，随后阅读[使用指南](docs/zh-CN/user-guide.md) |
| Headless/LAN 使用者 | [Headless 服务](docs/zh-CN/headless.md) |
| Agent 使用者 | [MCP 自动化指南](docs/zh-CN/mcp.md) |
| 开发者 | [架构说明](docs/zh-CN/architecture.md)和[开发与构建](docs/zh-CN/development.md) |

完整中英文文档位于[中文文档首页](docs/zh-CN/README.md)和 [English documentation](docs/en/README.md)。

## 状态与安全

项目仍处于活跃早期开发阶段。CoreDevice 是 Apple 提供的设备能力，会随 iOS、硬件、传输、主机准备和策略变化。配对成功不代表画面、HID、诊断或所有管理服务一定可用。

桌面服务保持 loopback。Headless LAN 模式必须显式启用并使用 token 认证，但没有内置 TLS、账号、角色或互联网安全边界。MCP 没有认证，应保持 loopback。

Nightly 安装包：[GitHub nightly release](https://github.com/boa-z/devicehub-mask/releases/tag/nightly)

## 验证

提交前运行与 CI 相同的源码门禁：

```sh
npm run verify
```

它检查文档、前端 lint/测试/构建、Rust 格式化/测试、Clippy 和 crate 边界，不运行真机测试。针对性、完整、打包和显式真机验证见[开发与构建](docs/zh-CN/development.md)。

## 致谢

按键映射交互模型参考 [scrcpy-mask](https://github.com/AkiChase/scrcpy-mask)，未使用 Android 传输代码。

## 许可证

Copyright (c) 2026 boa-z。DeviceHub Mask 仅以 [GNU Affero General Public License v3.0](LICENSE) 授权。通过网络提供修改版本时，必须以相同许可证向用户提供对应源代码。
