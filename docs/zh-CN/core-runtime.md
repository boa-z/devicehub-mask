# Core 与 Runtime 提取

简体中文 | [English](../en/core-runtime.md) | [文档首页](README.md)

状态：已接受，按阶段实施。

## 决策

DeviceHub Mask 保持单一仓库，并把 Rust 后端拆分为两个宿主无关库：

- `devicehub-core` 定义稳定的领域数据、校验、错误、事件和类型化服务句柄。
- `devicehub-runtime` 持有具体 Apple 设备实现，并实现 core 定义的服务。

Tauri、未来的无头进程、HTTP/WebSocket 与 MCP 都是同一 runtime 外围的宿主或适配器。它们不得分别创建设备会话，也不得复制设备策略。提取必须渐进进行：先在现有桌面 crate 内建立内部 `DeviceRuntime` 边界并覆盖生命周期测试，再创建 workspace crate。机械迁移与行为修改不得放在同一个 commit。

## 依赖方向

```text
devicehub-desktop -----> devicehub-runtime -----> devicehub-core
devicehub-headless ----> devicehub-runtime -----> devicehub-core
devicehub-server --------------------------------> devicehub-core
devicehub-mcp -----------------------------------> devicehub-core
```

`devicehub-core` 不得依赖 `idevice`、Tauri、Axum、tower-http、rmcp、FFmpeg、rodio、React 产物、原生对话框、更新器或窗口 API。它持有归一化 DTO、有界校验、业务规则、控制租约、稳定错误、事件和服务契约，并应包含真实策略，而不是空洞的 trait 集合。

`devicehub-runtime` 可以依赖 core、`idevice`、Tokio、序列化、媒体辅助模块，以及跨平台文件系统、网络和进程 API。它不得依赖 Tauri、Axum、rmcp、前端资源、HTTP 鉴权或窗口状态。原始 XPC、plist、CoreDevice client 和设备传输类型不得越过其公共 API。

适配器依赖 core 服务，不能直接打开 CoreDevice、DVT、Lockdown、AFC、House Arrest、Installation Proxy 或诊断 client。迁移期间可以把现有有界命令入口和状态槽作为兼容 API 重新导出，但新增适配器行为必须使用类型化服务。

## 所有权

runtime 持有设备专用 16 MiB 线程、Tokio runtime 与 `LocalSet`、发现、传输状态、唯一活动会话、重连策略、全部非 `Send` 设备 client、服务监督、命令队列、按住输入清理、媒体 worker 和 sidecar 生命周期。

宿主持有目录选择、环境变量与命令行解析、设置持久化、Tauri 能力、HTTP 监听、鉴权、TLS 与局域网策略，以及本机或远程音频消费者的选择。宿主解析后的路径和诊断覆盖通过配置传入。深层 `DEVICEHUB_*` 读取与全局 FFmpeg 资源目录属于迁移债务，必须在最终 crate 边界建立前移除。

## 目标 API

core 提供可克隆的类型化能力，不暴露 runtime 实现类型：

```rust
pub struct DeviceHubServices {
    pub devices: DeviceService,
    pub input: InputService,
    pub applications: ApplicationService,
    pub storage: StorageService,
    pub diagnostics: DiagnosticsService,
    pub media: MediaService,
}
```

runtime 持有启动与确定性关闭：

```rust
pub struct RuntimeConfig;
pub struct DeviceRuntime;

impl DeviceRuntime {
    pub fn start(config: RuntimeConfig) -> Result<Self, RuntimeError>;
    pub fn services(&self) -> DeviceHubServices;
    pub fn shutdown(self) -> Result<(), RuntimeError>;
}
```

启动 runtime 不会创建 HTTP、MCP、Tauri 或前端任务；启动适配器也不会创建设备会话。关闭时拒绝新命令、释放按住输入、结束活动会话与受监督任务、停止自有 sidecar，最后 join 设备线程。显式关闭、重复关闭和部分启动失败后的清理都必须安全。

## 迁移顺序

1. 引入内部 `DeviceRuntime`、配置、服务和关闭边界，不改变桌面行为。
2. 从宿主注入路径、FFmpeg、netmuxd、偏好、日志和音频发布决策。
3. 拆分目前集中在 `protocol.rs` 的领域 DTO、运行时命令与状态槽，以及适配器响应类型。
4. 创建 `devicehub-core`，迁移领域模型、校验、策略和类型化服务契约。
5. 创建 `devicehub-runtime`，迁移会话编排、设备实现、监督层和媒体发布。
6. 桌面入口只负责组合 runtime、私有 server、MCP 与 Tauri 平台能力。
7. 只有桌面行为与库边界稳定后，才加入无头入口和局域网宿主。

每一步都使用独立 commit。源码迁移阶段保持行为不变，通过 `npm run verify:full`，本地只构建不打包的 Debug 桌面程序，并保持 Windows、macOS 与 Linux 源码兼容。最后在 iPhone 13 Pro Max 上通过 USB 与 Wi-Fi 检查硬件行为。

## 边界与验收

两个库都永远不安装、sideload、签名、升级或注入 App。描述文件管理仍是独立授权的设备管理能力，不能演变为 App 安装路径。

局域网发布不能通过把当前私有 server 直接绑定到 `0.0.0.0` 实现。后续 server 边界必须具备显式启用、TLS、客户端配对、分级角色、控制租约、Origin 限制、速率限制和可撤销会话。MCP 在具备独立鉴权前继续只监听回环地址。

只有当 Tauri 与无头宿主可使用同一 runtime 生命周期、core 不导入禁止的实现依赖、只有 runtime 持有设备会话与非 `Send` client、core 测试不需要 Tauri 或网络端口、失败路径不遗留设备任务或 sidecar，并且 USB/Wi-Fi、WebCodecs、音频、输入、App 管理、AFC、诊断和重连行为继续通过验证时，提取才算完成。
