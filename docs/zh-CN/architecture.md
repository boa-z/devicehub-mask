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
更新时间。虚拟定位、Condition Inducer、Sysmontap、Graphics、NetworkMonitor 与
EnergyMonitor 通道分别使用有上限的指数退避恢复；单个通道断开不会终止视频或 HID。

设备条件模拟独占一条 DVT Condition Inducer channel 和有界命令队列。后端限制并净化设备
返回的配置目录，只接受目录中实际存在的 group/profile 组合。每次 channel 连接后首先停用
可能残留的条件，以建立已知基线。启用请求失败时仍按“可能已经生效”处理，因为设备可能在
回复失败前已经提交。会话退出时执行有超时的清理；无法确认成功时，共享状态保留
`cleanup_pending`，直到后续连接成功清除。重连后不会自动恢复之前的模拟条件。

性能监控复用活动软件隧道的克隆 handle，并建立相互隔离的 DVT 连接。性能工作台或所选 HUD
指标有需求时才启动 Sysmontap、Graphics、NetworkMonitor 和 EnergyMonitor 采样。
NetworkMonitor 使用独立 RemoteServer 连接，按连接累计计数器的差值计算每秒收发速率；超过一分钟未更新的连接
会过期，且跟踪表具有固定容量上限。标准化后的最新快照通过带鉴权的私有 API 提供；
短期图表历史只保留在前端，切换设备时清空。
Sysmontap 进程数组依据当前会话协商得到的属性顺序解码，不依赖固定字段下标。单进程 CPU
除以设备报告的逻辑核心数；快照保留 CPU 前十与物理内存前十的并集，最多二十行。
EnergyMonitor 通过另一条 RemoteServer 连接跟踪该有界列表中的前十六个进程，PID 集合
变化时更新设备订阅，需求消失时主动停止，并提供 Apple 的相对总能耗、CPU、GPU、网络、
显示、定位与 App 状态能耗分数。

连接时读取 Lockdown 元数据，并在设备信息请求时重新读取，因此存储或设备名称变更通知
可以展示当前值。设备详情刷新还会并行通过 MobileActivationd 读取激活状态、通过
DiagnosticsRelay 读取电池信息，并通过 MobileImageMounter 读取开发者模式。后端将厂商
状态字符串归一化为固定公开枚举，不请求激活记录、证书或 activation-info 载荷。
应用列表和生命周期控制优先复用同一会话长期持有的
CoreDevice AppService，避免每次操作创建新的 RSD tunnel。只有可执行文件的直接父目录
等于目标 App bundle 时才会匹配进程；停止操作会重新读取设备状态并发送固定 SIGTERM，
客户端不能指定 PID 或信号。缺少 AppService 时，列表回退到 Installation Proxy，运行
状态保持未知。
App 图标通过独立、按请求工作的 SpringBoardServices RSD channel 获取，因此不会占用 HID
分发循环。worker 会校验 PNG header 与尺寸，限制单张 4 MiB，并使用 256 项、32 MiB 的
FIFO 缓存；前端只请求接近可视区域的 App 行。
原生截图使用独立、有界的 CoreDevice ScreenCaptureService channel。worker 只接受一条
排队请求，校验 PNG 与尺寸并把响应限制在 32 MiB；截图不会占用 HID 分发循环。

设备网络抓包由独立、用户主动启动的 pcapd worker 通过克隆 RSD tunnel 执行。它将标准化
以太网记录直接写入目标同目录的主机临时文件，单包限制为协商的 256 KiB snapshot，完整
抓包限制为 256 MiB，最后原子替换所选 `.pcap` 目标。手动停止、超时、数据流失败和会话
关闭都会执行收尾。私有 API 只接收有界计数与状态，数据包正文不会进入 WebView 或 MCP
传输。

重启与关机是两个固定的私有 API 命令，不接受客户端传入任意 DiagnosticsRelay 操作。每个
命令都在有超时的独立任务中建立 relay 连接，因此等待设备确认不会阻塞 HID 分发；前端
必须显示包含设备名称的二次确认。

App 文档由独立受监督的 House Arrest worker 处理，并使用克隆的 RSD tunnel。每条命令只
vend 所选 App 的 Documents 根目录，并建立新的 AFC 会话；远端路径拒绝目录穿越和名称中
的路径分隔符。下载和上传都在 AFC 与主机文件间流式复制：下载使用可回滚的本地替换，
上传先写入唯一远端临时文件，关闭成功后才改名。上传不会静默覆盖同名项目，删除也不递归。

