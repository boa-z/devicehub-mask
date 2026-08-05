# 功能参考

简体中文 | [English](../en/features.md) | [文档首页](README.md) | [使用指南](user-guide.md)

本页是 DeviceHub Mask 当前已实现能力的简明清单。[使用指南](user-guide.md)提供具体 工作流、安全限制和故障语义。实际可用性仍取决于连接设备及当前 iOS 版本开放的服务。

## 桌面工作区

| 工作区 | 已实现能力 |
| --- | --- |
| **设备** | USB/Wi-Fi 设备选择、应用内 USB 信任配对、显式重新连接、实时画面、鼠标直接触控、映射与键盘透传模式、可选的注入触摸调试层（方向感知坐标与轨迹）、旋转、原生截图、WebView 支持时的画面录制、Unicode 粘贴、设备音频静音、硬件按键、设备画面全屏，以及支持普通/系统/轻 App 范围的设备检查器 |
| **键盘映射** | 可视化放置与编辑、实时或冻结截图背景、配置创建/复制/重命名/导入/导出、scrcpy-mask `0.0.1` 兼容、PlayCover `2.0.0` 导入、App 与配置关联、硬件按键快捷键 |
| **AFC** | 统一的公共 AFC、App Documents、App Container 与崩溃报告工作台；可搜索 App 选择、有界浏览与传输、新建、重命名、确认式递归删除、进度、取消及确认式崩溃报告删除 |
| **性能** | iPhone CPU/进程/内存/能耗、有界的逻辑/物理核心与物理内存容量、可搜索的按需运行进程清单、Core Animation FPS、GPU 内存、网络速率、App 活动、视频管线指标、服务健康、DVT 网络/热状态、全设备或按进程过滤的网络 PCAP 和蓝牙 HCI PCAP |
| **设备日志** | 按需结构化统一日志、SyslogRelay 回退、搜索、级别筛选、暂停、自动滚动、复制、清空、有界缓冲、恢复状态，以及经确认导出最近 1/6/24 小时的离线统一日志归档 |
| **虚拟定位** | DVT 优先并回退传统服务的定位设置、经纬度输入、内置地点预设、后端状态和显式恢复真实定位 |
| **设置** | 语言、窗口置顶、系统全屏、检查器显示、画面比例、映射覆盖层、可选注入触摸调试、旋转控制锁定、设备全屏工具栏行为、音频、剪贴板同步、可配置性能 HUD、更新、Debug 日志和日志目录 |

系统全屏与设备画面全屏是两个不同功能。系统全屏改变桌面窗口状态；设备画面全屏会隐藏 导航与检查器，让手机画面和必要控制占用当前窗口的可用空间。

## 设备检查器

### 设备信息

- 刷新 Lockdown 身份、iOS/build 版本、有界的设备类别、CPU 架构、型号编号与机身颜色字段、规范化语言/地区格式/时区设置、存储、激活状态，以及有界的电池健康、温度与充电信息。
- 收到归一化的语言/时区或开发者磁盘镜像挂载通知后自动刷新信息标签页，不暴露厂商通知载荷。
- 通过已配对 Lockdown session 修改设备名称，并读回验证结果。
- 可经二次确认显式撤销 USB Lockdown 信任并删除电脑配对记录，同时报告部分成功状态。
- 显示开发者模式与开发者磁盘镜像状态；可显示开发者模式设置入口，并显式挂载、取消或卸载 匹配的本地镜像文件集。
- 通过 CompanionProxy 读取已配对 Apple Watch 元数据，但不控制 Watch。
- 仅在用户显式点击后通过 SpringBoardServices 打开主屏幕或锁定屏幕的只读壁纸预览；预览不会持久化，也不通过 MCP 开放。
- 创建或续传未加密的本地 MobileBackup2 备份，支持进度、取消和可选强制完整备份。
- 采集有界且可取消的 CoreDevice sysdiagnose 归档。
- 通过 Diagnostics Relay 提供需要确认的**重启设备**和**关闭设备**。两者都会主动结束当前 设备会话；关机后必须手动重新开机。

