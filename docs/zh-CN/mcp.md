# MCP 自动化指南

简体中文 | [English](../en/mcp.md) | [文档首页](README.md)

DeviceHub Mask 通过内置 Model Context Protocol (MCP) 服务，将已连接的 iOS 设备会话提供给 Agent。本指南说明连接配置、可靠控制流程、诊断、WebDriverAgent (WDA)、安全边界，以及有意不向 MCP 开放的操作。

## 连接前准备

先启动 DeviceHub Mask 桌面应用并至少连接一台设备。MCP 服务只在桌面应用运行期间存在。每条 MCP 协议连接独立选择目标，并复用该设备的 CoreDevice 会话、视频流、输入队列、性能服务和有界日志缓冲；它不会建立竞争连接，也不会改变桌面 UI 或其他 MCP client 的目标。

默认 Streamable HTTP 端点为：

```text
http://127.0.0.1:8009/mcp
```

例如，可以将它注册到 Claude Code：

```sh
claude mcp add --transport http devicehub-mask http://127.0.0.1:8009/mcp
```

注册后先调用 `status`。如果该 MCP 连接尚未选择目标，调用 `list_devices`，再把返回的准确选择 ID 传给 `connect_device`。USB 与 Wi-Fi 条目可能对应同一台物理设备，但其选择 ID 不同；同一 UDID 同时只允许一条传输运行。

MCP 端点没有鉴权，必须保持监听回环地址。`DEVICEHUB_MCP_ADDR` 可以修改监听地址，但对非回环地址开放后，网络客户端将能够访问设备截图、输入、App 控制、进程名称、日志、崩溃报告、虚拟定位和 WDA 输出。应用会对非回环监听输出警告；只有主机位于可信隔离网络时才应这样配置。

## 选择正确的控制路径

DeviceHub Mask 提供三种不同的坐标概念，它们不能互换。

| 来源 | 坐标含义 | 适用操作 |
| --- | --- | --- |
| `screenshot` | 返回图片中的像素 | `tap`、`swipe` 和 `multi_touch` |
| `home_screen_layout` | 从 1 开始的 Dock、页面和文件夹顺序 | 判断 App 的组织位置，不能用于点击 |
| WDA 元素矩形 | WDA 逻辑窗口单位 | WDA 语义检查与操作 |

游戏和视觉界面应使用基于截图的 Universal HID；它延迟更低，也不依赖辅助功能元数据。表单、具名控件、状态检查，以及语义选择器比像素更可靠的流程适合使用 WDA。

## 截图与 HID 流程

1. 调用 `screenshot`。坐标网格默认开启，长边默认缩放到 1,024 像素；只有确实需要原始分辨率时才设置 `max_dim=0`。
2. 读取结果中的 `image_width` 和 `image_height`。
3. 在返回的图片中定位目标。
4. 将相同尺寸传给 `tap`、`swipe` 或 `multi_touch`。DeviceHub Mask 会应用当前方向转换和截图缩放比例。
5. 在执行依赖前一步结果的操作前，检查下一张截图或使用画面帧同步。

`tap` 默认按住 100ms，`hold_ms` 会限制在 25 至 5,000ms。`swipe` 默认持续 300ms，时长限制为 50 至 5,000ms。`multi_touch` 支持一至五条同步触控路径，默认持续 250ms，时长限制为 25 至 5,000ms。起点与终点相同表示按住按钮。

例如，下面的操作会在移动左侧摇杆的同时按住右侧动作按钮：

```json
{
  "contacts": [
    { "x1": 180, "y1": 700, "x2": 240, "y2": 650 },
    { "x1": 850, "y1": 680, "x2": 850, "y2": 680 }
  ],
  "duration_ms": 250,
  "image_width": 1024,
  "image_height": 768
}
```

`type_text` 发送可打印 HID 文本。CJK 或其他 Unicode 文本应使用 `paste_text`，它会写入设备剪贴板并发送 Cmd+V。`press_key` 支持 Enter、Escape、方向键、Home、End、Page Up 和 Page Down 等导航键。`press_button` 将 `home`、`lock`、`volume-up`、`volume-down`、`mute`、`siri` 或 `action` 作为硬件按钮操作。

`app_switcher` 通过与原生兼容的双击 Home HID 事件打开 iPhone App 切换器。它是独立的系统动作，不属于单次硬件按键操作。

`lock_device` 与 `press_button` 的 `button="lock"` 不同：`lock_device` 发送单向 Diagnostics Relay 休眠请求，不能唤醒已经锁定的设备；硬件锁定键模拟物理按钮切换，可能将其唤醒。

