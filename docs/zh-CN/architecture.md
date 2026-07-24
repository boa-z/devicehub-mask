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

仓库采用标准 Tauri 2 结构。Vite 从 `src/` 构建 React 界面，Rust 桌面代码和 Tauri 配置位于 `src-tauri/`。生产前端资源由 Tauri 嵌入，应用生命周期也由 Tauri 管理。

Rust 传输适配器不应分别实现设备控制规则。第一条迁移到 `application` 应用服务层的纵向链路包含活动会话命令、截图超时、最新原生帧、浏览器画面尺寸和联合帧版本。HTTP 截图与 MCP 截图、输入和帧等待把各自的请求/响应格式映射到同一个 `DeviceControlService`；等待新画面由原生 `watch` 或浏览器帧广播直接唤醒，不在适配器中轮询。后续设备能力应沿用“传输适配器 -> 应用服务 -> 会话能力”的依赖方向。

前端的 `useDeviceVideoStream` controller 独占视频 WebSocket、WebCodecs/JPEG 解码、Canvas 呈现、重连、画面需求、停滞检测和前端性能指标。`App` 只组合页面工作流，通过稳定的命令接口发送输入，并在断线回调中清理按键映射状态。截图与录制可以读取 controller 暴露的 Canvas ref，但不会参与解码器生命周期管理。

前端运行时服务沿用相同的所有权规则。`usePrivateBackend` 独占 Tauri 启动握手和 bearer 鉴权请求构造，`usePerformanceTelemetry` 与 `useDeviceLogDemand` 分别独占受监督服务的需求启停、轮询与清理。非主工作区和大型检查器通过独立 React suspense 边界加载，使设备画面无需等待 AFC、诊断、设置或映射编辑代码完成解析。AFC 只在首次访问时加载，此后保持挂载，因为活动传输与取消操作属于该工作区生命周期。`DeviceFullscreenToolbar` 负责独立的硬件与功能控制面及 Pointer Capture，纯 `fullscreenToolbarLayout` 函数负责边界限制、最近槽位吸合，以及以硬件控制面为最高优先级的实际渲染边界冲突处理。`App` 继续持有命令与生命周期状态；持久化的设备视图偏好分别控制两个工具条槽位以及设备/映射侧栏，设备侧栏通过 CSS 收起以保留检查器组件中的活动状态。

按键映射导入会先经过前端来源适配器注册表，再进入现有配置持久化路径。每个适配器统一声明 ID、接受的文件类型、大小限制、解析器，以及到共用导入结果的转换。界面会明确选择适配器，因此后续新增来源无需继续在配置管理器中堆叠格式猜测。原生格式和 scrcpy-mask 使用 JSON，PlayCover `2.0.0` plist 则由按需加载的结构化 XML 解析器处理。PlayCover 导入会仅允许标准 Apple plist DTD 声明并拒绝实体，同时限制文件大小、嵌套深度、节点数和模型数，再将支持的键盘控件转换为共用的标准化映射模型。

## 桌面端与私有传输

Axum 是内部传输层，而不是独立部署的网页服务器。默认监听随机回环端口，没有浏览器 入口，不负责提供前端文件，并要求使用通过 Tauri IPC 获取的每次启动独立 bearer token。没有活动会话时，设备管理路由返回 `503`。 锁定、重启和关机命令共享 Diagnostics Relay 单任务租约。锁定映射到单向休眠请求并保留 父控制会话；重启和关机则允许设备断开结束当前会话。

WebSocket 传输 JPEG 帧和类型化控制消息。前端发送归一化触点，而不是原始 HID report。 Rust 会在分发前验证触点身份、五触点上限、坐标范围和画面方向。

