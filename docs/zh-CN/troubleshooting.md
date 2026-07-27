# 故障排查

简体中文 | [English](../en/troubleshooting.md) | [文档首页](README.md)

## macOS 提示无法验证应用是否包含恶意软件

当前 macOS 发布包使用 ad-hoc 签名。免费的 Apple 开发者账号无法申请面向站外分发的 Developer ID Application 证书，也无法为发布包完成 Apple 公证，因此首次打开时可能出现“Apple could not verify ‘DeviceHub Mask’ is free of malware that may harm your Mac or compromise your privacy.”提示。这不表示 macOS 已检测到恶意软件，而是表示 Apple 无法验证发布者身份和公证票据。

只从项目的 GitHub Releases 下载，并先核对随发布包提供的 SHA-256 文件。可以在 Finder 中右键点击应用并选择“打开”，或进入“系统设置 > 隐私与安全”，在拦截记录旁选择“仍要打开”。如果系统仍然阻止启动，可以只移除 DeviceHub Mask 应用包的隔离属性，然后打开应用：

```sh
sudo xattr -rd com.apple.quarantine "/Applications/DeviceHub Mask.app"
open "/Applications/DeviceHub Mask.app"
```

根据实际安装位置替换完整路径。不要对 `/Applications`、`~/Downloads` 或其他目录整体执行该命令，也不要全局关闭 Gatekeeper。下载新版本后，macOS 可能再次附加隔离属性，需要重新确认该版本的来源和校验值。

## Debug 可执行文件打开后白屏

`tauri dev` 编译的 WebView 会从 `127.0.0.1:5173` 加载 Vite。Vite 停止后单独运行 这个开发可执行文件会显示白屏。

需要热重载时运行：

```sh
npm run tauri:dev
```

需要嵌入前端的独立版本时运行：

```sh
npm run tauri:build:debug
./src-tauri/target/debug/devicehub-mask
```

开发和独立构建使用不同 Cargo target 目录。

## 私有后端无法启动

默认随机回环端口可以避免普通端口冲突。停止可能仍占用 CoreDevice 会话的旧 `devicehub-mask`、`devicehub_rs` 和 FFmpeg 进程。`DEVICEHUB_ADDR` 应保持监听回环 地址。API 没有网页根路径，并始终要求启动令牌。

## 收集运行日志

进入“设置 > 诊断”，点击“打开日志目录”。日志采用 JSON Lines 格式，按日轮转并保留最近 7 个文件。只在复现问题时开启详细 Debug，进行性能测试前应关闭。分享同一次运行的日志 片段时请附上设置页中的运行 ID。诊断桥接不会写入令牌、剪贴板内容、视频帧或原始 HID report。

如果 UI 无法打开，可以从终端使用 `DEVICEHUB_LOG=devicehub_mask=debug` 启动。长时间采集 不要使用不受限的全局 `trace` 过滤器。

## 找不到 FFmpeg 或听不到设备声音

- 安装包已内置经过校验的 FFmpeg，并优先于 `PATH` 使用。macOS 开发构建可运行 `brew install ffmpeg`；由于应用不会继承终端 `PATH`，还会直接检查 `/opt/homebrew/bin/ffmpeg`、`/usr/local/bin/ffmpeg` 和 `/opt/local/bin/ffmpeg`。
- 调试 AAC-ELD 音频解码时可把 `DEVICEHUB_FFMPEG` 设置为绝对可执行路径，显式覆盖内置或系统版本。
- Windows：运行 `winget install --id Gyan.FFmpeg --exact`，然后打开新终端。
- 自定义路径：为应用进程设置 `DEVICEHUB_FFMPEG` 为可执行文件绝对路径。
- 解锁并重新连接设备，关闭其他画面会话，在状态标识和 Rust 日志中检查 RSD 或 displayservice 错误。

## 设备没有开放 displayservice

如果 RSD 没有报告 `com.apple.coredevice.displayservice`，说明连接和 RSD 握手已经 成功，但设备没有开放屏幕串流服务。这不代表 USB 不受支持。

Windows 上保持手机连接和解锁，然后运行：

```powershell
.\scripts\prepare-windows-device.ps1
```

脚本检查开发者模式、挂载 Personalized Developer Disk Image、重新执行 USB RSD 握手并验证服务名。准备成功后重新连接。持续失败可能需要在 Xcode Device Hub 中完成 一次有线配对，也可能是当前 iOS beta 不兼容。

使用 `RUST_LOG=devicehub_mask::session=debug` 输出完整 RSD 服务列表。 `192.168.9.147:62078` 这样的地址是 Lockdown 端点，不是 CoreDeviceProxy 返回的 RSD 端点，手动提供它不会让缺失的服务出现。

## Remote Pairing 验证出现 early EOF

`remote pairing verification failed: Socket(... UnexpectedEof ... "early eof")` 表示应用已经连接到设备通过 Bonjour 发布的 `_remotepairing._tcp` 服务，但设备在发送完整 RemotePairing 握手帧前关闭了 TCP 流。它本身不能证明已保存的授权无效。设备锁屏或切换网络、iOS 重启 RemotePairing 服务、Bonjour 地址刚刚更新，或者上一条隧道仍在关闭，都可能产生这种瞬时结果。