## 低延迟游戏流程

`tap` 和 `swipe` 默认等待画面稳定，适合普通界面自动化，但会增加连续游戏操作的延迟。`multi_touch` 默认不等待稳定。

对于延迟敏感的循环：

1. 使用 `wait_for_settle=false` 发送操作。
2. 保存返回的 `frame_version_after`。
3. 调用 `wait_for_frame`，将该值作为 `after_version`。
4. 确认出现更新画面后再获取下一张截图。

`wait_for_frame` 默认超时两秒，接受 1 至 10,000ms。超时表示请求期间没有出现更新的解码帧；它本身不能证明设备会话已经断开，也不能证明静止 App 的画面异常。应先检查 `status`，只有会话状态同时表明故障时才重试连接。

坐标操作和 WDA 修改操作共用手势锁，因此两个 Agent 操作不会交错各自的触控流。但串行化不会让旧截图自动变成最新状态；方向或布局可能变化时必须重新截图。

## Key Mapping 工作流

MCP 可以创建和回放与桌面“Key Mapping”工作区共用的本地 native v2 配置。`list_keymap_profiles` 返回有效的本地配置及其 App/分辨率元数据；`get_keymap_profile` 读取完整配置；`save_keymap_profile` 创建配置，或仅在 `overwrite=true` 时替换同名配置。保存不会切换桌面端当前激活的配置。保存时省略 `hardwareBindings` 会将全部硬件快捷键初始化为空。

映射位置使用 0 到 1 的显示比例，不是截图像素。截图后，应将目标的像素 `x` 除以 `image_width`、像素 `y` 除以 `image_height`，再写入映射。下面的参数会创建一个包含移动方向盘和动作键的可移植游戏配置：

```json
{
  "name": "example-game",
  "mappings": [
    {
      "id": "move",
      "type": "dpad",
      "contactId": 0,
      "x": 0.23,
      "y": 0.73,
      "radius": 0.1,
      "keys": { "up": "KeyW", "down": "KeyS", "left": "KeyA", "right": "KeyD" }
    },
    {
      "id": "skill-1",
      "type": "touch",
      "contactId": 1,
      "x": 0.78,
      "y": 0.72,
      "key": "Space"
    }
  ]
}
```

调用 `run_keymap` 时提供已保存的配置名，以及一个或多个浏览器 `KeyboardEvent.code` 值，例如 `KeyW`、`Space` 或 `KeyJ`。它会同时按住所有指定按键 `hold_ms`（默认 100ms，限制在 25 至 5,000ms），在需要时持续发送多点触控帧，并在返回前确保释放触点和硬件按键。操作只作用于该 MCP 连接显式选择的设备，不会自动激活桌面 UI 的配置。

实时游戏应使用 `start_game_session`，再通过 `set_game_input` 提供完整的按住键集合。设备侧会话以 60Hz 计算映射，因此连点、滑动和按住会在 Agent 调用之间持续执行。每次更新都会续期短租约（默认 1,500ms，范围 250 至 30,000ms）；较长租约可覆盖原生截图和视觉推理时间，但应使用满足工作流的最短值。租约到期、调用 `stop_game_session`、设备输入失败或会话关闭时，系统都会释放全部触点和硬件按键。游戏控制会话只占用其选中的设备；切换设备或交还给其他工作流前必须停止。

观察循环使用 `observe_game`。它可以等待比指定 `frame_version` 更新的帧，返回无网格图像，并在传输前裁剪归一化的感兴趣区域。裁剪坐标只用于说明完整截图中的视觉区域，不能作为 `tap` 或 `swipe` 的坐标系。

没有浏览器消费实时流时，`start_game_session` 会从原生截图取得正向屏幕尺寸。CoreDevice 截图可能不包含桌面视频流的少量编码边界填充，因此严格 profile 匹配在保持方向一致的前提下允许每个维度最多 2% 的差异；仓库筛选仍使用 profile 中记录的桌面实时流精确分辨率。

回放支持 `touch`、`dpad`、`Press`、`SingleTap`、`RepeatTap`、`MultipleTap`、`Swipe`、使用键盘 Button 绑定的 `DirectionPad`、`PadCastSpell`、`MouseCastSpell`、`CancelCast`、`Observation`、`Fps`、`Fire` 和 `hardwareBindings`。`Press` 触点会在整个 `run_keymap.hold_ms` 时段内保持按下，或在游戏会话的按键持续处于 held 状态时保持按下。`DirectionPad`、施法映射及指针驱动映射优先使用 `targetResolution`，没有时使用所选设备当前的正向视频流尺寸。