MCP 服务是独立的 Streamable HTTP 端点，默认监听 `127.0.0.1:8009/mcp`。它共享会话管理器的最新帧、输入通道、设备状态和控制通道， 并共享性能快照和有界设备日志缓冲，因此自动化客户端与 WebView 复用同一个 CoreDevice 会话。后端仅保留当前会话最近一条归一化设备元数据事件，用于消除读取事件游标与订阅 下一事件之间的竞态；切换会话会清除保留事件，但不会重用单调递增的序列号。性能与日志 调用使用临时需求租约，与 WebView 的显式需求组合，而不会改写其状态。 坐标工具携带截图尺寸，并通过 与鼠标直接触控相同的方向模型转换。游戏手势通过共享输入队列串行发送一至五触点 HID 帧；截图和动作结果携带帧版本，使 agent 可以跳过画面稳定等待并显式等待下一解码帧。 MCP 崩溃报告工具通过当前会话 provider 分发，报告正文上限为 1 MiB；设备路径使用与桌面 导出相同的防穿越校验。 设备详情调用使用现有会话命令队列，除非调用者显式请求，否则省略稳定硬件标识符。 MCP 没有鉴权；监听非回环地址属于显式部署选择，同时会输出警告。

## 会话所有权

CoreDevice 会话运行在专用 Tokio runtime 上，因为部分 `idevice` 服务对象无法安全跨越 普通 `tokio::spawn` 边界。会话拥有画面、HID、AppService 和设备状态资源；会话结束 或切换时会取消依赖操作。

可选设备服务统一运行在该 runtime 内的 Tokio `LocalSet` 和服务监督器下。这样不可 `Send` 的 DVT channel 始终留在 CoreDevice 所有者线程，而 HTTP、WebSocket 和 MCP 传输仍可使用多线程 runtime。每项服务发布统一的阶段、尝试次数、重启次数、最后错误和 更新时间。虚拟定位、Condition Inducer、Sysmontap、Graphics、NetworkMonitor 与 EnergyMonitor 通道分别使用有上限的指数退避恢复；单个通道断开不会终止视频或 HID。

受监督的 Notification Proxy 会把厂商通知名称缩减为固定事件枚举。App、磁盘、名称和 激活状态变化只刷新受影响的前端数据。SpringBoard 锁屏状态变化会释放所有活动输入，并在 不虚构已锁定/已解锁值的前提下转发，因为该通知不包含状态载荷。MCP 使用单调事件序列， 避免读取游标与订阅之间的竞态。

设备条件模拟独占一条 DVT Condition Inducer channel 和有界命令队列。后端限制并净化设备 返回的配置目录，只接受目录中实际存在的 group/profile 组合。每次 channel 连接后首先停用 可能残留的条件，以建立已知基线。启用请求失败时仍按“可能已经生效”处理，因为设备可能在 回复失败前已经提交。会话退出时执行有超时的清理；无法确认成功时，共享状态保留 `cleanup_pending`，直到后续连接成功清除。重连后不会自动恢复之前的模拟条件。

性能监控复用活动软件隧道的克隆 handle，并建立相互隔离的 DVT 连接。性能工作台或所选 HUD 指标有需求时才启动 Sysmontap、Graphics、NetworkMonitor 和 EnergyMonitor 采样。 NetworkMonitor 使用独立 RemoteServer 连接，按连接累计计数器的差值计算每秒收发速率；超过一分钟未更新的连接 会过期，且跟踪表具有固定容量上限。标准化后的最新快照通过带鉴权的私有 API 提供； 短期图表历史只保留在前端，切换设备时清空。 Sysmontap 进程数组依据当前会话协商得到的属性顺序解码，不依赖固定字段下标。单进程 CPU 除以设备报告的逻辑核心数；快照保留 CPU 前十与物理内存前十的并集，最多二十行。 EnergyMonitor 通过另一条 RemoteServer 连接跟踪该有界列表中的前十六个进程，PID 集合 变化时更新设备订阅，需求消失时主动停止，并提供 Apple 的相对总能耗、CPU、GPU、网络、 显示、定位与 App 状态能耗分数。 App 活动监控使用另一条 DVT Notifications 连接，并且只在性能工作台请求采样时存在。 通知类型、App 或进程名称以及状态值在进入私有 API 前会合并空白并限制长度。当前会话最多 保留 100 条带单调序列号的事件，设备会话重置时清空；原始归档通知载荷不会对外暴露。

