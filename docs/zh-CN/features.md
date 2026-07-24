# 功能参考

简体中文 | [English](../en/features.md) | [文档首页](README.md) | [使用指南](user-guide.md)

本页是 DeviceHub Mask 当前已实现能力的简明清单。[使用指南](user-guide.md)提供具体 工作流、安全限制和故障语义。实际可用性仍取决于连接设备及当前 iOS 版本开放的服务。

## 桌面工作区

| 工作区 | 已实现能力 |
| --- | --- |
| **设备** | USB/Wi-Fi 设备选择、应用内 USB 信任配对、显式重新连接、实时画面、鼠标直接触控、映射与键盘透传模式、旋转、原生截图、WebView 支持时的画面录制、Unicode 粘贴、设备音频静音、硬件按键、设备画面全屏，以及支持普通/系统/轻 App 范围的设备检查器 |
| **键盘映射** | 可视化放置与编辑、实时或冻结截图背景、配置创建/复制/重命名/导入/导出、scrcpy-mask `0.0.1` 兼容、PlayCover `2.0.0` 导入、App 与配置关联、硬件按键快捷键 |
| **AFC** | 统一的公共 AFC、App Documents、App Container 与崩溃报告工作台；可搜索 App 选择、有界浏览与传输、新建、重命名、确认式递归删除、进度、取消及确认式崩溃报告删除 |
| **性能** | iPhone CPU/进程/内存/能耗、可搜索的按需运行进程清单、Core Animation FPS、GPU 内存、网络速率、App 活动、视频管线指标、服务健康、DVT 网络/热状态、网络 PCAP 和蓝牙 HCI PCAP |
| **设备日志** | 按需结构化统一日志、SyslogRelay 回退、搜索、级别筛选、暂停、自动滚动、复制、清空、有界缓冲、恢复状态，以及经确认导出最近 1/6/24 小时的离线统一日志归档 |
| **虚拟定位** | DVT 优先并回退传统服务的定位设置、经纬度输入、内置地点预设、后端状态和显式恢复真实定位 |
| **设置** | 语言、窗口置顶、系统全屏、检查器显示、画面比例、映射覆盖层、旋转控制锁定、设备全屏工具栏行为、解码器与像素格式、音频、剪贴板同步、可配置性能 HUD、更新、Debug 日志和日志目录 |

系统全屏与设备画面全屏是两个不同功能。系统全屏改变桌面窗口状态；设备画面全屏会隐藏 导航与检查器，让手机画面和必要控制占用当前窗口的可用空间。

## 设备检查器

### 设备信息

- 刷新 Lockdown 身份、iOS/build 版本、硬件型号、规范化语言/地区格式/时区设置、存储、激活状态、电池健康与充电信息。
- 通过已配对 Lockdown session 修改设备名称，并读回验证结果。
- 可经二次确认显式撤销 USB Lockdown 信任并删除电脑配对记录，同时报告部分成功状态。
- 显示开发者模式与开发者磁盘镜像状态；可显示开发者模式设置入口，并显式挂载、取消或卸载 匹配的本地镜像文件集。
- 通过 CompanionProxy 读取已配对 Apple Watch 元数据，但不控制 Watch。
- 创建或续传未加密的本地 MobileBackup2 备份，支持进度、取消和可选强制完整备份。
- 采集有界且可取消的 CoreDevice sysdiagnose 归档。
- 通过 Diagnostics Relay 提供需要确认的**重启设备**和**关闭设备**。两者都会主动结束当前 设备会话；关机后必须手动重新开机。

设备工具栏中的 Lock 会模拟硬件键按下和释放，因此可能唤醒已经锁定的设备。MCP `lock_device` 才是独立的单向 Diagnostics Relay sleep 请求，不会唤醒已锁定设备。

### App

- 通过 CoreDevice AppService 列出用户 App，并可按需列出 Apple 默认 App；用户 App 目录可 回退 Installation Proxy。
- 可通过 CoreDevice OpenStdioSocket 显式启动开发者 App 或第三方 App，并在当前会话内有界采集 stdout/stderr。
- 在设备允许时显示原生图标、版本、签名类型、可移除状态、上报存储、运行状态，以及 SpringBoard Dock/页面/文件夹位置。
- 支持启动、重新启动、停止、安装新 IPA、通过 IPA 显式升级已安装 App，以及安全卸载符合条件的用户 App。上传前会显示经过限制的 IPA 元数据，并针对活动设备核对操作类型、最低系统版本、设备族和声明的所需能力；操作由当前会话持有，并报告进度或失败。
- iOS 允许时通过 House Arrest 打开 Documents 或完整 Container，执行有界的文件与目录 传输和修改。
- 可将 App 关联到已保存的按键映射配置；从 App 列表启动时会激活对应配置。
- 可显式启动和停止已安装、开发者签名的 WebDriverAgent `.xctrunner`；应用不会安装或签名 WDA。

### 描述文件与崩溃报告

