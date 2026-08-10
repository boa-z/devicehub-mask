# Core 与 Runtime 边界

[English](../en/core-runtime.md) | [文档首页](README.md)

Core/runtime 拆分已经完成。本页定义当前 Rust 边界，不再是迁移计划。`scripts/check-rust-boundaries.mjs` 会强制检查最重要的依赖与源码规则。

## 依赖方向

```text
devicehub-headless ----+--> devicehub-server ----+
                      |                          |
src-tauri -------------+--> devicehub-host ------+--> devicehub-runtime --> devicehub-core
```

宿主可以依赖其装配的下层能力，server 和 host 适配器可以依赖 runtime/core，runtime 依赖 core，反向依赖一律禁止。

## `devicehub-core`

Core 拥有稳定、规范化、有界的领域行为：设备和 App 值、输入命令、按键映射校验、存储路径策略、诊断/抓包状态、性能观测、服务健康、定位、描述文件元数据和可复用状态槽。

Core 不能依赖异步/设备/Web/桌面/进程框架，包括 `tokio`、`idevice`、`axum`、`rmcp`、`rodio`、Tauri、`wry` 或 FFmpeg。它不拥有宿主路径、原始 plist/XPC 值、协议 client、重试循环或任务启动。Core 应包含真实的校验和状态转换策略，而不只是 marker trait 或传输形状 DTO。

## `devicehub-runtime`

Runtime 是唯一 Apple 设备执行层，拥有发现、配对/信任协调、并发会话注册表、非 `Send` client、命令队列、Apple 服务转换、媒体协商/发布、Universal HID、需求租约、监督、重连和确定性会话清理。

其公共表面只包含类型化 client、命令、有界观测和宿主能力端口。原始协议 client 与传输 handle 保持私有。Runtime 可以使用 `idevice`、Tokio、序列化和平台中立网络，但不能使用 Axum、MCP、Tauri、`rodio`、`wry` 或桌面前端资源。生产 runtime 代码不得读取进程环境、解析可执行文件、启动 FFmpeg/netmuxd 或选择宿主目录。

关键公共边界为：

- `RuntimeManagerClient`：清单、前端选择、配对/信任和 manager 操作。
- `DeviceSessionRegistry`：准确会话查找。
- `DeviceSessionClient`：单一目标的观测、命令、媒体和需求。
- `ManagedOperationRegistry`：单一目标长操作的有界生命周期与类型化错误投影；详细领域状态仍由所属服务维护。
- 宿主端口：剪贴板、文件、抓包目标、Developer Image、描述文件、备份、诊断 sink、音频 pipeline 和 sidecar。

## `devicehub-server`

Server 拥有有界线上适配：认证私有 HTTP、状态投影、WebSocket 视频/音频/控制、MCP handler、SPA 和 API 错误映射。它接收已有 runtime client、repository 和显式配置。

它不能拥有监听器、解析宿主环境、启动设备 runtime、打开 Apple 服务或使用 Tauri/桌面音频。Manager 路由只接收 manager 能力；设备路由解析目标会话；文件/profile 路由接收窄化宿主 repository。新增 endpoint 不代表可以绕过 runtime 公共边界。

## `devicehub-host`

Host 包含桌面与 headless 共用的原生实现：受限文件系统、profile 持久化、浏览器传输、抓包/诊断目标、Developer Image/描述文件资源、备份、FFmpeg 音频解码、netmuxd 和 Wi-Fi 配对存储。

它必须兼容 headless，不能依赖 Tauri、`wry`、桌面剪贴板/音频库、监听策略或产品 UI。它实现 runtime/server 端口，但不决定设备策略。

## 装配根

`devicehub-headless` 拥有 CLI 解析、数据/前端目录、token 文件、监听地址、显式 LAN 权限、可选 MCP 绑定、信号处理和进程关闭。

`src-tauri` 拥有桌面进程、loopback 监听生命周期、Tauri 状态/权限、窗口/更新器/对话框、原生音频与剪贴板、桌面设置和应用关闭。桌面专用适配器留在这里，不泄漏到共享 crate。

两个装配根都不能创建第二套设备 session manager 或重复 server 路由。

## 多设备契约

注册表 key 是包含传输信息的 selection ID。操作先解析该 ID，再访问会话。切换 UI 焦点不销毁会话；一个物理 UDID 的重复 USB/Wi-Fi 活动会被拒绝；断开和恢复只影响目标。MCP 连接可以选择不同设备，HTTP 为每个请求解析一次已认证 `DeviceScope`，WebSocket 客户端携带明确设备范围。

视频、音频、性能和日志的需求租约按会话隔离，消费者在断开或失败时必须释放。新后台服务必须有明确所有者、有界重启策略、健康报告和确定性关闭。

## 可见性与模块形状

使用 `foo.rs + foo/*.rs` 把领域 facade 与相关实现放在一起。只有其他 crate 确实需要时，才从所有者 crate 的 `lib.rs` 导出公共符号。优先私有，其次 `pub(crate)`，最后才是窄化 `pub` API。不要用 wildcard re-export 或通用共享模块让所有权违规勉强通过编译。

## 验收检查

依赖、import、可见性、环境读取、进程启动、FFmpeg 解析、监听器或 crate 所有权变化时运行 `npm run rust:boundaries`，随后运行针对性测试和常规 `npm run verify`。只有两个宿主仍组合同一套 runtime/server 行为、多设备范围明确、清理有界，并且[架构说明](architecture.md)和[开发与构建](development.md)仍准确时，边界修改才可接受。