独立的双命令队列 worker 只在显式请求进程清单时打开 DVT DeviceInfo；结果按 PID 去重，最多返回 1,024 项，名称限制为 128 个字符，只通过私有 API 与 MCP 暴露 PID、进程/App 名称及 Apple 的 App 分类。worker 不调用 DeviceInfo 的目录、可执行路径、UID/GID 或进程控制方法。

连接时读取 Lockdown 元数据，并在设备信息请求时重新读取，因此存储或设备名称变更通知 可以展示当前值。设备详情刷新还会并行通过 MobileActivationd 读取激活状态、通过 DiagnosticsRelay 读取电池信息，并优先通过 AMFI、回退 MobileImageMounter 读取开发者 模式。后端将厂商状态字符串归一化为固定公开枚举，不请求激活记录、证书或 activation-info 载荷。 开发者模式准备使用独立的鉴权命令：建立新的已配对 AMFI 服务，重新读取设备状态，并且 只在未启用时发送 action 0，让设置中的选项显示。应用不会发送会触发重启的启用 action， 也不会发送设备确认 action；设备调用与 API 回复均有有界超时。 设备重命名只接受有界、非空且不含控制字符的 Unicode 名称；会话层会再次校验，建立已配对 的 Lockdown session，写入 `DeviceName` 并读回确认后才返回成功。诊断日志只记录字符数， 不记录请求名称；Lockdown 名称变更通知会刷新设备选择器和当前信息标签页。成功执行 `StartSession` 后，无论重命名成功或失败，后端都会有界尝试 `StopSession`。清理错误只进入 日志，不会把设备已经接受的新名称误报为重命名失败。 应用列表和生命周期控制优先复用同一会话长期持有的 CoreDevice AppService，避免每次操作创建新的 RSD tunnel。只有可执行文件的直接父目录 等于目标 App bundle 时才会匹配进程；停止操作会重新读取设备状态并发送固定 SIGTERM， 客户端不能指定 PID 或信号。缺少 AppService 时，列表回退到 Installation Proxy，运行 状态保持未知。 默认范围只请求可移除的用户 App。显式启用系统 App 后，CoreDevice 会额外包含默认 App， 隐藏和内部 App 仍会被排除。Installation Proxy `Any` 只补充这些 CoreDevice 已知条目的 元数据，绝不会用来构造系统目录。CoreDevice 列表失败时，系统范围会明确报告限制，不返回 不可靠的目录。卸载路径依然会按单个 `User` App 重新查询，绝不会把列表元数据当作授权依据。 同一次经过限制的 Installation Proxy 元数据查询会为两种列表路径补充 `StaticDiskUsage` 和 `DynamicDiskUsage`；数值通过校验后才会越过私有 API，总量使用 checked arithmetic 计算。缺失或异常字段保持未知，不会被错误显示为零。 App 图标通过独立、按请求工作的 SpringBoardServices RSD channel 获取，因此不会占用 HID 分发循环。worker 会校验 PNG header 与尺寸，限制单张 4 MiB，并使用 256 项、32 MiB 的 FIFO 缓存；前端只请求接近可视区域的 App 行。 主屏幕位置使用另一条按请求工作的 SpringBoardServices channel，布局读取不会延迟图标或 HID。 解析器最多接收 32 个列表、每列表 256 项、四层文件夹和 1,024 个唯一 Bundle ID，只返回 App 名称、Bundle ID 以及从 1 开始的 Dock、页面和文件夹顺序路径；另一条独立的 3 秒 channel 可选读取数值图标度量，并限制为布局尺寸、网格数量、Dock 容量和页面上限。Widget、Smart Stack 配置、Web Clip URL 与原始 plist 都不会越过私有 API 或 MCP 边界。布局请求失败后会 丢弃主 client 再重试，度量请求失败则保留已有布局结果。 原生截图使用独立、有界的 worker，优先使用 CoreDevice ScreenCaptureService，失败时回退到 USB Lockdown 或 RSD remote shim 上的 screenshotr。worker 只接受一条排队请求，只复用健康 client，校验 PNG 与尺寸并把响应限制在 32 MiB；截图不会占用 HID 分发循环。