游戏会话中的 `pointer_deltas` 会移动指定且当前生效的 `MouseCastSpell`、`Observation`、`Fps` 或不保留 FPS 控制的 `Fire` 映射。指针增量也可以携带归一化的 `cursor_x` 和 `cursor_y`；MouseCastSpell 会结合 `center`、`cast_radius`、`drag_radius` 及轴向缩放系数使用这个绝对光标位置，省略两个字段则保持相对增量兼容。`Observation` 会限制在 `max_radius` 内。`Fps` 使用按下边沿切换：在一次 `set_game_input` 更新中加入绑定键，并在下一次更新中移除；之后即使不按键也会保持生效并接收增量，直到再次产生按下边沿。其单/双触点回中策略会在本地 60Hz 会话中执行。`Fire` 根据 `preserve_fps_control` 选择以固定触点与 FPS 并存，或暂时接管指针控制。`CancelCast` 会把当前施法动画移动到取消点后释放。随机锚点、滑动曲线、方向盘距离缩放和有界漂移都由共享原生运行时计算，浏览器只发送按键、轴和指针状态。

`Script` 映射和脚本钩子与桌面控制共用同一套有界运行时，但 MCP 默认禁用；只有向 `run_keymap` 或 `start_game_session` 显式提供 `allow_scripts=true` 才会执行，启用前应先检查配置。脚本可输出触点、键盘、硬件按钮、Unicode 文本以及共享 FPS/施法动作。MCP 会拒绝 `enter_raw_input()`，因为 Agent 协议会话没有切换成桌面键盘透传的明确语义；独立 `RawInput` 映射也仍会明确报错。

## 设备与会话流程

使用 `list_devices` 查看当前传输清单，并把其中包含传输信息的准确 `id` 传给 `connect_device` 或 `reconnect_device`；不再接受裸 UDID。`status` 检查这条 MCP 连接选中的会话，`list_operations` 返回该会话的有界长操作生命周期和类型化错误。连接会选择或复用准确会话，不会停止其他物理设备的会话；重连只拆除并重新建立该目标。两者都会在有界时间内等待新视频帧，也可能报告连接仍在建立；此时应继续调用 `status` 或 `screenshot`，不要连续反复重连。

`device_details` 会刷新规范化的产品、系统、硬件、存储、激活、开发者模式、区域设置和有界电池信息。默认有意省略稳定标识符；只有确实需要设备身份时才设置 `include_identifiers=true` 请求 UDID、序列号和 ECID。

`list_companion_devices` 是有界、只读的 Apple Watch 元数据查询。空列表是有效结果；该工具不提供 Watch 控制、服务启动或端口转发。

## App 与进程

先调用 `list_apps` 查找准确 Bundle ID，再使用 App 工具。它默认返回用户 App，支持按名称或 Bundle ID 进行不区分大小写的查询，默认最多返回 100 项，硬上限为 200。CoreDevice AppService 支持时，`include_system=true` 可加入 Apple 默认 App，`include_app_clips=true` 可加入轻 App；隐藏和内部 App 始终不会返回。

使用 `launch_app` 和 `stop_app` 修改 App 生命周期。两者默认等待画面稳定；如果下一步将使用 `wait_for_app` 或显式帧同步，可以关闭该等待。`app_status` 检查安装和运行状态。`wait_for_app` 等待 `running` 或 `stopped`，默认五秒，最长十秒，`timeout_ms=0` 时只检查一次。

`list_processes` 返回有界的 DVT 进程清单，包括 PID、净化后的进程/App 名称和 Apple 的应用分类。使用 `process_status` 或 `wait_for_process` 前应获取最新清单，因为操作系统可能复用 PID。进程等待与 App 等待一样，默认五秒、最长十秒，超时为零时只检查一次。MCP 不能终止任意 PID，也不能检查进程内存。

单 App stdout/stderr 捕获仍只面向桌面端，因为控制台输出可能包含凭据和个人数据。App 安装、sideloading、签名和升级是项目的长期非目标，不得加入 MCP 或 DeviceHub Mask 的其他界面。MCP 也不开放 App 卸载。

## 事件驱动等待

`wait_for_device_event` 无需客户端轮询即可等待 App、存储、区域设置、设备名称、激活、开发者磁盘镜像挂载状态和 SpringBoard 锁屏状态的规范化变化。默认等待十秒，最长可设置为 30 秒。

