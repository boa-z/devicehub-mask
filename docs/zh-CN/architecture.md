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

## 会话所有权

CoreDevice 会话运行在专用 Tokio runtime 上，因为部分 `idevice` 服务对象无法安全跨越
普通 `tokio::spawn` 边界。会话拥有画面、HID、AppService 和设备状态资源；会话结束
或切换时会取消依赖操作。

连接时只读取一次 Lockdown 元数据。应用列表和启动优先复用同一会话长期持有的
CoreDevice AppService，避免每次操作创建新的 RSD tunnel。缺少 AppService 时，列表
回退到 Installation Proxy。

IPA 安装和应用卸载使用独立 Tokio 任务及新的 Installation Proxy 连接，因此上传不会
阻塞画面、HID 或应用列表。后端会重新查询卸载目标，只允许移除非 Apple 第一方且标记
为可卸载的用户应用。一个共享操作状态提供阶段和设备上报进度。

当前 `idevice` 包安装辅助逻辑会在 AFC 上传前缓冲整个 IPA，也无法报告字节级上传进度。
因此界面将上传阶段标记为不确定进度，不显示虚构百分比。

## 视频管线

CoreDevice displayservice 输出 RTP/HEVC。FFmpeg 接收 Annex-B HEVC，并输出自描述的
RGB24 PAM 帧。后端只保留最新解码帧，直接丢弃过期帧，避免形成无限队列。

Axum 使用线程本地复用的 TurboJPEG compressor 编码最新帧。每个 WebView 同时只允许
一帧在途；前端完成图片解码和 Canvas 绘制后发送确认。500ms credit 租约避免确认丢失
时永久停帧。这样发送 FPS 会接近显示 FPS，同时保留 60 FPS 上限。

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
- 前端应用元数据不会被用作卸载授权。
- HID report 只在后端验证后构建。
- 更新产物必须通过 Tauri 签名验证才能安装。
- Apple Developer ID 签名与应用更新签名彼此独立。