轻 App (App Clips) 是第二个可独立选择的 CoreDevice AppService 范围。`isAppClip` 标识在经过私有 API 与 MCP 归一化后仍会保留；默认范围及 Installation Proxy 后备路径始终把条目标记为普通 App。停止操作会同时解析轻 App，使列表中正在运行的轻 App 可以被终止；卸载授权会排除轻 App，因为其生命周期由 iOS 管理，不属于传统已安装用户 App 路径。请求轻 App 范围而 AppService 不可用时会明确失败，不会静默返回不完整的后备目录。

配对 Apple Watch 发现通过活动 iPhone 传输上的独立、按请求工作的 CompanionProxy RSD channel 完成。worker 延迟建立连接，最多接受两条排队命令，返回最多十六条经过净化的 注册项；请求失败后会丢弃 client，使下一次查询重新连接。它只读取选定的展示元数据， 不启动 Watch 服务、不控制 Watch，也不转发端口。空注册表是有效结果，单项元数据可能 缺失，配对设备标识符仍属于敏感信息。

WebDriverAgent 自动化是复用活动 `IdeviceProvider` 的按需可选服务。它不会后台探测，也不 负责安装、签名或静默启动 WDA。显式启动 Runner 时会先通过 MobileImageMounter 查询与 系统版本对应的 `Developer` 镜像；iOS 17 及以后使用 `Personalized` 类型。只有明确返回 未挂载时才会阻止 XCTest 并给出可操作错误，查询不可用时仍保留原有启动尝试以兼容不同 系统。刷新设备信息会以可空就绪状态返回同一结果，不暴露镜像签名或个性化标识符。 MCP 命令进入容量为 4 的队列并携带 12 秒截止时间；只允许六种 选择器策略、1,024 bytes 的选择器表达式、20 个匹配结果和 1 MiB 的 UI 树响应。工作线程 持有一个 WDA 会话，传输失败后删除或丢弃它，并通过 `device.wda` 报告健康状态，不会拆除 画面、HID 或设备管理服务。只有归一化状态、受限 XML、匹配序号和有限数值矩形会越过 MCP 边界；WDA 会话与元素标识符始终留在工作线程内。

手动挂载开发者磁盘镜像由另一项显式、受监督任务执行。它只接受绝对路径的本地普通文件， 拒绝符号链接，将 DMG 限制为 1.5 GB，并分别限制每个辅助文件。原生文件选择器返回的路径 只通过带 bearer 认证的私有 API，文件内容不会进入 WebView。iOS 16 及以前需要 DMG 与 signature，iOS 17 及以后需要 DMG、trust cache 和包含 非空 `BuildIdentities` 的 `BuildManifest.plist`。个性化挂载使用 idevice 的 TSS 流程，因此 仅在用户确认后向 Apple 服务发送设备相关签名标识符。任务会报告受限的阶段与进度状态， 可由用户取消，设备会话结束时也会中止；应用不会自动查找或下载镜像。 显式卸载在 iOS 17 之前使用 `/Developer`，较新系统使用 `/System/Developer`；挂载、卸载与 取消共用同一单操作队列，不会彼此竞态。

本地设备备份由活动会话持有的显式 MobileBackup2 worker 执行。USB 优先使用 lockdown 服务，并可回退到克隆 RSD tunnel；Wi-Fi 使用 remote RSD shim。worker 只会写入原生目录 选择器指定的主机目录及经过校验的设备标识符子目录。每次 delegate 文件系统操作都会验证 词法边界并拒绝符号链接祖先，包括复用已有备份执行增量备份时。进度回调只发布有界计数， 不会暴露文件路径。取消会立即丢弃 DeviceLink 会话，并保留已传输数据供后续增量备份使用。 恢复、擦除、备份密码变更以及自动或后台备份均不属于此边界。

