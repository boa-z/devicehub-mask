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

## 验证

提交前运行源码门禁：

```sh
npm run docs:check
npm run lint
npm test
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --all --check
cargo test --manifest-path src-tauri/Cargo.toml --locked
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --locked -- -D warnings
actionlint
bash -n scripts/package-dmg.sh scripts/generate-update-manifest.sh
```

最后构建并运行独立 debug 应用做本地集成检查：

```sh
npm run tauri:build:debug
./src-tauri/target/debug/devicehub-mask
```

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

需要额外参数时可直接传给 Tauri：

```sh
npm run tauri -- build --bundles app
```

典型 macOS 产物包括 `src-tauri/target/release` 下的可执行文件、`.app` 和 DMG。实际
名称会随架构和 Tauri 版本变化。

### Windows

```powershell
npm run tauri:build
```

NSIS 和 MSI 位于 `src-tauri/target/release/bundle/nsis` 与 `bundle/msi`。FFmpeg 和
Apple Mobile Device Service 仍是运行时依赖，不会打进安装包。

### Linux

安装[快速开始](getting-started.md)列出的依赖后运行：

```sh
npm run tauri -- build --bundles appimage,deb
```

产物位于 `bundle/appimage` 和 `bundle/deb`。

### Universal macOS

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
npm run tauri -- build --target universal-apple-darwin --bundles app
```

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
