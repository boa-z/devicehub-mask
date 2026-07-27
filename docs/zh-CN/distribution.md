# CI、发布与更新

简体中文 | [English](../en/distribution.md) | [文档首页](README.md)

## 工作流触发方式

`.github/workflows/nightly.yml` 只在 commit 和手动触发时运行。没有定时任务，也不使用 GitHub Environments，因此不会创建妨碍清理历史的 Deployment 记录。

`.github/workflows/cleanup-nightly.yml` 每周运行，也支持手动触发。默认保留最新 20 次已完成的 nightly workflow 运行，并删除超过 14 天的 nightly artifacts。手动运行可以在受限范围内调整两个保留参数，或使用 dry-run 只查看预计删除内容。它不会删除滚动 `nightly` Release、tag 或 Release assets。

`.github/workflows/release.yml` 只允许手动触发。运行时选择包含确切发布源码的 Git ref，输入与 `v<tauri.conf.json version>` 匹配的标签，并选择是否保留为 Draft。它复用 Nightly 的同一套验证和打包流程，但会注入 Stable 通道和纯产品版本。正式标签和已发布 Release 不可覆盖。工作流会先创建或继续 Draft，上传全部资源；仅当关闭 **Draft** 时，才在上传完成后将其发布为仓库最新正式版。

## Jobs

- **verify** 使用相互独立失败的 macOS、Windows 和 Linux 矩阵。每个平台运行前端 lint、测试和构建，Rust 格式、测试和 Clippy，以及 Tauri debug 应用构建。
- **build-macos** 生成 Apple Silicon/Intel Universal DMG 和 Universal 无头 tarball，并验证两个可执行架构和完整应用签名。
- **build-windows** 生成 x64 NSIS、MSI 安装包和 x64 无头 zip。
- **build-linux** 生成 x64 AppImage、DEB 和 x64 无头 tarball。
- **publish-release** 等待全部安装包，将更新片段合并成一个 `latest.json`，然后原子替换滚动 nightly release 的资源。

工作流 artifact 保留 14 天。公开滚动发布地址：

<https://github.com/boa-z/devicehub-mask/releases/tag/nightly>

每次 Nightly 和 Stable 发布都会同时生成两种 Windows 安装包。Nightly NSIS 使用 zlib 压缩以控制 CI 耗时，Stable NSIS 则使用体积更小但速度更慢的 LZMA。Tauri 下载的 NSIS 和 WiX 工具链会在不同运行之间缓存；CMake 和 NASM 仅在 runner 缺失时安装。

每个无头归档都包含原生 `devicehub-headless` 可执行文件、共享构建后的 React `dist/`、FFmpeg、netmuxd、第三方声明、许可证和中英文启动文档。每个归档都有相邻的 SHA-256 文件，并与桌面安装包一起上传到同一个滚动 Release。使用方式见[无头服务](headless.md)。

## 版本与产物

`tauri.conf.json` 保存当前目标正式版本。Nightly 构建使用跨平台 SemVer 预发布版本 `<product-version>-<run-number>`；例如目标版本为 `0.1.0` 的第 96 次构建是 `0.1.0-96`。预发布标识必须为数字，因为 Windows MSI 工具链会拒绝文本标识和大于 65,535 的数值。数字标识用于排序 Nightly，最终的 `0.1.0` 正式版高于所有 `0.1.0-*` 构建。正式版发布后，必须先提升产品版本，再继续生成 Nightly。

安装包文件名包含产品版本和 workflow build number，macOS 也使用运行编号作为 `CFBundleVersion`。设置页分别显示**版本**、**构建编号**和七位 **Commit**；当前更新通道已能识别正式版或 Nightly，不再向用户暴露第二套内部版本。

Release 可以包含：

```text
devicehub-mask_<base-version>+<build>_universal.dmg
devicehub-mask_<base-version>+<build>_universal.dmg.sha256
devicehub-mask_<base-version>-<build>_universal.app.tar.gz
devicehub-mask_<base-version>-<build>_universal.app.tar.gz.sig
devicehub-mask_<base-version>+<build>_x64-setup.exe
devicehub-mask_<base-version>+<build>_x64-setup.exe.sig
devicehub-mask_<base-version>+<build>_x64.msi
devicehub-mask_<base-version>+<build>_amd64.AppImage
devicehub-mask_<base-version>+<build>_amd64.AppImage.sig
devicehub-mask_<base-version>+<build>_amd64.deb
devicehub-mask-headless_<base-version>+<build>_macos-universal.tar.gz
devicehub-mask-headless_<base-version>+<build>_windows-x64.zip
devicehub-mask-headless_<base-version>+<build>_linux-x64.tar.gz
latest.json
```

