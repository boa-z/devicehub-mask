# DeviceHub Mask 文档

简体中文 | [English](../en/README.md) | [项目 README](../../README.zh-CN.md)

请按目标选择阅读路径。每个详细主题只有一个权威页面，其他页面通过链接引用，不重复维护相同说明。

## 使用 DeviceHub Mask

| 目标 | 阅读 |
| --- | --- |
| 安装依赖并首次连接设备 | [快速开始](getting-started.md) |
| 使用桌面端各工作区 | [使用指南](user-guide.md) |
| 创建、导入和绑定按键映射 | [按键映射指南](key-mapping.md) |
| 确认某项能力是否已经实现 | [功能参考](features.md) |
| 处理连接、媒体或平台故障 | [故障排查](troubleshooting.md) |

## 服务与自动化

| 目标 | 阅读 |
| --- | --- |
| 在本机或局域网运行浏览器界面 | [Headless 服务](headless.md) |
| 让 agent 控制和检查设备 | [MCP 自动化指南](mcp.md) |

## 开发与发布

| 目标 | 阅读 |
| --- | --- |
| 理解进程、crate、运行时和数据流边界 | [架构说明](architecture.md) |
| 判断代码属于 core、runtime、server、host 还是装配根 | [Core 与 Runtime 边界](core-runtime.md) |
| 配置仓库、验证修改和本地构建 | [开发与构建](development.md) |
| 显式执行硬件回归 | [真机回归](device-regression.md) |
| 构建 CI 产物、发布版本或配置更新 | [CI、发布与更新](distribution.md) |

## 权威信息来源

| 问题 | 权威页面 |
| --- | --- |
| 已实现和明确排除哪些能力？ | [功能参考](features.md) |
| 用户如何完成某项流程？ | [使用指南](user-guide.md)及对应专题指南 |
| 某项行为由哪一层负责？ | [架构说明](architecture.md) |
| Rust 依赖边界有哪些强制规则？ | [Core 与 Runtime 边界](core-runtime.md) |

## 支持概览

| 范围 | macOS | Windows | Linux |
| --- | --- | --- | --- |
| Tauri 桌面 UI | 支持 | 支持 | 支持 |
| CoreDevice USB 画面 | 主要开发平台 | 完成设备准备后支持 | 取决于主机配对/usbmuxd 环境 |
| Universal HID 控制 | 取决于设备能力 | 取决于设备能力 | 取决于设备与主机能力 |
| CI 桌面产物 | Universal DMG | x64 NSIS 和 MSI | x64/ARM64 AppImage 和 DEB |
| Headless 产物 | Universal tar.gz | x64 zip | x64/ARM64 tar.gz |

CoreDevice 服务是否可用由 Apple 决定。配对成功不代表远程画面、HID、诊断或全部管理服务一定可用。

## 文档规则

除非另有说明，命令都在仓库根目录运行。`nightly` 表示从 `main` 生成的滚动构建。服务名、路径和标识符不翻译。行为修改必须同步更新中英文页面，并通过 `npm run docs:check`。
