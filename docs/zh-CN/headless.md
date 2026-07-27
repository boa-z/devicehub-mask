# 无头服务

简体中文 | [English](https://github.com/boa-z/devicehub-mask/blob/main/docs/en/headless.md) | [文档首页](https://github.com/boa-z/devicehub-mask/blob/main/docs/zh-CN/README.md)

`devicehub-headless` 是实验性的独立原生宿主。它不链接 Tauri 或 Wry，但与桌面应用复用同一套设备运行时、鉴权 HTTP/WebSocket API、WebCodecs 视频链路和 React 界面。服务默认只监听本机回环地址，适合在没有桌面窗口的电脑上运行，并通过浏览器操作设备。

## 运行前准备

- iPhone 或 iPad 已在宿主电脑上完成配对并信任，测试时保持解锁。
- 已启用开发者模式并挂载与系统版本匹配的 Developer Disk Image。设备准备方法见[快速开始](https://github.com/boa-z/devicehub-mask/blob/main/docs/zh-CN/getting-started.md)。
- Windows 已安装并运行 Apple Mobile Device Service；Linux 已安装项目要求的 usbmuxd/libusb 运行环境。
- 浏览器支持 HEVC WebCodecs。Windows 浏览器是否可用取决于浏览器、GPU 驱动和系统提供的 HEVC 解码能力。
- 设备音频需要包内或通过 `--ffmpeg` 指定的 FFmpeg；FFmpeg 不参与视频解码。

## 使用 Nightly 包

从 [nightly release](https://github.com/boa-z/devicehub-mask/releases/tag/nightly) 下载对应平台的无头归档及相邻的 `.sha256` 文件：

```text
devicehub-mask-headless_<version>+<build>_macos-universal.tar.gz
devicehub-mask-headless_<version>+<build>_windows-x64.zip
devicehub-mask-headless_<version>+<build>_linux-x64.tar.gz
```

校验归档后完整解压，不要单独移动可执行文件。归档中的 `devicehub-headless`、`dist/`、FFmpeg、netmuxd、许可证和说明文档必须保持在同一目录。macOS/Linux 可运行 `shasum -a 256 <archive>` 或 `sha256sum <archive>`，Windows 可运行 `Get-FileHash <archive> -Algorithm SHA256`。

在解压后的顶层目录启动服务：

```sh
./devicehub-headless
```

Windows PowerShell：

```powershell
.\devicehub-headless.exe
```

打开终端输出的 `Open http://127.0.0.1:8080/#access_token=...` 地址。临时令牌位于不会随普通 HTTP 请求发送的 URL fragment 中；前端完成引导后会将其从地址栏移除。不要把含令牌的启动 URL 发送给不受信任的人。

按 `Ctrl+C` 会停止 HTTP/MCP 监听、设备会话和 sidecar。不要直接删除仍在使用的数据目录。

## 从源码开发

需要 Node.js、npm、Rust stable 和当前平台的原生构建依赖。在仓库根目录执行：

```sh
npm ci
npm run headless:dev -- --listen 127.0.0.1:8080
```

`headless:dev` 会先构建共享 React 前端，再运行 Cargo debug 二进制。它不会启动、安装或覆盖 Tauri 桌面应用。默认使用仓库根目录下的 `dist/` 和 `./.devicehub-mask/`。

仅构建并手动运行 release 二进制时，需要先准备前端和 sidecar，并显式提供它们的位置：

```sh
npm ci
npm run sidecars:prepare
npm run build
cargo build -p devicehub-headless --release --locked
./src-tauri/target/release/devicehub-headless \
  --frontend-dir ./dist \
  --ffmpeg ./src-tauri/resources/ffmpeg \
  --netmuxd ./src-tauri/resources/netmuxd \
  --data-dir ./.devicehub-mask
```

Windows 将三个可执行文件名改为 `.exe`。直接运行 Cargo 产物时，自动查找以可执行文件所在目录为基准，因此从仓库运行时推荐使用上述显式路径。

## 构建可分发归档

项目脚本会构建 release 二进制和前端、准备经过校验的 sidecar、复制许可证，并生成归档及 `.sha256`：

```sh
npm ci
npm run headless:package -- --version 0.1.0 --build-number 1
```

产物位于 `release-artifacts/`。`--version` 应与项目版本一致，`--build-number` 应为本次构建编号。该脚本只支持当前发布矩阵中的 macOS arm64/x64、Windows x64 和 Linux x64；CI 使用 `universal-apple-darwin` 合并 Universal macOS 二进制。完整发布规则见[CI、发布与更新](https://github.com/boa-z/devicehub-mask/blob/main/docs/zh-CN/distribution.md)。

构建或提交前至少运行：

```sh
npm run verify
```

较大修改或发布前运行 `npm run verify:full`。完整门禁会编译桌面 debug 目标，但不会启动或安装它。

## 常用启动方式

指定持久数据目录和初始设备：

```sh
./devicehub-headless \
  --data-dir /var/lib/devicehub-mask \
  --device <DEVICE_IDENTIFIER>
```

使用固定令牌便于浏览器重新连接：

```sh
openssl rand -hex 32 > devicehub.token
chmod 600 devicehub.token
./devicehub-headless --token-file ./devicehub.token
```

Windows PowerShell：

```powershell
[guid]::NewGuid().ToString("N") | Set-Content -NoNewline devicehub.token
.\devicehub-headless.exe --token-file .\devicehub.token
```

令牌必须是至少 24 个字符的单行 URL 安全文本，只能包含字母、数字、`-` 和 `_`。Unix 上如果文件可被组或其他用户读取，服务会拒绝启动。

显式覆盖本机工具或只使用系统 usbmuxd：

```sh
./devicehub-headless --ffmpeg /opt/devicehub/ffmpeg --netmuxd off
./devicehub-headless --usbmuxd 127.0.0.1:27015
```

启用本机 MCP 服务：

```sh
./devicehub-headless --mcp-listen 127.0.0.1:8009
```

MCP Streamable HTTP 端点为 `http://127.0.0.1:8009/mcp`。MCP 当前没有鉴权，因此强制只能监听回环地址。

## 局域网访问

非回环监听必须显式启用：

```sh
./devicehub-headless \
  --listen 0.0.0.0:8080 \
  --allow-lan \
  --token-file ./devicehub.token
```

在其他电脑打开时，将终端输出 URL 中的 `127.0.0.1` 替换为服务端局域网地址。开放系统防火墙时只允许可信局域网来源，不要把端口直接映射到互联网。

内置服务只提供令牌鉴权，不提供 TLS、用户账户、速率限制或公网部署保护。WebCodecs 等浏览器 API 通常还要求安全上下文；`http://localhost` 会被浏览器特殊信任，但 `http://<LAN-IP>` 不一定可用。完整局域网视频访问应在可信反向代理上终止 HTTPS，并同时转发静态页面、`/api/*` 和 WebSocket `/api/ws`。启用 TLS 不会替代访问令牌。

## 参数参考

| 参数 | 默认值 | 说明 |
| --- | --- | --- |
| `--listen <IP:PORT>` | `127.0.0.1:8080` | 浏览器 HTTP/WebSocket 监听地址 |
| `--allow-lan` | 关闭 | 允许非回环 `--listen`，不自动提供 TLS |
| `--data-dir <PATH>` | `./.devicehub-mask` | 设置、配对记录、映射和临时传输数据 |
| `--frontend-dir <PATH>` | `./dist` | 包含 `index.html` 的 Vite 构建目录 |
| `--token-file <PATH>` | 临时随机令牌 | 读取持久 API 令牌 |
| `--device <IDENTIFIER>` | 自动选择 | 启动后优先连接的设备标识符 |
| `--ffmpeg <PATH>` | 自动查找 | AAC-ELD 音频解码器路径 |
| `--netmuxd <PATH\|off>` | 自动查找 | netmuxd 路径，或关闭 netmuxd sidecar |
| `--usbmuxd <ADDRESS>` | 平台默认 | 系统 usbmuxd 地址覆盖 |
| `--mcp-listen <IP:PORT>` | 关闭 | 可选、仅回环的 MCP 监听地址 |

运行 `./devicehub-headless --help` 可查看二进制当前支持的参数。相对路径基于启动时的当前工作目录，而不是配置文件位置。

## 数据与日志

数据目录默认位于启动目录的 `.devicehub-mask/`：

```text
.devicehub-mask/
├── settings.json
├── pairings/
├── profiles/
└── transfers/
```

`transfers/` 是浏览器文件传输的隔离暂存区，正常操作结束后会立即清理，服务启动时也会清理异常退出留下的内容。配对记录和映射配置需要持久化；部署服务时应为数据目录设置仅服务账户可读写的权限并纳入适当备份。

日志默认输出到标准错误。使用标准 tracing 过滤器调整详细程度：

```sh
RUST_LOG=devicehub_mask=debug,devicehub_runtime=debug ./devicehub-headless
```

生产环境可由 systemd、launchd、Windows 服务包装器或容器运行时负责日志收集和进程重启，但应保留 `Ctrl+C`/终止信号对应的优雅关闭时间。

## 浏览器能力与限制

浏览器全屏、设备控制、WebCodecs 视频、设备音频、AFC/App 存储单文件上传下载和崩溃报告下载可用。音频受浏览器自动播放策略影响，首次播放可能需要点击页面。AFC 与 App 存储上传上限为 64 MiB，下载上限为 256 MiB；目录传输仍仅支持桌面端。

窗口置顶、桌面安装器更新、原生文件对话框、打开服务端目录和宿主剪贴板同步等桌面专属能力会被明确禁用。抓包、sysdiagnose、日志归档和 Developer Image 等仍依赖宿主文件路径的流程尚未全部完成浏览器传输适配。

DeviceHub Mask 不安装、侧载、签名或升级 iOS 应用。桌面端与无头端都不会加入这些能力。

## 故障排查

- `frontend build is missing`：从包含 `dist/` 的解压目录启动，或传入正确的 `--frontend-dir`；源码运行前先执行 `npm run build`。
- `address already in use`：更换 `--listen` 端口，或停止占用该端口的进程。
- 非回环监听被拒绝：同时传入 `--allow-lan`；这只是显式风险确认，不是安全配置。
- 浏览器返回 `401`：重新打开当前进程输出的完整启动 URL；固定令牌部署应确认所有客户端使用同一个受保护令牌文件。
- 页面可打开但 WebCodecs 不可用：确认浏览器处于安全上下文，并检查 Windows HEVC、GPU 驱动和硬件加速支持。
- 没有设备：确认设备已解锁并信任、Developer Mode/DDI 就绪以及 Apple Mobile Device Service 或 usbmuxd 正常；再在页面刷新设备列表。
- 没有声音：在设备设置中启用音频并点击一次页面解除自动播放限制；随后检查 FFmpeg 路径和服务日志。
- Wi-Fi 设备不可见：先通过 USB 完成配对，确认配对目录可写，并保持设备与服务端处于同一可信网络。
