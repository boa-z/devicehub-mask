# 开发与构建

简体中文 | [English](../en/development.md) | [文档首页](README.md)

修改所有权或运行时行为前，应先阅读[架构说明](architecture.md)和 [Core 与 Runtime 边界](core-runtime.md)。本页作为命令与构建参考。

## 仓库结构

```text
devicehub-mask/
├── .github/workflows/       # 验证和 nightly 发布
├── docs/en/                 # 英文文档
├── docs/zh-CN/              # 简体中文文档
├── crates/
│   ├── devicehub-core/      # 宿主无关领域策略与状态
│   ├── devicehub-headless/  # 独立浏览器宿主二进制
│   ├── devicehub-host/      # 共享文件系统与进程适配器
│   ├── devicehub-keymap/    # 共享确定性映射与脚本运行时
│   ├── devicehub-runtime/   # Apple 设备会话与监督
│   └── devicehub-server/    # 可复用 HTTP/WebSocket 协议适配器
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

## 无头服务开发

先构建共享的 React 界面，再从仓库根目录启动独立原生宿主：

```sh
npm run headless:dev -- --listen 127.0.0.1:8080
```

打开进程输出的 URL。API 令牌放在不会随 HTTP 请求发送的 URL fragment 中，完成引导后会从地址栏移除。需要固定令牌时使用 `--token-file`；该文件必须预先存在，并包含一个至少 24 字符的 URL 安全单行令牌。在 Unix 上还需将文件权限设为 `0600`。

监听地址默认限制在回环接口。非回环 `--listen` 地址必须同时显式传入 `--allow-lan`，否则启动会被拒绝。此开关不提供 TLS、用户账户或可安全暴露到公网的部署能力。MCP 仅在传入 `--mcp-listen` 后启动，并且由于没有鉴权而强制限制在回环接口。运行 `npm run headless:dev -- --help` 可查看全部宿主路径和传输覆盖参数。

## 开发模式

```sh
npm ci
npm run tauri:dev
```

开发产物位于 `target/tauri-dev`，并从 `http://127.0.0.1:5173` 加载 Vite。 Vite 退出后不要单独运行这个可执行文件。独立 debug 和生产构建会通过 Tauri protocol 嵌入前端资源。

## 环境变量

| 变量 | 默认值 | 用途 |
| --- | --- | --- |
| `DEVICEHUB_ADDR` | `127.0.0.1:0` | 私有后端地址；端口 `0` 表示随机端口 |
| `DEVICEHUB_MCP_ADDR` | `127.0.0.1:8009` | Streamable HTTP MCP 监听地址；端点路径为 `/mcp` |
| `DEVICEHUB_PROFILE_DIR` | Tauri 应用数据目录 | 映射配置存储位置 |
| `DEVICEHUB_FFMPEG` | 自动查找 | 设备音频解码使用的 FFmpeg 可执行文件绝对路径 |
| `DEVICEHUB_VIDEO_IN_FLIGHT_FRAMES` | `8` | 有界 WebView 入口流水线的诊断 A/B 覆盖，接受 `1` 至 `8` |
| `DEVICEHUB_LOG` | DeviceHub info 日志 | 首选 Rust tracing 过滤器；优先于 `RUST_LOG` |
| `RUST_LOG` | DeviceHub info 日志 | 标准 tracing 过滤器回退 |
| `DEVICEHUB_HID_DUMP` | 未设置 | 导出 Universal HID 服务 plist 供协议分析 |

`DEVICEHUB_ADDR` 应保持为回环地址。修改地址不会取消令牌鉴权，但外部监听不属于支持的 桌面应用模型。

MCP 端点没有鉴权，除非主机位于可信隔离网络，否则必须保持监听回环地址。监听非回环地址时应用会输出警告。MCP 端口绑定失败不会终止桌面后端或设备会话。客户端配置、工具工作流和安全边界请查看 [MCP 自动化指南](mcp.md)。

运行日志以 JSON Lines 写入各平台的应用日志目录，按日轮转并保留 7 个文件。在“设置 > 诊断”中可以查看当前过滤器、运行 ID、丢弃行数，临时开启 Debug，并打开日志目录。Debug 开关只对本次运行生效。如需缩小 trace 范围，请显式设置过滤器，例如：

```sh
DEVICEHUB_LOG=devicehub_mask=info,devicehub_mask::session=trace npm run tauri:dev
```

环境过滤器优先于设置页开关。无效过滤器会被拒绝，应用自动使用默认过滤器继续启动。