收到事件后，下一次调用应把其 `sequence` 作为 `after_sequence`。游标能够消除读取与订阅之间的竞态，并允许服务端立即返回已经保留的更新事件。不提供游标时，只有调用开始后发生的事件符合条件。

事件表示发生了变化，但不一定包含变化后的值。收到 `regional_settings_changed` 或 `developer_image_mounted` 后应调用 `device_details`；收到 `lock_state_changed` 后应重新截图，因为 Notification Proxy 不提供最终锁定值。Apple 原始通知名称和载荷不会跨越 MCP 边界。

## 诊断流程

`performance_snapshot` 会临时请求现有 DVT 采样器，默认最多等待 2.5 秒获取新样本；`wait_ms=0` 会立即返回缓存快照。服务可用时，结果可能包含 CPU 容量与占用、高负载进程、内存、相对能耗、Core Animation、GPU 内存和网络指标。

`recent_device_logs` 会临时请求现有设备日志服务，每次最多返回 500 条匹配结果。`after` 是增量序列游标；`level` 支持 `notice`、`info`、`debug`、`error` 或 `fault`；`query` 会在正文和元数据中进行不区分大小写的匹配。MCP 临时需求不会关闭桌面工作台已经启用的采样或日志流。

App 崩溃后：

1. 调用 `list_crash_reports`，可选按报告名或路径查询。默认上限为 50，最大为 200。
2. 选择结果中返回的准确 `device_path`。
3. 调用 `read_crash_report`。默认返回 256KiB，最大不超过 1MiB，并提供 `truncated` 和 `lossy_utf8` 标记。

崩溃工具保持只读，报告读取会拒绝相对路径、路径穿越、目录和超限请求。截图、进程名称、日志、崩溃片段和 WDA 树都可能包含敏感数据。

## 虚拟定位与设备条件

`set_location` 通过活动 DVT 或旧版定位服务设置固定经纬度。测试不再需要模拟时必须调用 `clear_location`。

设备条件会影响整台手机，而不是单个 App。先调用 `list_device_conditions`，只选择它返回的组/配置组合，再调用 `apply_device_condition`。网络或热状态配置可能同时中断前台游戏和 MCP 连接。测试清理流程必须包含 `clear_device_condition`，失败分支也不能省略。如果传输故障后仍显示等待清理，应保持设备连接，让受监督 DVT channel 恢复后还原正常条件。

## WebDriverAgent 流程

WDA 是需要单独准备的可选能力，DeviceHub Mask 不负责安装或签名。使用 WDA 工具前：

1. 在设备上启用开发者模式。
2. 挂载兼容的开发者磁盘镜像。桌面“设备信息”页会报告就绪状态，并提供显式挂载流程。
3. 为设备安装并签名兼容的 WebDriverAgent `.xctrunner`。
4. 从外部启动它，或使用 `list_apps` 查找准确 Bundle ID，再调用 `wda_start`。
5. 在语义自动化前调用 `wda_status`。

`wda_start` 使用 XCTest，最长等待 30 秒。`wda_runner_status` 只报告由 DeviceHub Mask 启动的 Runner，`wda_stop` 也只停止该 Runner，绝不会终止外部管理的 WDA。DeviceHub Mask 同样不会下载或猜测开发者磁盘镜像。

语义交互应优先使用辅助功能 ID 或名称：

1. 坐标空间或锁屏状态有影响时，先调用 `wda_device_state`。
2. 使用 `wda_find_elements` 或有界的 `wda_ui_tree` 查找控件。
3. 当显示、可用或选中状态有影响时，使用 `wda_inspect_element`。
4. 使用 `wda_wait_for_element`，避免客户端自行轮询。
5. 使用 `wda_click`、`wda_double_tap`、`wda_touch_and_hold`、`wda_type_text` 或 `wda_scroll` 操作。

支持的选择器策略为 `accessibility id`、`name`、`class name`、`xpath`、`-ios predicate string` 和 `-ios class chain`。查找最多返回 20 个从零开始编号的结果。等待状态包括 `present`、`absent`、`displayed`、`hidden`、`enabled`、`disabled`、`selected` 和 `unselected`；默认等待五秒，最长十秒，超时为零时只检查一次。元素缺失满足 `absent` 和 `hidden`，但不满足 `disabled` 或 `unselected`。

`wda_type_text` 接受最多 1,024 个 Unicode 字符和 4,096 UTF-8 bytes。`wda_touch_and_hold` 接受 100 至 10,000ms。`wda_scroll` 只接受 `up`、`down`、`left` 或 `right`。`wda_background_app` 在未提供延时参数时让前台 App 保持后台状态，或者请求 WDA 在 100 至 5,000ms 后恢复它。