sysdiagnose 导出是另一项显式任务，通过克隆 RSD tunnel 使用 CoreDevice DiagnosticsService。 同目录临时文件预留成功后请求即返回，设备采集与流式传输继续由会话监督器持有。worker 最长 运行 45 分钟、最多接收 8 GiB，拒绝超大数据块或与设备声明长度不一致的数据流，且只发布 有界计数和所选文件名。 取消或会话结束会丢弃服务流并删除部分文件；完成时会先刷新、同步归档，再原子替换所选目标。 诊断内容不会越过私有 API 或 MCP 边界。

设备网络抓包由独立、用户主动启动的 pcapd worker 执行。USB 会话优先打开传统 lockdown pcapd 服务，并保留克隆 RSD tunnel 作为回退；Wi-Fi 会话通过 RSD 使用 CoreDevice remote pcapd shim。它将标准化 以太网记录直接写入目标同目录的主机临时文件，单包限制为协商的 256 KiB snapshot，完整 抓包限制为 256 MiB，最后原子替换所选 `.pcap` 目标。手动停止、超时、数据流失败和会话 关闭都会执行收尾。私有 API 只接收有界计数与状态，数据包正文不会进入 WebView 或 MCP 传输。

蓝牙 HCI 抓包沿用同一所有权模型管理 idevice 的 `BTPacketLogger` 数据流。它使用 DLT 201 和四字节方向伪头写入大端 PCAP 记录，将抓包限制为最长五分钟和最大 64 MiB，并原子替换 用户选择的目标文件。设备未安装 Bluetooth Logging 配置描述文件时，静默数据流最终会形成 零数据包的有效抓包，应用不会据此虚构服务可用性结论。

重启与关机是两个固定的私有 API 命令，不接受客户端传入任意 DiagnosticsRelay 操作。每个 命令都在有超时的独立任务中建立 relay 连接，因此等待设备确认不会阻塞 HID 分发；前端 必须显示包含设备名称的二次确认。

App 存储由独立受监督的 House Arrest worker 处理。USB 会先通过已配对的 Lockdown provider 连接 House Arrest，失败时回退到克隆的 RSD tunnel；Wi-Fi 只使用 RSD。每条命令都显式 携带 `documents` 或 `container` 范围，并通过 `VendDocuments` 或 `VendContainer` 建立 新的 AFC 会话；API 未传范围时为兼容旧客户端而默认 Documents。逻辑路径分别绑定到 `/Documents` 或 `/`，并拒绝目录穿越和名称中的路径分隔符；符号链接只显示为不可操作的 特殊条目。文件和目录下载使用可回滚的本地暂存，上传先写入唯一远端临时路径，全部流关闭 成功后才改名。递归传输使用迭代遍历，上限为 64 层和 100,000 个条目，拒绝符号链接与特殊 条目，并校验每个普通文件的字节数。上传不会静默覆盖同名项目，根目录禁止修改。递归删除 必须携带显式 API 标志，先按相同深度、条目类型与符号链接约束完成预扫描，再以后序逐项 删除，并在每次删除前立即复验目标。

worker 会按 App 发布当前设备会话内的传输 activity 快照。每复制一个 64 KiB 数据块便累计 字节数，并最多每 100 ms 发布一次。单文件传输会公开已知总字节数；目录传输不额外执行一次 远端预扫描，而是持续报告已完成的字节、文件与目录数量。前端仅在上传或下载请求进行期间 轮询这个只读快照。 取消请求通过 App 与当前会话共同限定的原子令牌直达活动传输，不会在 worker 队列中等待。 复制循环会在每个 64 KiB 数据块和目录条目之间检查令牌、关闭已打开的 AFC 文件描述符， 并尝试删除主机或设备端暂存路径后再报告 `cancelled`。