实时视频固定将完整 Annex-B HEVC Access Unit 发送到 WebView，并使用 WebCodecs 解码。应用已移除 FFmpeg 视频解码、RGB/YUV 原始帧传输、JPEG 编码、解码器选择和像素格式设置。FFmpeg 仍用于 AAC-ELD 设备音频解码。

## 验证

提交前运行源码门禁：

```sh
npm run verify
```

生产前端构建还会依据 Vite manifest 检查已提交的性能预算，包括初始 JavaScript、初始 CSS、JavaScript 总量和最大异步 chunk。可运行 `npm run budget:check` 检查现有 `dist/` 产物。不要通过调高预算掩盖回退；应先缩减或拆分依赖图，并记录任何有意的基线变更。

稳定的运行时 HID identity 分配器引入后，JavaScript 总量基线为 1,452,000 字节。初始 JavaScript 和单 chunk 限制保持不变，因此控制热路径的增长不能掩盖启动或懒加载回退。

这与 GitHub Actions 使用同一套跨平台源码门禁，包括文档、前端 lint/测试/构建、Rust 格式/测试，以及将警告视为错误的 Clippy。较大改动在推送前运行完整本地门禁：

```sh
npm run verify:full
```

完整门禁还会构建独立 Debug 应用，但不会启动、打包或安装它。两条命令都不会运行真机测试；真机验证仍必须显式执行 `npm run verify:device -- --udid <UDID>`。

本地验证默认禁用 Cargo 增量编译，并将构建任务数设为 1，避免反复运行测试、Clippy 以及不同 feature/profile 组合时累积巨大的增量缓存。如需临时覆盖，可以显式设置 `CARGO_INCREMENTAL` 和 `CARGO_BUILD_JOBS`。编译前，`verify` 要求至少有 8 GiB 可用空间，`verify:full` 要求至少有 12 GiB，使空间不足时在开始阶段给出可操作的错误，而不是写入产物途中失败。Rust 生成产物长期累积后，可清理所有工作区 Cargo target：

```sh
npm run clean:rust
```

此命令通过 Cargo 官方清理操作同时处理工作区 target 和独立的开发/历史 target，只删除可重新构建的 Cargo 产物，不会删除源文件或应用数据。macOS 与 Linux 可另外使用 `bash -n scripts/package-dmg.sh scripts/generate-update-manifest.sh` 检查发布脚本语法。

多点触控生产路径已在 iPhone 13 Pro Max 上使用双触点 report 验证。跨平台 CI 可以验证 编译，但不能替代真机测试。

修改 runtime 或传输层后，按[真机回归验证](device-regression.md)运行显式 UDID 的只读检查，并完成 USB/Wi-Fi 人工清单。

## 本地化

翻译资源位于 `src/locales/en-US.json` 和 `src/locales/zh-CN.json`。Crowdin 将 `en-US.json` 作为源文件，并通过 `.github/workflows/crowdin.yml` 下载目标语言文件；不要将 Crowdin 凭据提交到仓库。新增界面文案时先添加到源文件，并在组件中使用 `useTranslation()`。`npm run locales:check` 和 `src/i18n.test.ts` 会检查资源树和插值 token 是否一致。

### Crowdin 配置

本仓库使用 Crowdin JSON 文件型项目。`crowdin.yml` 只管理一个源文件 `src/locales/en-US.json`，并将目标语言映射到 `src/locales/%locale%.json`。locale 文件必须保持为普通 JSON 对象，不要加入 export、可执行代码或注释。

首次 bootstrap 时，项目源语言选择 English，并只添加应用准备发布的目标语言。先上传 `en-US.json` 作为源文件，再从每个目标语言页面使用 **Upload Translations** 导入已有 locale 文件。确认导入报告中的 `Imported` 大于 0，并在 Crowdin 编辑器中看到预期 key 后，再下载翻译文件。

将 `CROWDIN_PROJECT_ID` 和 `CROWDIN_PERSONAL_TOKEN` 配置为 GitHub Repository-level Actions secrets。token 绝不能出现在 `crowdin.yml`、源代码、commit、issue 或 PR 中。正常同步 workflow 保持 `upload_translations: false`：完成首次导入后，Crowdin 是翻译的唯一来源。workflow 会上传源文件变更、下载翻译，并创建 review PR。如果没有检查 Crowdin 导入报告，不要合并目标文件仍为源语言文本的本地化 PR。