剪贴板同步仅在持久化的显式开关启用并重新连接设备后使用 CoreDevice Pasteboard Service。
设备侧变更优先通过推送接收，电脑侧按有界频率轮询并抑制回环；默认关闭时不会产生
后台剪贴板访问或传输，但显式的单次粘贴仍会写入请求的文本。同步活动通过容量为 8 的
广播通道发送给已鉴权 WebSocket，因此界面提示不会反压设备服务。
单次 Unicode 粘贴通过容量为 4 的命令队列复用同一个 Pasteboard Service 所有者，并且
只在收到 SET 回复后发送 Cmd+V。

IPA 安装和应用卸载使用独立 Tokio 任务及新的 Installation Proxy 连接，因此上传不会
阻塞画面、HID 或应用列表。后端会重新查询卸载目标，只允许移除非 Apple 第一方且标记
为可卸载的用户应用。一个共享操作状态提供阶段和设备上报进度。

当前 `idevice` 包安装辅助逻辑会在 AFC 上传前缓冲整个 IPA，也无法报告字节级上传进度。
因此界面将上传阶段标记为不确定进度，不显示虚构百分比。

崩溃报告列表与导出会在独立 Tokio 任务中建立新的 CrashReportCopyMobile/AFC 会话，因此
递归目录读取和文件传输不会阻塞 HID 分发。列表受目录深度和条目数量限制；导出会重新
验证设备绝对路径与普通文件元数据，把内存分配限制在 128 MiB，并且只向 WebView 返回
元数据。

设备日志仅在日志工作台打开时建立受监督的 SyslogRelay 连接。每条日志会先清理控制字符
并限制为 16 KiB，再进入最多 2,000 条的内存环形缓冲区；私有 API 每次最多返回 500 条，
并报告游标落后造成的缺口。手机日志不会进入应用自身的 tracing 日志，也不会自动持久化。

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

## 音频管线

CoreDevice 协商 48 kHz 双声道 AAC-ELD，每个 RTP 包包含一个 10 ms Access Unit。设备发送
裸 Access Unit，因此后端先补充 RFC 3640 AU header，再将 RTP 转发给 FFmpeg。FFmpeg
解码为交错 S16LE；后端通过有界广播发布 20 ms PCM 块，生产端不会等待消费者。

音频使用带版本的 `DHAP` WebSocket 二进制信封，JPEG 消息保持原格式。WebView 通过 Web
Audio 以小型抖动缓冲调度 PCM；排队延迟超过 250 ms 时会重新同步。设备音频默认关闭，
音频解码器故障时仅退回静默 drain，不会终止视频或输入。

## 输入管线

React 合并映射、鼠标和键盘状态，并跳过完全相同的触控帧。Rust 将校验后的类型化触点
转换成一个固定五槽位 Universal HID 多点触控 report。键盘和硬件命令保留按下与释放
状态，断线清理会释放所有按住的 usage。

映射模式和键盘透传互斥，避免一个物理按键同时产生映射触控和键盘 usage。

## 描述文件数据

描述文件由独立监督的有界 Misagent 命令服务管理，因此相关操作不会阻塞 HID 输入循环。
在 plist 元数据进入私有 API 前会先解码 CMS SignedData。原始描述文件和 provisioned
device identifiers 不会进入前端；单个损坏文件会被隔离，不会导致整个列表失败。

安装和移除命令携带请求截止时间，避免 HTTP 请求超时后排队操作仍然延迟生效。安装前会
验证本地文件和描述文件元数据，移除前后都会重新读取设备目录。输入错误、不存在、冲突、
传输故障和超时在私有 API 中保持类型化语义；只有传输故障和超时才会让监督器重建
Misagent channel。

如果 displayservice 不可用，但 Lockdown 仍可使用，后端会保留降级管理会话。画面控制
和仅 AppService 支持的操作会明确标记为不可用，而不是隐藏整个设备。

## 依赖固定版本

`idevice` 暂时固定到项目 fork 中已审查的 `a64b886` revision。该版本包含 iOS 27
CoreDevice 修复，以及性能工作台使用的类型化 DVT NetworkMonitor 与 EnergyMonitor
客户端。等价修复合并并正式发布后，应切回上游版本。

## 安全边界

- 私有 API 始终只监听回环地址并要求令牌。
- MCP 默认只监听回环地址且没有鉴权，监听非回环地址时会输出警告。
- 前端应用元数据不会被用作卸载授权。
- HID report 只在后端验证后构建。
- 更新产物必须通过 Tauri 签名验证才能安装。
- Apple Developer ID 签名与应用更新签名彼此独立。