`wda_unlock` 不接受密码、不能绕过认证，并且只有 WDA 确认最终已经解锁时才成功。`wda_ui_tree` 可能暴露密码、消息和其他可见文本。WDA 逻辑矩形不是截图像素，不能直接传给 HID 坐标工具。

## 工具参考

| 分类 | 工具 | 注意事项 |
| --- | --- | --- |
| 画面与输入 | `screenshot`、`observe_game`、`tap`、`swipe`、`multi_touch`、`wait_for_frame`、`type_text`、`paste_text`、`press_key`、`press_button`、`app_switcher`、`lock_device`、`rotate` | 截图尺寸定义 HID 坐标；`observe_game` 无网格并支持感兴趣区域 |
| Key Mapping | `list_keymap_profiles`、`get_keymap_profile`、`save_keymap_profile`、`run_keymap`、`start_game_session`、`set_game_input`、`stop_game_session` | 本地 native v2 配置；持续会话使用完整的浏览器键盘代码状态，且不会切换桌面端激活配置 |
| 设备与会话 | `status`、`device_details`、`list_devices`、`list_operations`、`connect_device`、`reconnect_device`、`wait_for_device_event`、`list_companion_devices`、`home_screen_layout` | 准确选择 ID 区分 USB/Wi-Fi；稳定标识符需要显式请求 |
| App 与进程 | `list_apps`、`launch_app`、`stop_app`、`app_status`、`wait_for_app`、`list_processes`、`process_status`、`wait_for_process` | 使用准确 Bundle ID 和最新 PID |
| 诊断 | `list_crash_reports`、`read_crash_report`、`performance_snapshot`、`recent_device_logs` | 有界、只读的诊断输出 |
| 定位与条件 | `set_location`、`clear_location`、`list_device_conditions`、`apply_device_condition`、`clear_device_condition` | 每次测试后清除模拟状态 |
| WDA | `wda_runner_status`、`wda_start`、`wda_stop`、`wda_status`、`wda_device_state`、`wda_unlock`、`wda_ui_tree`、`wda_find_elements`、`wda_inspect_element`、`wda_wait_for_element`、`wda_click`、`wda_type_text`、`wda_double_tap`、`wda_touch_and_hold`、`wda_scroll`、`wda_background_app` | 需要单独准备 WDA 和开发者前置条件 |

## 有意保留的边界

MCP 不开放设备重启、关机、恢复或抹除，App 卸载，AMFI 签名者信任，AFC 或 App 容器修改，备份或备份密码管理，sysdiagnose 采集，统一日志归档导出，描述文件修改，抓包，开发者磁盘镜像挂载/卸载，Apple Watch 控制，以及 WDA 自动安装与签名。App 安装、sideloading、签名和升级依据长期产品策略在整个 DeviceHub Mask 中均不可用，而不只是未向 MCP 开放。

重启和关机只能通过桌面“设备信息”中要求确认的操作执行。文件修改、签名信任、镜像管理、抓包和破坏性操作保持交互式，避免 Agent 静默扩大操作权限。

## 故障排查

- **客户端无法连接：**确认桌面应用正在运行，检查日志中的 MCP 监听地址，并使用准确的 `/mcp` 路径。端口绑定失败不会终止桌面设备会话。
- **没有活动设备：**调用 `list_devices`，使用返回的准确选择 ID 连接，再调用 `status`。保持设备解锁并已信任电脑；需要相关服务时还应启用开发者模式。
- **点击位置偏移：**重新截图并传入其准确 `image_width` 和 `image_height`。不能把设备原始分辨率、SpringBoard 顺序位置或 WDA 逻辑坐标当作截图像素。
- **静止画面等待超时：**它只表示没有新帧，不等于自动断线。先检查 `status`，重新截图后再判断是否需要重连。
- **App 或进程等待超时：**核对准确 Bundle ID，或获取最新 PID。目标状态未在期限内出现并不代表传输失败。
- **WDA 不可用：**检查开发者模式、匹配且已挂载的开发者磁盘镜像、已安装并签名的 `.xctrunner`，以及 `wda_status`。DeviceHub Mask 无法代替用户修复签名或安装 WDA。
- **设备条件中断连接：**必要时重新连接传输，保持设备接入，并调用 `clear_device_condition`，直到确认已经恢复正常条件。

服务配置和日志请参阅[开发与构建](development.md)。设备传输、CoreDevice 和视频故障请参阅[故障排查](troubleshooting.md)。