设备工具栏中的 Lock 会模拟硬件键按下和释放，因此可能唤醒已经锁定的设备。MCP `lock_device` 才是独立的单向 Diagnostics Relay sleep 请求，不会唤醒已锁定设备。

### App

- 通过 CoreDevice AppService 列出用户 App，并可按需列出 Apple 默认 App；用户 App 目录可 回退 Installation Proxy。
- 可通过 CoreDevice OpenStdioSocket 显式启动开发者 App 或第三方 App，并在当前会话内有界采集 stdout/stderr。
- 在设备允许时显示原生图标、版本、签名类型、可移除状态、上报存储、运行状态，以及 SpringBoard Dock/页面/文件夹位置。
- 支持启动、重新启动、停止，以及安全卸载符合条件的用户 App。卸载前会根据设备当前元数据重新鉴权，操作由当前会话持有，并报告进度或失败。
- iOS 允许时通过 House Arrest 打开 Documents 或完整 Container，执行有界的文件与目录 传输和修改。
- 可将 App 关联到已保存的按键映射配置；从 App 列表启动时会激活对应配置。
- 可显式启动和停止已安装、开发者签名的 WebDriverAgent `.xctrunner`；应用不会安装或签名 WDA。

### 描述文件与崩溃报告

- 通过 Misagent 列出描述文件。本地 `.mobileprovision` 安装会校验 CMS、UUID、大小与过期 状态；移除需要确认，并通过刷新后的设备目录验证。有效的开发描述文件可在确认后显式请求 AMFI 信任 App 签名者。
- 通过 CrashReportCopyMobile 列出、搜索、导出崩溃报告，并可在确认后逐条删除。MCP 保持只读，只能为 Agent 诊断读取另行限制大小的文本片段。

## 画面、音频与输入

| 领域 | 当前行为 |
| --- | --- |
| 视频 | CoreDevice HEVC 压缩 Access Unit 传输，并固定使用 WebCodecs 解码 |
| 画面录制 | 通过系统 WebView 的 MediaRecorder 以最高 60 FPS 录制已渲染 Canvas，并下载 MP4 或 WebM；切页或切换设备时停止，不包含主机原生播放的设备音频 |
| 音频 | 可选 CoreDevice AAC-ELD 采集、主机原生播放、音量和静音；启用采集后需要重新连接 |
| 剪贴板 | 单次 Unicode 粘贴始终可用；可选文本/图片双向同步需要重新连接 |
| 触控 | 鼠标直接输入与映射输出合并为经过校验的五触点 Universal HID report |
| 键盘 | 映射模式与原始 HID 键盘透传互斥；失焦、切页、切模式、全屏变化和断线都会释放按住的输入 |
| Keymap 脚本 | 有界虚拟时间程序共用 Rust 桌面/MCP 运行时；不提供 shell、文件、环境、进程或网络访问 |
| 硬件按键 | Home、Lock、音量加减、静音、Siri、Action，以及随配置保存的键盘快捷键 |
| 系统控制 | 通过与原生兼容的双击 Home HID 事件打开 App 切换器 |

## idevice 服务覆盖

| 能力 | 主要服务 |
| --- | --- |
| 设备身份、名称、区域设置、存储回退 | Lockdown |
| 实时画面、音频、方向、剪贴板、HID | CoreDevice display、orientation、Pasteboard 和 HID 服务 |
| 原生截图 | CoreDevice ScreenCaptureService、screenshotr 和最终 DVT Screenshot 递进回退 |
| App 列表、进程状态、停止与启动后备 | CoreDevice AppService |
| App 启动 | DVT ProcessControl，并提供仅限发送前的 CoreDevice 回退 |
| 显式带控制台启动 App | CoreDevice AppService + OpenStdioSocket |
| 用户 App 元数据回退与安全卸载 | Installation Proxy |
| App Documents/Container | House Arrest 和 AFC |
| 公共媒体文件 | 标准 AFC / remote AFC shim |
| 有界的电池健康/温度与设备电源操作 | Diagnostics Relay |
| 开发者模式与镜像 | AMFI 和 MobileImageMounter |
| 描述文件与显式签名者信任 | Misagent 和 AMFI |
| 备份 | MobileBackup2 |
| sysdiagnose | CoreDevice DiagnosticsService |
| 设备日志与离线归档 | OsTraceRelay / SyslogRelay |
| 性能、进程与设备状态模拟 | DVT DeviceInfo、Sysmontap、Graphics、Energy、Network Monitor、Notifications、Condition Inducer |
| 只读网络接口目录 | DVT DeviceInfo，不包含 IP 或 MAC 地址 |
| 虚拟定位 | DVT Location Simulation，并回退 `com.apple.dt.simulatelocation` |
| 全设备/按进程网络抓包与蓝牙抓包 | pcapd 数据包 PID/effective PID 元数据和 BTPacketLogger |
| Watch 元数据 | CompanionProxy |
| App 图标 | CoreDevice AppService，回退 SpringBoardServices |
| 主屏幕布局与按需壁纸预览 | SpringBoardServices |
| 崩溃报告与归一化摘要 | CrashReportCopyMobile |
| 语义自动化 | WebDriverAgent 和 XCTest runner 服务 |

