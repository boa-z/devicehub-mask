# DeviceHub Mask 文档

简体中文 | [English](../en/README.md) | [项目说明](../../README.zh-CN.md)

这些文档按任务拆分，不需要按顺序阅读。

## 指南

- [快速开始](getting-started.md)：平台依赖、获取源码、设备准备和首次开发运行
- [使用指南](user-guide.md)：设备、映射、应用、键盘、硬件按键、截图、语言和更新工作流
- [功能参考](features.md)：当前桌面工作区、设备管理操作、idevice 服务覆盖、MCP 工具和功能边界
- [架构说明](architecture.md)：进程边界、私有传输、CoreDevice 会话、视频管线、HID 校验和数据所有权
- [开发与构建](development.md)：仓库结构、环境变量、验证、本地生产构建和各平台打包
- [发布与更新](distribution.md)：GitHub Actions、nightly 产物、更新签名、Apple 签名和版本规则
- [故障排查](troubleshooting.md)：白屏、FFmpeg、Windows 设备准备、CoreDevice 错误、触控坐标和更新失败

## 支持矩阵

| 能力 | macOS | Windows | Linux |
| --- | --- | --- | --- |
| Tauri 桌面界面 | 支持 | 支持 | 支持 |
| CoreDevice USB 画面 | 主要开发平台 | 需要完成设备准备 | 取决于主机配对和 usbmuxd 环境 |
| Universal HID 控制 | 设备开放服务时支持 | 设备开放服务时支持 | 取决于 CoreDevice 服务可用性 |
| CI 安装包 | Universal DMG | x64 NSIS 和 MSI | x64 AppImage 和 DEB |
| 应用内更新 | 签名 app 压缩包 | 签名 NSIS 安装包 | 签名 AppImage |

CoreDevice 能力由 Apple 控制。USB 配对成功不代表当前硬件和 iOS 组合一定开放远程画面或 Universal HID 服务。

## 文档约定

- 除非页面另有说明，所有命令都从仓库根目录运行。
- `nightly` 表示由 `main` 更新产生的滚动发布版本。
- 路径和服务标识符不翻译。
- 修改功能行为时，应同时更新 `docs/en` 和 `docs/zh-CN` 中对应的页面。