## Tauri 更新签名

更新签名与 Apple 代码签名相互独立。公钥提交在 `src-tauri/tauri.conf.json`，私钥绝对 不能提交到仓库。

只有在发布首个兼容版本前才应生成替代密钥：

```sh
mkdir -p .tauri
npm run tauri -- signer generate --write-keys .tauri/devicehub-mask.key
```

更新 `plugins.updater.pubkey`，然后配置仓库 Actions secrets：

| Secret | 内容 |
| --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | 私钥文件完整内容 |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | 生成密码，或留空 |
| `HOMEBREW_TAP_TOKEN` | 对 `boa-z/homebrew-devicehub-mask` 具备 Contents 权限的 fine-grained token |

```sh
gh secret set TAURI_SIGNING_PRIVATE_KEY < .tauri/devicehub-mask.key
gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD
```

缺少私钥时，CI 仍可发布原生安装包，但会跳过更新签名和 `latest.json`。私钥丢失或替换 后，已有安装将无法接受新密钥签名的更新。

运行时可在设置页选择**正式版**或 **Nightly** 更新通道，并关闭自动检查。偏好分别保存在 `devicehub-mask.updates.channel` 和 `devicehub-mask.updates.automatic`，手动检查始终可用。正式版使用 `releases/latest/download/latest.json`，Nightly 使用滚动发布的 `releases/download/nightly/latest.json`，两条路径采用相同的 Tauri 签名 manifest 格式。接受更新后会下载、验证、安装并重启应用。在正式版发布签名 `latest.json` 之前，检查正式版通道会明确报告 manifest 不存在，不会静默回退到 Nightly。

## Apple 签名与公证

当前 nightly macOS 应用在 Universal 合并和版本写入后使用结构有效的 ad-hoc 签名。 它能验证 sealed resources 和二进制架构，但不能证明发布者身份，Gatekeeper 仍可能要求 用户手动批准。

免费的 Apple 开发者账号不能申请 Developer ID Application 证书或完成站外分发公证，因此当前发布流程继续保留 ad-hoc 签名。用户端处理方法见[故障排查](troubleshooting.md#macos-提示无法验证应用是否包含恶意软件)。

正式发布应配置 Developer ID Application 证书、对 DMG 公证并 staple ticket。Apple 签名不能替代 Tauri 更新签名。

## Homebrew Tap

独立的 `boa-z/homebrew-devicehub-mask` Tap 使用 Formula 发布 `devicehub-mask-headless`，并使用 `devicehub-mask` Cask 发布桌面应用。Formula 使用完整的 macOS headless 归档，而不是从桌面 bundle 中提取单个可执行文件。因此 macOS 打包 job 必须同时上传 `devicehub-mask-headless_<version>+<build>_macos-universal.tar.gz` 及其校验文件。

完整 Nightly 发布或非草稿 Stable 发布完成后，如果配置了 `HOMEBREW_TAP_TOKEN`，上游 workflow 会发送 `devicehub-release` repository dispatch。Tap 在替换任一配方前，会确认 headless 归档、DMG 及两个校验文件属于同一版本和构建编号。Stable 草稿不会更新 Homebrew。

## 发布检查清单

1. 运行[开发与构建](development.md)中的验证命令。
2. 确认 commit 作者和目标分支。
3. 推送 `main` 或手动触发 workflow。
4. 确认三个验证矩阵和全部打包 job。
5. 确认 release 包含预期原生安装包、签名和 `latest.json`。
6. 至少安装一个 CI 产物，不要只测试 Cargo target 可执行文件。

## 正式版发布流程

1. 将 `src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml` 和 `package.json` 更新为目标正式版本，三处版本必须一致。
2. 完成本地验证，并推送确切的发布 commit。
3. 打开 **Actions > Publish Stable Release**，选择该 Git ref，输入 `v<version>`（例如 `v0.1.0`）。
4. 保持开启 **Draft** 以便检查候选版；关闭后，工作流会在全部产物上传完成后自动公开发布。
5. 在 macOS、Windows 和 Linux 上验证全新安装和应用内 Stable 更新。
6. 发布后先提升配置中的产品版本，再生成下一个 Nightly。
