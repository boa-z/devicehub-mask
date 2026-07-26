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
devicehub-desktop -----> devicehub-server -----> devicehub-runtime
devicehub-headless ----> devicehub-server -----> devicehub-runtime
devicehub-mcp -----------------------------------> devicehub-core
```

`devicehub-core` 不得依赖 `idevice`、Tauri、Axum、tower-http、rmcp、FFmpeg、rodio、React 产物、原生对话框、更新器或窗口 API。它持有归一化 DTO、有界校验、业务规则、控制租约、稳定错误、事件和服务契约，并应包含真实策略，而不是空洞的 trait 集合。

设备存储直接遵循该归属：core 定义公共 AFC 与应用容器 DTO、传输活动策略、取消分类、Bundle ID 校验和设备路径约束规范化。runtime 持有 AFC 与 House Arrest 执行命令和传输，宿主保留不透明的本机路径及文件系统流实现。

core 也持有行为不依赖 Apple 传输的有界观察端口，包括抓取与诊断状态、设备条件状态和规范化设备日志环形缓冲。runtime 持有这些端口的生产者；协议转换、需求门控、重试、截止时间和命令 worker 仍是实现细节。

按键映射配置 DTO 与校验也属于 core 策略：配置名约束、支持的映射形式、规范化位置、App 绑定和硬件按键冲突规则必须在桌面与未来无头宿主中保持一致。`devicehub-server` 通过 `ProfileRepository` 端口暴露 HTTP；宿主通过实现该端口继续持有配置目录选择与持久化。

Developer Image 挂载状态与版本到镜像类型的策略遵循相同规则。core 暴露规范化观察结果；runtime 持有不透明的资源请求、宿主注入加载、个性化、设备传输和操作监督。

core 持有规范化服务健康注册表和重启计数转换策略。runtime 持有 reporter 及全部可执行监督行为，包括 tracing、重试延迟、关闭信号、任务创建和强制终止。

core 还持有合并后的性能观察槽及其有界历史和排序策略。runtime 专用转换器接收 Apple DVT 与 plist 样本并产生类型化规范观察；需求信号、采样 worker 和设备 channel 留在 runtime。

`devicehub-runtime` 可以依赖 core、`idevice`、Tokio、序列化、媒体辅助模块，以及跨平台文件系统和网络 API。它不得依赖 Tauri、Axum、rmcp、前端资源、HTTP 鉴权或窗口状态，也不得自行读取宿主环境或解析、启动操作系统进程。原始 XPC、plist、CoreDevice client 和设备传输类型不得越过其公共 API。

适配器依赖 core 服务，不能直接打开 CoreDevice、DVT、Lockdown、AFC、House Arrest、Installation Proxy 或诊断 client。迁移期间可以把现有有界命令入口和状态槽作为兼容 API 重新导出，但新增适配器行为必须使用类型化服务。输入命令以及抓包与诊断状态值由适配器直接从 core 导入，runtime 不再转发这些领域类型。

## 所有权

runtime 持有设备专用 16 MiB 线程、Tokio runtime 与 `LocalSet`、发现、传输状态、唯一活动会话、重连策略、全部非 `Send` 设备 client、服务监督、命令队列、按住输入清理、媒体 worker 和 sidecar 生命周期策略。具体 sidecar 进程的解析和启动由宿主适配器在 runtime 端口背后完成。

面向宿主的 facade 只公开类型化命令、观察状态和能力端口。具体输入 dispatcher、服务 reporter 与 supervisor、重试辅助函数、协议 client 和传输 handle 均保持私有，确保宿主不能绕过 session manager 建立第二条执行或恢复路径。

面向宿主的 `RuntimeClient` 具有两个明确的所有权分组。`RuntimeManagerClient` 只暴露发现清单、当前选择和 manager 生命周期控制；`DeviceSessionClient` 暴露与当前选中会话关联的媒体、输入、观察、服务及设备操作接口；根 client 只负责组合二者。内部 `CoreRuntimeState` 通过私有的 manager 与 device-session 状态组镜像相同拆分，避免 manager view 与宿主 client 投影出不同所有权。`runtime` facade 还把 owner 线程执行器与状态图放在独立的私有模块中。这在保持当前单会话行为的同时，为后续由 registry 持有多个隔离设备 runtime 建立边界。

宿主持有目录选择、环境变量与命令行解析、设置持久化、操作系统进程解析、Tauri 能力、HTTP 监听、鉴权、TLS 与局域网策略，以及本机或远程音频消费者的选择。宿主解析后的路径、FFmpeg 配置、sidecar 适配器和诊断覆盖通过配置或能力端口传入。边界检查会阻止生产 runtime 重新引入环境变量读取、进程启动或 FFmpeg 路径解析。

`devicehub-server` 持有可复用的线路协议适配器，但不持有监听器或 runtime 生命周期。其 WebSocket 适配器统一负责状态发布、输入校验、WebCodecs 数据包发送、流控、遥测和断连清理；MCP 适配器持有完整工具目录、校验、handler 实现与 Streamable HTTP router，同时保持现有 `devicehub_mask` 服务标识。独立 HTTP 适配器持有 App 发现与生命周期、有界崩溃报告、性能工作台、公共 AFC/App 容器存储以及长时间诊断导出路由，每个适配器只接收窄化的 runtime 命令与观察句柄。存储路由通过类型化 runtime 命令传递不透明的宿主路径，runtime 传输仍使用宿主注入的文件系统端口完成校验、流式 I/O 和原子发布；抓包检查与诊断目标规范化通过各自用途明确的异步宿主能力进入适配器。桌面及未来无头 composition root 注入既有 `RuntimeClient` 和有界适配器配置；适配器不能读取进程环境、监听生产端口或启动设备会话。

## 目标 API

core 提供可克隆的类型化能力，不暴露 runtime 实现类型。输入命令和规范化触点属于 core 领域值，runtime 负责将其转换为 Apple HID report：

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
3. 拆分领域 DTO、运行时命令与状态槽及适配器响应类型；适配器可以直接导入所有者 crate 后，移除混合的 `protocol.rs` 与领域通配 facade。
4. 创建 `devicehub-core`，迁移领域模型、校验、策略和类型化服务契约。
5. 创建 `devicehub-runtime`，迁移会话编排、设备实现、监督层和媒体发布。
6. 桌面入口只负责组合 runtime、私有 server、MCP 与 Tauri 平台能力。
7. 只有桌面行为与库边界稳定后，才加入无头入口和局域网宿主。

每一步都使用独立 commit。源码迁移阶段保持行为不变，通过 `npm run verify:full`，本地只构建不打包的 Debug 桌面程序，并保持 Windows、macOS 与 Linux 源码兼容。最后在 iPhone 13 Pro Max 上通过 USB 与 Wi-Fi 检查硬件行为。

## 后续宿主目标

模块提取完成后的下一个仓库级目标是无头 CLI 服务宿主。它组合相同的 `devicehub-runtime` 与 core 服务，但不链接 Tauri、窗口 API、桌面音频输出或前端资源。CLI 配置负责监听地址、鉴权材料、数据目录、配对存储、sidecar 解析、日志、关闭信号，以及显式启用的 HTTP/WebSocket/MCP 适配器。首个版本默认仍只监听回环地址；发布到局域网仍必须满足下述安全边界。

多设备保持连接安排在无头宿主之后，因为无头宿主能最清楚地验证生命周期边界。当前单 runtime 状态图将演进为宿主持有的 runtime registry，加上每个已选物理设备一个隔离的 `DeviceRuntime`。设备发现与信任存储成为共享协调服务，每台设备则保留独立 owner 线程、会话、监督树、命令、媒体流控、需求计数器和确定性关闭。属于同一物理设备的 USB 与 Wi-Fi 端点必须归并为一个逻辑设备和一条活动传输，同时显式定义全局 CPU、内存、解码器、音频输出和重连限额，不能依赖进程级隐式状态。

## 边界与验收

两个库都永远不安装、sideload、签名、升级或注入 App。描述文件管理仍是独立授权的设备管理能力，不能演变为 App 安装路径。

局域网发布不能通过把当前私有 server 直接绑定到 `0.0.0.0` 实现。后续 server 边界必须具备显式启用、TLS、客户端配对、分级角色、控制租约、Origin 限制、速率限制和可撤销会话。MCP 在具备独立鉴权前继续只监听回环地址。

只有当 Tauri 与无头宿主可使用同一 runtime 生命周期、core 不导入禁止的实现依赖、只有 runtime 持有设备会话与非 `Send` client、core 测试不需要 Tauri 或网络端口、失败路径不遗留设备任务或 sidecar，并且 USB/Wi-Fi、WebCodecs、音频、输入、App 管理、AFC、诊断和重连行为继续通过验证时，提取才算完成。
