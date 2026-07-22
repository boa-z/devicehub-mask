# CI、发布与更新

简体中文 | [English](../en/distribution.md) | [文档首页](README.md)

## 工作流触发方式

`.github/workflows/nightly.yml` 只在 commit 和手动触发时运行。没有定时任务，也不使用
GitHub Environments，因此不会创建妨碍清理历史的 Deployment 记录。

## Jobs

- **verify** 使用相互独立失败的 macOS、Windows 和 Linux 矩阵。每个平台运行前端
  lint、测试和构建，Rust 格式、测试和 Clippy，以及 Tauri debug 应用构建。
- **build-macos** 生成 Apple Silicon/Intel Universal DMG，并验证两个可执行架构和完整
  应用签名。
- **build-windows** 生成 x64 NSIS 和 MSI 安装包。
- **build-linux** 生成 x64 AppImage 和 DEB。
- **publish-nightly** 等待全部安装包，将更新片段合并成一个 `latest.json`，然后原子替换
  滚动 nightly release 的资源。

工作流 artifact 保留 14 天。公开滚动发布地址：

<https://github.com/boa-z/devicehub-mask/releases/tag/nightly>

## 版本与产物

安装包文件名包含基础版本和 workflow build number。macOS 使用运行编号作为
`CFBundleVersion`。更新产物统一使用 `major.minor.<run-number>`，因为
`0.1.0+12` 这样的 SemVer build metadata 不参与更新顺序比较。

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
latest.json
```

## Tauri 更新签名

更新签名与 Apple 代码签名相互独立。公钥提交在 `src-tauri/tauri.conf.json`，私钥绝对
不能提交到仓库。

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

```sh
gh secret set TAURI_SIGNING_PRIVATE_KEY < .tauri/devicehub-mask.key
gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD
```

缺少私钥时，CI 仍可发布原生安装包，但会跳过更新签名和 `latest.json`。私钥丢失或替换
后，已有安装将无法接受新密钥签名的更新。

运行时可在设置页关闭自动 nightly 检查，选项保存在
`devicehub-mask.updates.automatic`，手动检查始终可用。接受更新后会下载、验证、安装
并重启应用。

## Apple 签名与公证

当前 nightly macOS 应用在 Universal 合并和版本写入后使用结构有效的 ad-hoc 签名。
它能验证 sealed resources 和二进制架构，但不能证明发布者身份，Gatekeeper 仍可能要求
用户手动批准。

正式发布应配置 Developer ID Application 证书、对 DMG 公证并 staple ticket。Apple
签名不能替代 Tauri 更新签名。

## 发布检查清单

1. 运行[开发与构建](development.md)中的验证命令。
2. 确认 commit 作者和目标分支。
3. 推送 `main` 或手动触发 workflow。
4. 确认三个验证矩阵和全部打包 job。
5. 确认 release 包含预期原生安装包、签名和 `latest.json`。
6. 至少安装一个 CI 产物，不要只测试 Cargo target 可执行文件。
