# 开发与构建

简体中文 | [English](../en/development.md) | [文档首页](README.md)

## 仓库结构

```text
devicehub-mask/
├── .github/workflows/       # 验证和 nightly 发布
├── docs/en/                 # 英文文档
├── docs/zh-CN/              # 简体中文文档
├── scripts/                 # 设备准备和打包脚本
├── src/                     # React 应用
├── src-tauri/
│   ├── capabilities/        # Tauri 权限
│   ├── icons/
│   ├── src/                 # Rust 桌面后端
│   ├── Cargo.toml
│   └── tauri.conf.json
├── package.json
└── README.md
```

生成的 `dist/` 和 Cargo `target/` 目录不是源码文档的一部分。

## 开发模式

```sh
npm ci
npm run tauri:dev
```

开发产物位于 `target/tauri-dev`，并从 `http://127.0.0.1:5173` 加载 Vite。
Vite 退出后不要单独运行这个可执行文件。独立 debug 和生产构建会通过 Tauri protocol
嵌入前端资源。

## 环境变量

| 变量 | 默认值 | 用途 |
| --- | --- | --- |
| `DEVICEHUB_ADDR` | `127.0.0.1:0` | 私有后端地址；端口 `0` 表示随机端口 |
| `DEVICEHUB_MCP_ADDR` | `127.0.0.1:8009` | Streamable HTTP MCP 监听地址；端点路径为 `/mcp` |
| `DEVICEHUB_PROFILE_DIR` | Tauri 应用数据目录 | 映射配置存储位置 |
| `DEVICEHUB_FFMPEG` | 自动查找 | FFmpeg 可执行文件的绝对路径 |
| `DEVICEHUB_VIDEO_MAX_DIMENSION` | Windows 为 `1920`，其他平台为原始尺寸 | 最大解码宽度或高度；保持比例且不放大；`0` 表示禁用限制 |
| `DEVICEHUB_VIDEO_PIXEL_FORMAT` | 设置页选项 | 使用 `rgb24` 或实验性 `yuv420p` 覆盖应用的视频像素格式设置 |
| `DEVICEHUB_VIDEO_IN_FLIGHT_FRAMES` | `2` | 有界 WebView 帧流水线的诊断 A/B 覆盖，仅接受 `1` 或 `2` |
| `DEVICEHUB_LOG` | DeviceHub info 日志 | 首选 Rust tracing 过滤器；优先于 `RUST_LOG` |
| `RUST_LOG` | DeviceHub info 日志 | 标准 tracing 过滤器回退 |
| `DEVICEHUB_HID_DUMP` | 未设置 | 导出 Universal HID 服务 plist 供协议分析 |

`DEVICEHUB_ADDR` 应保持为回环地址。修改地址不会取消令牌鉴权，但外部监听不属于支持的
桌面应用模型。

MCP 端点没有鉴权，除非主机位于可信隔离网络，否则必须保持监听回环地址。监听非回环
地址时应用会输出警告。MCP 端口绑定失败不会终止桌面后端或设备会话。

运行日志以 JSON Lines 写入各平台的应用日志目录，按日轮转并保留 7 个文件。在“设置 >
诊断”中可以查看当前过滤器、运行 ID、丢弃行数，临时开启 Debug，并打开日志目录。Debug
开关只对本次运行生效。如需缩小 trace 范围，请显式设置过滤器，例如：

```sh
DEVICEHUB_LOG=devicehub_mask=info,devicehub_mask::session=trace npm run tauri:dev
```

环境过滤器优先于设置页开关。无效过滤器会被拒绝，应用自动使用默认过滤器继续启动。

“设置 > 视频”提供 RGB24 与实验性 YUV420P 路径，RGB24 仍为默认值。选择会持久化到
平台应用配置目录，并在下次连接设备时生效。显式设置
`DEVICEHUB_VIDEO_PIXEL_FORMAT` 后，本次运行中的界面选项将变为只读。

同一区域默认启用实验性的“浏览器 / WebCodecs”解码器。该路径把完整 Annex-B HEVC
Access Unit 直接发送到 WebView，从实时视频链路中移除 FFmpeg、原始帧传输和 JPEG 编码。
WebCodecs 能力检测、输出超时或运行时失败会显示在设置页，并自动重连当前设备；本次运行
后续使用原生解码器。