新增源文案时，只修改 `en-US.json`，保留 `{{name}}`、`{{count}}` 等插值 token，并保持协议标识符、Bundle ID、文件路径、产品名和 key code 不变。合并前检查生成的目标文件 diff，并运行 `npm run locales:check`、`npm run lint`、`npm test` 和 `npm run build`。前端 workflow 会在 pull request 中自动运行这些检查。

### 增加语言

Crowdin 下载的文件不会被运行时自动发现。增加目标语言必须同时完成以下修改：

1. 在 Crowdin 中增加目标语言，并确认其 locale code 会映射到 `src/locales/<locale>.json`。
2. 在 `src/i18n.ts` 中加入 locale code 和动态 loader，并在需要时更新 `normalizeLanguage()` 的地区别名处理。
3. 在 `src/components/SettingsPage.tsx` 的语言选择器中加入该语言。
4. 在 `src/AppProviders.tsx` 中加入对应的 Ant Design locale 映射。
5. 保持 `npm run locales:check` 和 key 对齐测试通过，在 Settings 界面实际切换验证，并确认新 locale 被拆分为独立 chunk。

当前应用只注册了 `zh-CN` 和 `en-US`；Crowdin 下载的新文件在完成这些运行时注册前不能直接使用。英文作为 fallback 打入首包，目标语言按需加载。增加语言后必须重新运行 `npm run build` 和前端性能预算检查。

协议标识符、键码、配置名称和用户标签不翻译。默认映射标签只在新建配置时本地化。 系统字体 token `--system-font` 定义在 `src/styles.css`，并由 `src/AppProviders.tsx` 传给 Ant Design；不要引入远程或捆绑字体。

修改文档时，应保持 `docs/en` 和 `docs/zh-CN` 的页面名称与导航对应。 `npm run docs:check` 会验证页面对应关系和本地 Markdown 链接，CI 会在 macOS、Windows 和 Linux 上运行该检查。

## 生产构建

构建当前主机配置的全部安装包：

```sh
npm run tauri:build
```

该命令会先为当前主机下载经过 SHA-256 校验的 netmuxd 和 LGPL FFmpeg sidecar。 Windows 与 Linux 使用 BtbN 滚动 [`latest` Release](https://github.com/BtbN/FFmpeg-Builds/releases/tag/latest) 中的 `n8.1` LGPL 资产。准备脚本优先通过 Releases API 解析真实资产 URL 和 GitHub SHA-256 digest；API 不可用时回退到 `latest/checksums.sha256`。脚本不会下载不带文件名的 `releases/download/latest` 路径，必须指定具体资产文件名。需要复现固定构建时，可设置 `DEVICEHUB_FFMPEG_BTB_TAG` 为不可变的 BtbN Release tag。

桌面 sidecar 生成在 `src-tauri/resources`，且不会纳入 Git。`ffmpeg-target.json` 记录 FFmpeg 版本和 target triple；只有元数据匹配时准备脚本才会复用现有文件。直接准备非当前主机 target 时必须指定独立暂存目录，例如 `node scripts/prepare-ffmpeg.mjs --target aarch64-unknown-linux-gnu --output-dir release-artifacts/sidecars/ffmpeg/aarch64-unknown-linux-gnu`。Headless 打包已自动使用这种隔离目录，不会再覆盖桌面主机资源。安装包优先使用内置 FFmpeg；测试时仍可用 `DEVICEHUB_FFMPEG` 显式覆盖。需要明确重建当前主机资源时使用 `npm run ffmpeg:prepare -- --force`。

需要额外构建参数时，可在 `--` 后传给统一构建脚本：

```sh
npm run tauri:build -- --bundles app
```

典型 macOS 产物包括 `src-tauri/target/release` 下的可执行文件、`.app` 和 DMG。实际 名称会随架构和 Tauri 版本变化。

### Windows

```powershell
npm run tauri:build
```

NSIS 和 MSI 位于 `src-tauri/target/release/bundle/nsis` 与 `bundle/msi`。FFmpeg 已内置， 启动时不会弹出控制台窗口；Apple Mobile Device Service 仍是运行时依赖。

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

统一构建脚本会从 `--target` 推导 sidecar 平台，并从固定校验和的上游源码构建仅启用 LGPL 组件的 universal FFmpeg 可执行文件；Windows 与 Linux 使用当前 `n8.1` 的 LGPL 静态构建并校验 SHA-256。安装包同时包含 `THIRD_PARTY_NOTICES.txt` 和完整 FFmpeg 许可证。

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