设备公共文件由另一条受监督的标准 AFC worker 处理。USB 会先通过已配对的 lockdown provider 打开 `com.apple.afc`，失败时可回退到克隆的 RSD tunnel；Wi-Fi 通过 RSD 使用 `com.apple.afc.shim.remote`，任何路径都不会请求 AFC2。worker 会复用客户端直到操作失败， 私有 API 只开放标准 AFC 容器内的有界操作。路径会拒绝目录穿越、反斜杠、NUL 与不安全 组件，符号链接和特殊条目不会被继续访问。文件传输使用 64 KiB 缓冲并校验字节数；导入先 写入唯一远端临时名称，完成后改名，导出则在所选主机目标旁暂存。递归传输使用迭代遍历， 上限为 64 层和 100,000 个条目。写入拒绝同名覆盖、根目录修改与不支持的本地条目类型， 递归删除必须经过前端明确确认；AFC2 路径和 MCP 修改工具均不在此边界内。

AFC 顶层工作台提供四种范围，但不会合并其服务或授权边界。公共 AFC 使用受监督的标准 AFC worker；App Documents 与 App Container 会先按实际可用范围筛选当前 App 目录，再使用现有 的逐 App House Arrest worker；崩溃报告仍是只读 CrashReportCopyMobile 列表与导出路径。 活动传输期间会锁定范围和 App 选择，避免所属 pane、进度快照和取消入口消失。公共浏览器只在 被选中时加载目录，并在设备变化时重置。直接路径输入会先按后端一致的 UTF-8 字节数与路径段 限制校验，但最终目录约束仍以后端为准。

公共 AFC 导入和导出会发布独立的当前会话 activity 快照。整次传输中的所有文件复用一个 64 KiB 缓冲区，并最多每 100 ms 发布字节和条目计数。单文件公开已知总字节数；目录保持 不确定总量，以避免再次遍历设备目录。AFC 标签页仅在传输请求进行期间轮询该快照。 取消请求通过当前会话的原子令牌绕过串行 AFC 命令队列。复制循环会在每个 64 KiB 数据块和 目录条目之间检查令牌，删除主机或设备端暂存路径后再报告 `cancelled`；取消不会使原本健康 的 AFC 客户端失效。

剪贴板同步仅在持久化的显式开关启用并重新连接设备后使用 CoreDevice Pasteboard Service。 设备侧变更优先通过推送接收，电脑侧按有界频率轮询并抑制回环；默认关闭时不会产生 后台剪贴板访问或传输，但显式的单次粘贴仍会写入请求的文本。同步活动通过容量为 8 的 广播通道发送给已鉴权 WebSocket，因此界面提示不会反压设备服务。 单次 Unicode 粘贴通过容量为 4 的命令队列复用同一个 Pasteboard Service 所有者，并且 只在收到 SET 回复后发送 Cmd+V。

IPA 安装和应用卸载使用独立 Tokio 任务及新的 Installation Proxy 连接，因此上传不会 阻塞画面、HID 或应用列表。后端会重新查询卸载目标，只允许移除非 Apple 第一方且标记 为可卸载的用户应用。一个共享操作状态提供阶段和设备上报进度。

当前 `idevice` 包安装辅助逻辑会在 AFC 上传前缓冲整个 IPA，也无法报告字节级上传进度。 因此界面将上传阶段标记为不确定进度，不显示虚构百分比。

崩溃报告列表与导出会在独立 Tokio 任务中建立新的 CrashReportCopyMobile/AFC 会话，因此 递归目录读取和文件传输不会阻塞 HID 分发。列表受目录深度和条目数量限制；导出会重新 验证设备绝对路径与普通文件元数据，把内存分配限制在 128 MiB，并且只向 WebView 返回 元数据。

