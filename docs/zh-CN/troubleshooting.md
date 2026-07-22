# 故障排查

简体中文 | [English](../en/troubleshooting.md) | [文档首页](README.md)

## Debug 可执行文件打开后白屏

`tauri dev` 编译的 WebView 会从 `127.0.0.1:5173` 加载 Vite。Vite 停止后单独运行
这个开发可执行文件会显示白屏。

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

默认随机回环端口可以避免普通端口冲突。停止可能仍占用 CoreDevice 会话的旧
`devicehub-mask`、`devicehub_rs` 和 FFmpeg 进程。`DEVICEHUB_ADDR` 应保持监听回环
地址。API 没有网页根路径，并始终要求启动令牌。

## 收集运行日志

进入“设置 > 诊断”，点击“打开日志目录”。日志采用 JSON Lines 格式，按日轮转并保留最近
7 个文件。只在复现问题时开启详细 Debug，进行性能测试前应关闭。分享同一次运行的日志
片段时请附上设置页中的运行 ID。诊断桥接不会写入令牌、剪贴板内容、视频帧或原始 HID
report。

如果 UI 无法打开，可以从终端使用 `DEVICEHUB_LOG=devicehub_mask=debug` 启动。长时间采集
不要使用不受限的全局 `trace` 过滤器。

## 找不到 FFmpeg 或没有画面

- macOS：运行 `brew install ffmpeg`。打包应用不会继承终端 `PATH`，因此也会直接检查
  `/opt/homebrew/bin/ffmpeg`、`/usr/local/bin/ffmpeg` 和 `/opt/local/bin/ffmpeg`。
- Windows：运行 `winget install --id Gyan.FFmpeg --exact`，然后打开新终端。
- 自定义路径：为应用进程设置 `DEVICEHUB_FFMPEG` 为可执行文件绝对路径。
- 解锁并重新连接设备，关闭其他画面会话，在状态标识和 Rust 日志中检查 RSD 或
  displayservice 错误。

## 设备没有开放 displayservice

如果 RSD 没有报告 `com.apple.coredevice.displayservice`，说明连接和 RSD 握手已经
成功，但设备没有开放屏幕串流服务。这不代表 USB 不受支持。

Windows 上保持手机连接和解锁，然后运行：

```powershell
.\scripts\prepare-windows-device.ps1
```

脚本检查开发者模式、挂载 Personalized Developer Disk Image、重新执行 USB RSD
握手并验证服务名。准备成功后重新连接。持续失败可能需要在 Xcode Device Hub 中完成
一次有线配对，也可能是当前 iOS beta 不兼容。

使用 `RUST_LOG=devicehub_mask::session=debug` 输出完整 RSD 服务列表。
`192.168.9.147:62078` 这样的地址是 Lockdown 端点，不是 CoreDeviceProxy 返回的 RSD
端点，手动提供它不会让缺失的服务出现。

## CoreDevice 错误 9021

设备拒绝了远程控制能力。支持情况取决于硬件与 iOS 组合，不代表所有低于 iOS 27 的
设备都不受支持；但对于明确拒绝的设备，需要升级到 iOS 27 或使用受支持的新硬件。

切换 USB/Wi-Fi、修改 FFmpeg、应用签名或重复重试都无法绕过设备端检查。DeviceHub
Mask 会显示本地化错误说明，不输出归档 binary plist。目前没有仅画面回退，因为初始
audio media session 同时建立视频和 Universal HID 控制授权。

## 触控位置错误或横屏拉伸

不要强制 Canvas 填充任意宽高。DeviceHub Mask 使用同一个比例 contain-fit 旋转后的
画面，并只在实际显示矩形内归一化触控。报告回归时请提供源分辨率、显示分辨率、方向和
截图。

## Windows CPU 占用较高

观察界面的解码 / 发送 / 显示 FPS 和 JPEG 延迟：

- 由于同时只有一帧 JPEG 在途，发送 FPS 应接近显示 FPS。
- 解码 FPS 仍可能接近 60 FPS 源帧率。
- Windows 默认使用 1920 像素长边和 RGB24 传输。

可以降低解码限制，同时保持画面比例：

```powershell
$env:DEVICEHUB_VIDEO_MAX_DIMENSION = "1280"
npm run tauri:dev
```

仅在诊断原始分辨率时设置为 `0`。记录 CPU、全部 FPS 指标、JPEG 延迟、设备分辨率、
GPU，以及测试的是安装版还是 debug 版。Debug 构建不能代表 release 性能。

## 检查更新失败

- 确认 nightly release 包含 `latest.json`、当前平台更新产物和对应 `.sig`。
- 确认 `src-tauri/tauri.conf.json` 的 `plugins.updater.pubkey` 与 CI 私钥匹配。
- 确认已安装版本低于 manifest 版本。
- Windows 和 Linux 分别使用 NSIS 和 AppImage 更新，macOS 使用 app 压缩包。

密钥和产物说明见[发布与更新](distribution.md)。