## 验证

提交前运行源码门禁：

```sh
npm run verify
```

这与 GitHub Actions 使用同一套跨平台源码门禁，包括文档、前端 lint/测试/构建、Rust
格式/测试，以及将警告视为错误的 Clippy。较大改动在推送前运行完整本地门禁：

```sh
npm run verify:full
```

完整门禁还会构建独立 Debug 应用，但不会启动、打包或安装它。macOS 与 Linux 可另外使用
`bash -n scripts/package-dmg.sh scripts/generate-update-manifest.sh` 检查发布脚本语法。

多点触控生产路径已在 iPhone 13 Pro Max 上使用双触点 report 验证。跨平台 CI 可以验证
编译，但不能替代真机测试。

## 本地化

翻译资源位于 `src/locales/en-US.ts` 和 `src/locales/zh-CN.ts`。新增界面文案时必须同时
添加到两个文件，并在组件中使用 `useTranslation()`。`src/i18n.test.ts` 会检查两个
资源树的 key 是否一致。

协议标识符、键码、配置名称和用户标签不翻译。默认映射标签只在新建配置时本地化。
系统字体 token `--system-font` 定义在 `src/styles.css`，并由 `src/AppProviders.tsx`
传给 Ant Design；不要引入远程或捆绑字体。

修改文档时，应保持 `docs/en` 和 `docs/zh-CN` 的页面名称与导航对应。
`npm run docs:check` 会验证页面对应关系和本地 Markdown 链接，CI 会在 macOS、Windows
和 Linux 上运行该检查。

## 生产构建

构建当前主机配置的全部安装包：

```sh
npm run tauri:build
```

该命令会先为当前主机下载经过固定 SHA-256 校验的 netmuxd 和 LGPL FFmpeg sidecar。
生成的可执行文件位于 `src-tauri/resources` 且不会纳入 Git。安装包优先使用内置 FFmpeg；
测试时仍可用 `DEVICEHUB_FFMPEG` 显式覆盖。已有 FFmpeg 只有在目标架构和必需能力均通过
校验后才会复用；需要明确重建时使用 `npm run ffmpeg:prepare -- --force`。

需要额外构建参数时，可在 `--` 后传给统一构建脚本：

```sh
npm run tauri:build -- --bundles app
```

典型 macOS 产物包括 `src-tauri/target/release` 下的可执行文件、`.app` 和 DMG。实际
名称会随架构和 Tauri 版本变化。

### Windows

```powershell
npm run tauri:build
```

NSIS 和 MSI 位于 `src-tauri/target/release/bundle/nsis` 与 `bundle/msi`。FFmpeg 已内置，
启动时不会弹出控制台窗口；Apple Mobile Device Service 仍是运行时依赖。

### Linux

安装[快速开始](getting-started.md)列出的依赖后运行：

```sh
npm run tauri:build -- --bundles appimage,deb
```

产物位于 `bundle/appimage` 和 `bundle/deb`。

### Universal macOS

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
npm run tauri:build -- --target universal-apple-darwin --bundles app
```

统一构建脚本会从 `--target` 推导 sidecar 平台，并从固定校验和的上游源码构建仅启用
LGPL 组件的 universal FFmpeg 可执行文件；Windows 与 Linux 使用固定版本并校验
SHA-256 的 LGPL 静态构建。安装包同时包含 `THIRD_PARTY_NOTICES.txt` 和完整 FFmpeg
许可证。

产物位于 `src-tauri/target/universal-apple-darwin/release/bundle/macos`。

### 可复现 DMG

使用 CI 相同脚本为已有 app 写入版本并生成校验文件：

```sh
APP_VERSION=0.1.0 \
BUILD_NUMBER=1 \
APP_PATH="src-tauri/target/release/bundle/macos/DeviceHub Mask.app" \
  scripts/package-dmg.sh
```

脚本生成 `dist/devicehub-mask_0.1.0+1.dmg` 及 SHA-256 文件。

自动发布流程见[发布与更新](distribution.md)。
