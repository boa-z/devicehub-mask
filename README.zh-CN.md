# DeviceHub Mask

简体中文 | [English](README.md)

DeviceHub Mask 是一个基于 Tauri 2 的 iOS 游戏桌面控制应用，支持 macOS、
Windows 和 Linux。项目将 CoreDevice 屏幕串流、Universal HID 控制与参考
[scrcpy-mask](https://github.com/AkiChase/scrcpy-mask) 设计的按键映射编辑器整合在一起。

本项目不使用 iPhone 镜像，也不使用 scrcpy 的 Android 传输层。内部 Axum 服务只
监听回环地址，每次启动使用独立令牌鉴权，不会作为网页应用对外暴露。

## 主要能力

- CoreDevice HEVC 实时画面，最高 60 FPS，并保持横竖屏比例
- 最多五个 Universal HID 并发触点、鼠标直接触控、键盘透传和可配置硬件按键快捷键
- 完整导入和导出 scrcpy-mask `0.0.1` 控制器配置，支持实时画面或截图背景的可视化编辑器
- 可编辑设备名称、设备信息与激活就绪状态、带真实图标的应用浏览与启动、IPA 安装、安全卸载、App Documents 文件管理、
  确认式设备重启/关机，以及描述文件检查、校验安装和确认移除
- 通过 CoreDevice Pasteboard Service 支持单次 Unicode 文本粘贴，并可选启用文本与图片双向同步
- 按需读取结构化 iPhone 统一日志，支持级别与上下文筛选、受监督 SyslogRelay 回退和有界缓冲
- 按需采集归一化 iPhone CPU、高负载进程 CPU/内存与相对能耗排行、Core Animation
  FPS、GPU 内存与设备网络速率，并监督 DVT 服务恢复；支持设备级 DVT 网络/热状态模拟，
  还可通过 pcapd 有界导出 PCAP
- 内置 Streamable HTTP MCP 服务，支持截图、低延迟多点触控、App 启动、帧同步、设备切换和 DVT 虚拟定位
- 原生 Tauri 2 桌面控件、中英文界面和签名 nightly 自动更新
- 使用 GitHub Actions 验证并打包 macOS、Windows 和 Linux 版本

## 快速开始

安装 Rust stable、Node.js 22 或更高版本、FFmpeg 以及当前平台所需的原生依赖。
连接 iOS 设备并解锁、信任电脑，同时启用开发者模式。

```sh
git clone https://github.com/boa-z/devicehub-mask.git
cd devicehub-mask
npm ci
npm run tauri:dev
```

Windows 还需要 Apple Mobile Device Service、Visual Studio Build Tools、CMake
和 NASM。首次连接前运行一次设备准备脚本：

```powershell
.\scripts\prepare-windows-device.ps1
```

完整的平台依赖和设备准备流程请查看[快速开始](docs/zh-CN/getting-started.md)。

应用运行时会在 `http://127.0.0.1:8009/mcp` 提供 MCP：

```sh
claude mcp add --transport http devicehub-mask http://127.0.0.1:8009/mcp
```

## 文档

| 主题 | 简体中文 | English |
| --- | --- | --- |
| 文档首页 | [中文文档](docs/zh-CN/README.md) | [English docs](docs/en/README.md) |
| 安装与首次运行 | [快速开始](docs/zh-CN/getting-started.md) | [Getting Started](docs/en/getting-started.md) |
| 应用工作流与控制 | [使用指南](docs/zh-CN/user-guide.md) | [User Guide](docs/en/user-guide.md) |
| 系统设计与协议 | [架构说明](docs/zh-CN/architecture.md) | [Architecture](docs/en/architecture.md) |
| 开发与本地构建 | [开发与构建](docs/zh-CN/development.md) | [Development](docs/en/development.md) |
| CI、发布与更新 | [发布与更新](docs/zh-CN/distribution.md) | [Distribution](docs/en/distribution.md) |
| 常见问题 | [故障排查](docs/zh-CN/troubleshooting.md) | [Troubleshooting](docs/en/troubleshooting.md) |

## 项目状态

实时画面、HID 控制、按键映射、应用管理和更新流程已经可用。具体设备和 iOS
版本仍取决于 Apple CoreDevice 服务是否开放。当前优先事项包括 Windows 视频管线
性能分析、扩展 Device Hub 设备管理能力，以及补齐 scrcpy-mask 运行时兼容性。

Nightly 安装包：[GitHub nightly release](https://github.com/boa-z/devicehub-mask/releases/tag/nightly)

## 验证

```sh
npm run lint
npm test
npm run build
cargo test --manifest-path src-tauri/Cargo.toml --locked
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --locked -- -D warnings
```

完整构建和打包检查请查看[开发与构建](docs/zh-CN/development.md)。

## 致谢

按键映射交互参考了 scrcpy-mask 的实时覆盖层、方向键、按键捕获和配置管理方式。
本项目未使用其 Android 传输代码。