设备日志仅在日志工作台打开时建立受监督的 OsTraceRelay 连接，并读取统一日志的级别、 进程、PID、子系统、类别和文件名等结构化字段。若连接或启动统一日志活动失败，同一监督器 会回退到 SyslogRelay；运行中的流失败则触发受监督重连，不会在连接中静默切换来源。正文 会清理控制字符并限制为 16 KiB，元数据单字段限制为 512 bytes，随后进入两种来源共享的 最多 2,000 条内存环形缓冲区；私有 API 每次最多返回 500 条，并报告游标落后造成的缺口。 手机日志不会进入应用自身的 tracing 日志，也不会自动持久化。

每个活动设备会话都会运行受监督的 Lockdown 心跳：收到设备的 `Marco` 后回复 `Polo`， 限制设备提供的等待间隔，并在休眠、超时或传输失败后重连。心跳属于可选服务，不会阻止 视频、输入或设备管理启动；其生命周期通过共享服务健康注册表公开。

## 视频管线

CoreDevice displayservice 输出 RTP/HEVC。后端先组装完整 HEVC Access Unit，再进入 16 MiB 字节上限队列；溢出时丢弃依赖帧直至 IRAP，并通过 PLI/FIR 请求恢复。FFmpeg 默认输出 自描述的 RGB24 PAM 帧。实验性 YUV420P 设置（也可通过 `DEVICEHUB_VIDEO_PIXEL_FORMAT=yuv420p` 选择）输出 YUV4MPEG2，并将 planar YUV420P 直接交给 TurboJPEG，避免 RGB 转换并将解码帧带宽减半。 最新帧通过 `watch` 通道按事件通知 WebSocket，取消固定频率轮询；慢消费者只会看到最新帧， 不会积压陈旧画面。

默认启用的实验性“浏览器 / WebCodecs”后端在同一个有界 Access Unit 队列后分流。Rust 通过已鉴权 WebSocket 发布带版本头的 Annex-B HEVC Access Unit。WebView 从各数据流的 SPS 推导 RFC 6381 HEVC profile、tier、level、compatibility flags 和 constraints，再使用 `VideoDecoder` 解码并把 `VideoFrame` 绘制到现有 Canvas。压缩帧广播缓冲可在 60 FPS 下吸收约半秒的短时 WebSocket 阻塞。广播落后、序列断点、解码队列积压、解码输出超时或配置变化时会丢弃依赖帧并进入明确的重同步状态；重同步期间后端停止发送增量帧，按受限间隔重复 PLI/FIR，直到 IRAP 确实发送给 WebView。设备解码继续而 WebSocket 输出为零时，前端指标也会补发恢复请求。能力检测、输出超时或运行时连续失败会自动重连并回退原生后端。初始化时会探测带真实尺寸与 codec 的准确配置；如果 WebKit 报告支持但 `configure()` 拒绝，应用会依次尝试更简化的 `hev1` 和 `hvc1` 配置，全部失败后才触发原生回退。WebCodecs 的 `EncodedVideoChunk.timestamp` 来自 90 kHz RTP 时钟而非固定 60 FPS 计数，因此会保留设备的实际帧间隔、可变帧率以及更高刷新率。SPS/codec 扫描仅发生在首次、关键帧或尺寸变化时，普通依赖帧不再重复遍历整个压缩 Access Unit。

WebSocket 客户端会显式声明实时画面需求。仅设备控制页和使用实时背景的按键映射页接收视频； 切换到设置、AFC、日志等页面、使用静态映射截图或窗口不可见时，后端仍维持 RTP/RTCP 会话， 但停止向该 WebView 复制和发送画面。恢复显示时主动请求 IRAP，避免用缺少参考帧的 P-frame 恢复。此门控只作用于 UI 显示，MCP 截图继续使用按需 CoreDevice ScreenCaptureService，帧同步 同时观察原生与浏览器帧版本。

Axum 使用线程本地复用的 TurboJPEG compressor 编码最新帧。每个 WebView 最多允许两帧 在途，使后端 JPEG 编码与 WebView JPEG 解码可以重叠，同时不会形成无限队列。前端会对 已解码呈现或主动替换的帧发送确认；500ms credit 租约避免确认丢失时永久停帧。

