# 架构说明

简体中文 | [English](../en/architecture.md) | [文档首页](README.md)

## 系统概览

```text
Tauri 2 桌面外壳（WKWebView / WebView2 / WebKitGTK）
        |
React 19 + Ant Design 工作区
        |
Tauri IPC 启动握手
        |
经过鉴权的私有回环 WebSocket 和 HTTP API
        |
Rust / Axum 服务
        |
idevice：CoreDevice、Lockdown、Installation Proxy、Misagent、Universal HID
```

仓库采用标准 Tauri 2 结构。Vite 从 `src/` 构建 React 界面，Rust 桌面代码和 Tauri
配置位于 `src-tauri/`。生产前端资源由 Tauri 嵌入，应用生命周期也由 Tauri 管理。

## 桌面端与私有传输

Axum 是内部传输层，而不是独立部署的网页服务器。默认监听随机回环端口，没有浏览器
入口，不负责提供前端文件，并要求使用通过 Tauri IPC 获取的每次启动独立 bearer
token。没有活动会话时，设备管理路由返回 `503`。

WebSocket 传输 JPEG 帧和类型化控制消息。前端发送归一化触点，而不是原始 HID report。
Rust 会在分发前验证触点身份、五触点上限、坐标范围和画面方向。

MCP 服务是独立的 Streamable HTTP 端点，默认监听
`127.0.0.1:8009/mcp`。它共享会话管理器的最新帧、输入通道、设备状态和控制通道，
因此自动化客户端与 WebView 复用同一个 CoreDevice 会话。坐标工具携带截图尺寸，并通过
与鼠标直接触控相同的方向模型转换。游戏手势通过共享输入队列串行发送一至五触点 HID
帧；截图和动作结果携带帧版本，使 agent 可以跳过画面稳定等待并显式等待下一解码帧。
MCP 没有鉴权；监听非回环地址属于显式部署选择，同时会输出警告。

## 会话所有权

CoreDevice 会话运行在专用 Tokio runtime 上，因为部分 `idevice` 服务对象无法安全跨越
普通 `tokio::spawn` 边界。会话拥有画面、HID、AppService 和设备状态资源；会话结束
或切换时会取消依赖操作。

可选设备服务统一运行在该 runtime 内的 Tokio `LocalSet` 和服务监督器下。这样不可
`Send` 的 DVT channel 始终留在 CoreDevice 所有者线程，而 HTTP、WebSocket 和 MCP
传输仍可使用多线程 runtime。每项服务发布统一的阶段、尝试次数、重启次数、最后错误和
更新时间。虚拟定位、Sysmontap 与 Graphics 通道分别使用有上限的指数退避恢复；单个
通道断开不会终止视频或 HID。

性能监控复用活动软件隧道的克隆 handle，并建立相互隔离的 DVT 通道。只有性能工作台
打开时才启动 Sysmontap 和 Graphics 采样，关闭页面后停止。标准化后的最新快照通过带
鉴权的私有 API 提供；短期图表历史只保留在前端，切换设备时清空。

连接时只读取一次 Lockdown 元数据。应用列表和启动优先复用同一会话长期持有的
CoreDevice AppService，避免每次操作创建新的 RSD tunnel。缺少 AppService 时，列表
回退到 Installation Proxy。

IPA 安装和应用卸载使用独立 Tokio 任务及新的 Installation Proxy 连接，因此上传不会
阻塞画面、HID 或应用列表。后端会重新查询卸载目标，只允许移除非 Apple 第一方且标记
为可卸载的用户应用。一个共享操作状态提供阶段和设备上报进度。

当前 `idevice` 包安装辅助逻辑会在 AFC 上传前缓冲整个 IPA，也无法报告字节级上传进度。
因此界面将上传阶段标记为不确定进度，不显示虚构百分比。

## 视频管线

CoreDevice displayservice 输出 RTP/HEVC。后端先组装完整 HEVC Access Unit，再进入 16 MiB
字节上限队列；溢出时丢弃依赖帧直至 IRAP，并通过 PLI/FIR 请求恢复。FFmpeg 默认输出
自描述的 RGB24 PAM 帧。实验性 YUV420P 设置（也可通过
`DEVICEHUB_VIDEO_PIXEL_FORMAT=yuv420p` 选择）输出 YUV4MPEG2，并将 planar YUV420P
直接交给 TurboJPEG，避免 RGB 转换并将解码帧带宽减半。
最新帧通过 `watch` 通道按事件通知 WebSocket，取消固定频率轮询；慢消费者只会看到最新帧，
不会积压陈旧画面。

Axum 使用线程本地复用的 TurboJPEG compressor 编码最新帧。每个 WebView 最多允许两帧
在途，使后端 JPEG 编码与 WebView JPEG 解码可以重叠，同时不会形成无限队列。前端会对
已解码呈现或主动替换的帧发送确认；500ms credit 租约避免确认丢失时永久停帧。

Windows 默认将解码长边限制在 1920 像素。FFmpeg 始终保持比例、不放大，并输出偶数
尺寸。设置 `DEVICEHUB_VIDEO_MAX_DIMENSION=0` 可使用原始分辨率，低性能设备可设置
更小值。

Canvas 使用同一个比例 contain-fit 旋转后的源画面。鼠标坐标只在准确显示矩形内归一化，
避免横屏拉伸和触控偏移。

## 输入管线

React 合并映射、鼠标和键盘状态，并跳过完全相同的触控帧。Rust 将校验后的类型化触点
转换成一个固定五槽位 Universal HID 多点触控 report。键盘和硬件命令保留按下与释放
状态，断线清理会释放所有按住的 usage。

映射模式和键盘透传互斥，避免一个物理按键同时产生映射触控和键盘 usage。

## 描述文件数据

描述文件通过长期 Misagent 连接读取，在 plist 元数据进入私有 API 前以 CMS SignedData
解码。原始描述文件和 provisioned device identifiers 不会进入前端。单个损坏文件会被
隔离，不会导致整个列表失败。

如果 displayservice 不可用，但 Lockdown 仍可使用，后端会保留降级管理会话。画面控制
和仅 AppService 支持的操作会明确标记为不可用，而不是隐藏整个设备。

## 依赖固定版本

`idevice` 暂时固定到项目 fork 中已审查的 `0371286` revision。该版本加入 iOS 27
AppService 请求解码所需的 `requireContainerAccess=false`。等价修复合并并正式发布后，
应切回上游版本。

## 安全边界

- 私有 API 始终只监听回环地址并要求令牌。
- MCP 默认只监听回环地址且没有鉴权，监听非回环地址时会输出警告。
- 前端应用元数据不会被用作卸载授权。
- HID report 只在后端验证后构建。
- 更新产物必须通过 Tauri 签名验证才能安装。
- Apple Developer ID 签名与应用更新签名彼此独立。