## MCP 工具覆盖

客户端配置、坐标规则、推荐 Agent 工作流、WDA 前置条件和故障排查请查看 [MCP 自动化指南](mcp.md)。

桌面应用运行时，Streamable HTTP MCP 端点提供以下工具：

- 画面与输入：`screenshot`、`observe_game`、`tap`、`swipe`、`multi_touch`、`wait_for_frame`、`type_text`、`paste_text`、`press_key`、`press_button`、`app_switcher`、`rotate`。`observe_game` 为 Agent 循环提供无网格画面和可选的归一化感兴趣区域。
- Key Mapping：`list_keymap_profiles`、`get_keymap_profile`、`save_keymap_profile`、`run_keymap`、`start_game_session`、`set_game_input` 和 `stop_game_session`。Agent 可以创建 native v2 配置，并在自身选中的设备上运行持续的 60Hz 映射回放；续期租约会在 Agent 停止更新时自动释放控制。有界脚本需要 MCP 显式选择启用。
- 设备与会话：`status`、`device_details`、`list_devices`、`connect_device`、 `reconnect_device`、`lock_device`、`wait_for_device_event`、 `list_companion_devices`、`home_screen_layout`。
- App 与诊断：`list_apps`、`launch_app`、`stop_app`、`app_status`、`wait_for_app`、`list_processes`、`process_status`、`wait_for_process`、`list_crash_reports`、带归一化摘要的 `read_crash_report`、`performance_snapshot`、`recent_device_logs`。
- 定位与条件：`set_location`、`clear_location`、`list_device_conditions`、 `apply_device_condition`、`clear_device_condition`。
- WDA：`wda_runner_status`、`wda_start`、`wda_stop`、`wda_status`、 `wda_device_state`、`wda_unlock`、`wda_ui_tree`、`wda_find_elements`、 `wda_inspect_element`、`wda_wait_for_element`、`wda_click`、 `wda_type_text`、`wda_double_tap`、`wda_touch_and_hold`、`wda_scroll` 和 `wda_background_app`。

MCP 当前开放单向锁屏，但不开放设备重启或关机。重启与关机已经在桌面“设备信息”页实现，并要求交互式确认。App 安装和升级在 DeviceHub Mask 的任何界面都不存在；MCP 另外不开放 App 卸载、AMFI 签名者信任、AFC 修改、备份、sysdiagnose、统一日志归档导出、描述文件修改、抓包或开发者磁盘镜像修改。

## 有意保留的边界

- App 安装、sideloading、签名和基于 IPA 的升级是明确的非目标。后续功能完善不得加入这些能力；请使用专门工具准备和部署 App。
- 不提供设备恢复、抹除、备份密码管理或后台自动备份。
- 不提供 AFC2/root 文件系统访问，不跟随符号链接。
- 不提供 Apple Watch 控制或端口转发。
- 不自动安装/签名 WDA，不自动下载或猜测开发者磁盘镜像版本。
- 不自动启用设备条件；必须显式选择配置，并在测试后恢复正常状态。
- 不宣称支持 120 FPS 画面；当前协商和渲染管线最高为 60 FPS。
- Wi-Fi 和远程服务可用性仍取决于配对、主机发现、Apple 服务及 iOS 策略。