Windows 默认将解码长边限制在 1920 像素。FFmpeg 始终保持比例、不放大，并输出偶数 尺寸。设置 `DEVICEHUB_VIDEO_MAX_DIMENSION=0` 可使用原始分辨率，低性能设备可设置 更小值。

浏览器后端不应用 FFmpeg 尺寸上限。它要求平台 WebView 通过 WebCodecs 暴露 HEVC；Windows 通常还需要系统 HEVC Video Extensions。应用会在运行时探测并配置 SPS 推导出的准确解码 配置，不会仅凭 WebCodecs 存在就假定 HEVC 可用。

Canvas 使用同一个比例 contain-fit 旋转后的源画面。鼠标坐标只在准确显示矩形内归一化， 避免横屏拉伸和触控偏移。

## 音频管线

CoreDevice 协商 48 kHz 双声道 AAC-ELD，每个 RTP 包包含一个 10 ms Access Unit。设备发送 裸 Access Unit，因此后端先补充 RFC 3640 AU header，再将 RTP 转发给 FFmpeg。FFmpeg 解码为交错 S16LE，并把有界的 20 ms PCM 块交给独立原生输出线程。Rodio 将音频转换为 主机默认输出格式；排队超过 240 ms 时清除旧音频，因此输出背压不会阻塞 RTP、视频或输入。 PCM 不再经过 WebSocket 或 WebView。

设备音频默认关闭。静音和音量由后端持久化并直接作用于原生输出，不依赖浏览器自动播放 策略或页面可见性。输出设备故障和解码器运行中退出都会有界重试；若解码器根本无法启动， 则继续静默 drain 协商后的流，不会终止视频或输入。

## 输入管线

React 合并映射、鼠标和键盘状态，并跳过完全相同的触控帧。Rust 将校验后的类型化触点 转换成一个固定五槽位 Universal HID 多点触控 report。键盘和硬件命令保留按下与释放 状态，断线清理会释放所有按住的 usage。

映射模式和键盘透传互斥，避免一个物理按键同时产生映射触控和键盘 usage。

## 描述文件数据

描述文件由独立监督的有界 Misagent 命令服务管理，因此相关操作不会阻塞 HID 输入循环。 在 plist 元数据进入私有 API 前会先解码 CMS SignedData。原始描述文件和 provisioned device identifiers 不会进入前端；单个损坏文件会被隔离，不会导致整个列表失败。

安装和移除命令携带请求截止时间，避免 HTTP 请求超时后排队操作仍然延迟生效。安装前会 验证本地文件和描述文件元数据，移除前后都会重新读取设备目录。输入错误、不存在、冲突、 传输故障和超时在私有 API 中保持类型化语义；只有传输故障和超时才会让监督器重建 Misagent channel。

如果 displayservice 不可用，但 Lockdown 仍可使用，后端会保留降级管理会话。画面控制 和仅 AppService 支持的操作会明确标记为不可用，而不是隐藏整个设备。

## 依赖固定版本

`idevice` 暂时固定到项目 fork 中已审查的 `5e89583` revision。该版本包含 iOS 27 CoreDevice 修复、iOS 27 AppService 所需的显式容器访问选项，以及性能工作台使用的 类型化 DVT NetworkMonitor 与 EnergyMonitor 客户端。等价修复合并并正式发布后， 应切回上游版本。

## 安全边界

- 私有 API 始终只监听回环地址并要求令牌。
- MCP 默认只监听回环地址且没有鉴权，会暴露可能敏感的截图、进程名称、设备日志和崩溃 报告、UI 树，监听非回环地址时会输出警告。
- 前端应用元数据不会被用作卸载授权。
- HID report 只在后端验证后构建。
- 更新产物必须通过 Tauri 签名验证才能安装。
- Apple Developer ID 签名与应用更新签名彼此独立。