- 通过 Misagent 列出描述文件。本地 `.mobileprovision` 安装会校验 CMS、UUID、大小与过期 状态；移除需要确认，并通过刷新后的设备目录验证。有效的开发描述文件可在确认后显式请求 AMFI 信任 App 签名者。
- 通过 CrashReportCopyMobile 列出、搜索、导出崩溃报告，并可在确认后逐条删除。MCP 保持只读，只能为 Agent 诊断读取另行限制大小的文本片段。

## 画面、音频与输入

| 领域 | 当前行为 |
| --- | --- |
| 视频 | CoreDevice HEVC，最高 60 FPS；Native/FFmpeg 是兼容路径，实验性 WebCodecs 失败时自动回退 |
| 画面录制 | 通过系统 WebView 的 MediaRecorder 以最高 60 FPS 录制已渲染 Canvas，并下载 MP4 或 WebM；切页或切换设备时停止，不包含主机原生播放的设备音频 |
| 像素格式 | 默认 RGB24；YUV420P 为实验选项，环境变量强制指定时界面不可修改 |
| 音频 | 可选 CoreDevice AAC-ELD 采集、主机原生播放、音量和静音；启用采集后需要重新连接 |
| 剪贴板 | 单次 Unicode 粘贴始终可用；可选文本/图片双向同步需要重新连接 |
| 触控 | 鼠标直接输入与映射输出合并为经过校验的五触点 Universal HID report |
| 键盘 | 映射模式与原始 HID 键盘透传互斥；失焦、切页、切模式、全屏变化和断线都会释放按住的输入 |
| 硬件按键 | Home、Lock、音量加减、静音、Siri、Action，以及随配置保存的键盘快捷键 |

## idevice 服务覆盖

| 能力 | 主要服务 |
| --- | --- |
| 设备身份、名称、区域设置、存储回退 | Lockdown |
| 画面、音频、方向、剪贴板、HID | CoreDevice display、orientation、Pasteboard 和 HID 服务 |
| App 列表、进程状态、启动、停止 | CoreDevice AppService |
| 显式带控制台启动 App | CoreDevice AppService + OpenStdioSocket |
| IPA 安装与用户 App 回退 | Installation Proxy |
| App Documents/Container | House Arrest 和 AFC |
| 公共媒体文件 | 标准 AFC / remote AFC shim |
| 电池与设备电源操作 | Diagnostics Relay |
| 开发者模式与镜像 | AMFI 和 MobileImageMounter |
| 描述文件与显式签名者信任 | Misagent 和 AMFI |
| 备份 | MobileBackup2 |
| sysdiagnose | CoreDevice DiagnosticsService |
| 设备日志与离线归档 | OsTraceRelay / SyslogRelay |
| 性能、进程与设备状态模拟 | DVT DeviceInfo、Sysmontap、Graphics、Energy、Network Monitor、Notifications、Condition Inducer |
| 虚拟定位 | DVT Location Simulation，并回退 `com.apple.dt.simulatelocation` |
| 网络/蓝牙抓包 | pcapd 和 BTPacketLogger |
| Watch 元数据 | CompanionProxy |
| App 图标 | CoreDevice AppService，回退 SpringBoardServices |
| 主屏幕布局 | SpringBoardServices |
| 崩溃报告 | CrashReportCopyMobile |
| 语义自动化 | WebDriverAgent 和 XCTest runner 服务 |

## MCP 工具覆盖

桌面应用运行时，Streamable HTTP MCP 端点提供以下工具：

- 画面与输入：`screenshot`、`tap`、`swipe`、`multi_touch`、`wait_for_frame`、 `type_text`、`paste_text`、`press_key`、`press_button`、`rotate`。
- 设备与会话：`status`、`device_details`、`list_devices`、`connect_device`、 `reconnect_device`、`lock_device`、`wait_for_device_event`、 `list_companion_devices`、`home_screen_layout`。
- App 与诊断：`list_apps`、`launch_app`、`stop_app`、`list_processes`、`list_crash_reports`、`read_crash_report`、`performance_snapshot`、`recent_device_logs`。
- 定位与条件：`set_location`、`clear_location`、`list_device_conditions`、 `apply_device_condition`、`clear_device_condition`。
- WDA：`wda_runner_status`、`wda_start`、`wda_stop`、`wda_status`、 `wda_ui_tree`、`wda_find_elements`、`wda_click`。

MCP 当前开放单向锁屏，但不开放设备重启或关机。重启与关机已经在桌面“设备信息”页实现，并要求交互式确认。MCP 也不开放 IPA 安装、升级或卸载、AMFI 签名者信任、AFC 修改、备份、sysdiagnose、统一日志归档导出、描述文件修改、抓包或开发者磁盘镜像修改。

## 有意保留的边界

- 不提供设备恢复、抹除、备份密码管理或后台自动备份。
- 不提供 AFC2/root 文件系统访问，不跟随符号链接。
- 不提供 Apple Watch 控制或端口转发。
- 不自动安装/签名 WDA，不自动下载或猜测开发者磁盘镜像版本。
- 不自动启用设备条件；必须显式选择配置，并在测试后恢复正常状态。
- 不宣称支持 120 FPS 画面；当前协商和渲染管线最高为 60 FPS。
- Wi-Fi 和远程服务可用性仍取决于配对、主机发现、Apple 服务及 iOS 策略。