DeviceHub Mask 会保留现有凭据，使用全新 socket 重试瞬时断流，然后通过有界退避重建完整 Wi-Fi 隧道。请保持设备唤醒、解锁并与电脑处于同一网络；不要因为一次 EOF 删除信任。如果应用明确报告 Wi-Fi 授权已不再被设备接受，并且错误持续出现，再通过 USB 连接，执行**忘记电脑信任**，重新确认**信任此电脑**，然后选择 Wi-Fi 传输。显式移除信任现在也会删除 DeviceHub Mask 独立保存的 RemotePairing 凭据，使下一次 Wi-Fi 连接能够创建干净的新身份。

## CoreDevice 错误 9021

设备拒绝了远程控制能力。支持情况取决于硬件与 iOS 组合，不代表所有低于 iOS 27 的 设备都不受支持；但对于明确拒绝的设备，需要升级到 iOS 27 或使用受支持的新硬件。

切换 USB/Wi-Fi、修改 FFmpeg、应用签名或重复重试都无法绕过设备端检查。DeviceHub Mask 会显示本地化错误说明，不输出归档 binary plist。目前没有仅画面回退，因为初始 audio media session 同时建立视频和 Universal HID 控制授权。

## 触控位置错误或横屏拉伸

不要强制 Canvas 填充任意宽高。DeviceHub Mask 使用同一个比例 contain-fit 旋转后的 画面，并只在实际显示矩形内归一化触控。报告回归时请提供源分辨率、显示分辨率、方向和 截图。

## Windows CPU 占用较高

实时视频固定使用 WebCodecs。如果 Windows 报告 `OperationError: Unsupported configuration`，应用会从 SPS 读取 HEVC profile 与 level，并重试保守的 `hev1`、`hvc1` 配置。全部失败表示 WebView2 或系统 codec 无法解码设备视频流。GPU 具备 HEVC 能力仍不充分；Windows 通常需要 HEVC Video Extensions。当前没有 Native / FFmpeg 视频回退。

`browser video client lagged` 表示 WebSocket 发送端短暂落后于压缩 HEVC 广播，不代表 CoreDevice 已停止产出画面。应用会丢弃不可继续解码的依赖帧，重复请求 IRAP 直到重同步，并在无需重新连接设备的情况下恢复。如果工具栏的“源/解码”持续非零，而“发送/显示”超过数秒仍为零，请采集包含 lag 警告、后续 PLI/FIR 请求、收到 IRAP 记录和 `devicehub_mask::perf` 输出的 Debug 日志。

观察界面的解码 / 发送 / 显示 FPS 和解码入口延迟：

- 源 FPS 来自完整 RTP 帧 marker；发布 FPS 表示进入 WebCodecs 传输的压缩 Access Unit。
- 发送与显示 FPS 应接近发布 FPS。后端最多保留两个未确认包，前端入口队列上限为八个包。
- Debug 性能日志会报告 RTP 时间戳步长、源到达抖动、HEVC 排队时间、帧年龄、WebSocket 写入、解码入口确认、呈现确认、WebCodecs 输出、Canvas 绘制和各阶段丢帧。
- Windows 使用 WebCodecs 暴露的源分辨率，不再存在 RGB24/YUV420P 传输或 FFmpeg 尺寸限制。

这些指标和 Debug 日志字段在各平台保持一致。比较 macOS、Windows 与 Linux 时，应使用 Release 构建，并保持设备、画面内容、解码尺寸和 `DEVICEHUB_VIDEO_IN_FLIGHT_FRAMES` 相同。记录 CPU、全部 FPS 指标、入口/呈现延迟、设备分辨率、GPU、WebView 版本，以及测试的是安装版还是 Debug 构建。Debug 构建不能代表 Release 性能。

## 按进程过滤的网络抓包没有数据包

过滤使用抓包前选择的 PID 快照。App 重新启动会得到新的 PID；请刷新运行进程清单、选择新条目后重新开始抓包。如果“已排除数据包”非零但写入数据包为零，说明 pcapd 正在产生流量，但没有数据包归属于所选主 PID 或 effective PID。可选择“全部进程”判断抓包服务本身是否正常产出。

## 蓝牙抓包没有数据包

开始 HCI 抓包前，需要在 iPhone 上安装 Apple Bluetooth Logging 配置描述文件。未安装时 `BTPacketLogger` 可能接受连接但保持静默，此时生成只有 24 字节全局文件头的有效 PCAP 属于预期行为。抓包期间保持目标蓝牙手柄或音频设备活跃；如果服务本身无法启动，请在 “服务健康”中检查 `bluetooth.capture`。

## 检查更新失败

- 确认 nightly release 包含 `latest.json`、当前平台更新产物和对应 `.sig`。
- 确认 `src-tauri/tauri.conf.json` 的 `plugins.updater.pubkey` 与 CI 私钥匹配。
- 确认已安装版本低于 manifest 版本。
- Windows 和 Linux 分别使用 NSIS 和 AppImage 更新，macOS 使用 app 压缩包。

密钥和产物说明见[发布与更新](distribution.md)。
