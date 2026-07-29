# 架构说明

[English](../en/architecture.md) | [文档首页](README.md)

DeviceHub Mask 是一个产品，包含 Tauri 2 桌面端和 headless 浏览器服务两个原生宿主。它们共用 React UI、领域模型、Apple 设备运行时、原生适配器和认证服务端路由。宿主负责装配这些组件，不重新实现它们。

## 系统结构

```text
                 React UI (src)
                  HTTP / WebSocket
                         |
              devicehub-server
           HTTP + WS + MCP + SPA
                         |
                 RuntimeClient
                         |
              devicehub-runtime
       发现 + 多设备会话 + Apple 服务
                 媒体 + HID
                         |
                 devicehub-core
              有界值 + 领域策略

Tauri 宿主 ---------------------------- Headless 宿主
src-tauri                               devicehub-headless
桌面策略                               CLI/监听/LAN 策略
           \                           /
                  devicehub-host
        文件 + FFmpeg + netmuxd 适配器
```

依赖方向是强约束。Core 不知道 runtime、传输、网络或 UI 框架；runtime 知道 Apple 设备行为，但不知道 HTTP、Tauri 或宿主进程探测；server 知道线上协议，但不启动 runtime，也不绑定生产监听器；装配根拥有生命周期与暴露策略。

## 分层职责

| 层 | 职责 |
| --- | --- |
| `devicehub-core` | 规范化 DTO、校验、状态槽、有界策略、输入和领域值 |
| `devicehub-runtime` | 发现、信任、设备会话、Apple 协议、服务监督、媒体/输入、重连与清理 |
| `devicehub-server` | 认证私有 HTTP、WebSocket 媒体/控制、MCP、SPA 路由和协议校验 |
| `devicehub-host` | 共用原生文件、传输、FFmpeg、netmuxd、配对存储和资源适配器 |
| `devicehub-headless` | CLI 配置、数据路径、token 策略、监听和可选 LAN/MCP 暴露 |
| `src-tauri` | 桌面进程生命周期、私有 loopback 监听、原生音频、剪贴板、窗口、对话框、更新器和权限 |
| `src` | 共用 React 工作区、浏览器视频/音频、输入调度和宿主能力展示 |

可执行依赖规则与模块所有权见 [Core 与 Runtime 边界](core-runtime.md)。

## 宿主装配

两个宿主都创建一个 `RuntimeClient`，注入 `devicehub-host` 能力，再把窄化 client 传给 `devicehub-server`。服务端路由可复用且不拥有监听器。

桌面端绑定随机 loopback 私有 API 供 WebView 使用，并单独暴露 loopback MCP。Tauri 壳层拥有原生音频、剪贴板、文件对话框、窗口状态、更新安装和桌面权限。

Headless 二进制提供同一份前端构建和 API，默认监听 `127.0.0.1:8080`；非 loopback 地址必须使用 `--allow-lan`。浏览器通过 URL fragment 引导的 access token 认证。Headless 与桌面端不能演变成两套 endpoint 实现。

## 多设备运行时

`devicehub-runtime` 拥有专用设备线程、Tokio runtime 与 `LocalSet`、共享发现/信任协调以及隔离设备会话注册表。每个 selection ID 都有独立阶段、错误、命令、媒体状态、服务 worker、重连状态和观测值。

切换设备只改变 UI 焦点，不终止其他已连接会话。断开、重连、配对和撤销信任都只作用于目标。一个物理 UDID 的 USB/Wi-Fi 条目仍是不同发现选项，但 runtime 会阻止同一物理设备出现互相竞争的活动传输。

宿主表面分为管理和会话能力：

- `RuntimeManagerClient` 管理发现清单、选择、配对、信任和 manager 生命周期。
- `DeviceSessionRegistry` 按准确 selection ID 解析 `DeviceSessionClient`。
- `DeviceSessionClient` 只暴露一个会话的观测、媒体、输入和操作。

私有 HTTP 使用 `X-DeviceHub-Device`，WebSocket 使用 `device_id`，每个 MCP 连接持有自己的目标。缺失或未知目标在可能误选设备时必须被拒绝。

## 资源治理

视频、音频、性能采样和设备日志是独立的会话级需求。已连接但未显示的设备应保持可用，同时不承担完整活动设备成本。

- 没有视频消费者时继续排空并观测 RTP/RTCP，但跳过 access unit 发布；恢复时清除旧状态并请求关键帧。
- 桌面端只解码当前设备音频；headless 仅在浏览器为该会话请求未静音音频时解码。
- 性能和设备日志只在工作区或 API 消费者持有需求时启动。
- 关闭和会话替换以有界方式释放按住的 HID、消费者、sidecar 和监督任务。

## 媒体与输入流

视频只有一条路径：runtime 接收 HEVC RTP、组装完整 Annex-B access unit、执行有界展示 credit，再通过 WebSocket 发布。浏览器配置 WebCodecs，在重新同步后等待关键帧，解码并绘制设备画面。当前架构不使用 FFmpeg 解码视频。

音频 RTP 携带 AAC-ELD，由宿主提供的 FFmpeg sidecar 解码为 48 kHz 双声道 PCM。Tauri 送入原生输出，headless 向已认证浏览器发送有界音频帧。LAN 浏览器音频受自动播放和 secure context 策略影响。

鼠标、映射、键盘直通和 MCP 输入都规范化为 core 输入值。Runtime 校验边界与 contact 所有权、转换为 Universal HID report，并按设备串行发送。控制租约以及失焦、模式切换、断开和客户端丢失时的清理用于防止触点或按键卡住。

## 服务与故障模型

每项设备服务报告规范化健康阶段，能安全恢复时独立监督。定位、日志、诊断或性能通道故障不应拆掉视频和输入。传输终止故障只转换受影响会话并使用有界重连策略。错误投影保留用户或 agent 可操作的目标和操作上下文，不暴露无界原始协议数据。

抓包、备份、诊断、文件传输和控制台等长操作都有明确限制，在支持时可取消，并随会话清理。宿主文件只能通过注入能力访问，runtime 不解析或信任本地路径。

## 数据所有权

- Runtime 观测是有界内存槽与事件流。
- 用户偏好和按键映射由宿主通过明确 repository 持久化。
- Headless 数据位于配置的数据目录；桌面数据使用平台应用目录。
- 抓包、备份、日志、崩溃报告和容器传输保持在 WebView 外，除非有界 endpoint 明确返回规范化内容。
- 原始 XPC、plist、CoreDevice client、OS 路径和子进程 handle 不跨越公共领域 API。

## 安全边界

桌面 API 仅暴露到私有 loopback 并使用单次运行认证。Headless LAN 必须显式启用并用 token 认证，但没有内置 TLS、账号、角色、Origin 策略、限流或可撤销会话，只适合可信 LAN，不能直接发布到互联网。MCP 没有认证，应保持 loopback。

App 安装、侧载、签名、注入和升级不属于产品边界。DeviceHub Mask 可以检查和管理已有 App 与描述文件，但描述文件管理不得成为隐藏安装通道。

## 扩展系统

领域策略加入 core，Apple 执行加入 runtime，线上表示加入 server，OS 能力加入 host，生命周期和暴露决策加入装配根。设备身份必须明确，并考虑所有已连接会话。新的高成本生产者必须按需启用并报告健康状态；新的长操作必须有界且可清理；新 UI 应在两个宿主工作，或清晰声明宿主能力。更新权威文档，并按[开发与构建](development.md)执行验证。
