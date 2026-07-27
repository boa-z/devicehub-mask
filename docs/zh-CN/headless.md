# 无头服务

简体中文 | [English](../en/headless.md) | [文档首页](README.md)

实验性无头宿主与 Tauri 桌面应用使用同一套设备运行时、鉴权 HTTP/WebSocket API、WebCodecs 传输和 React 界面，但不链接 Tauri 或 Wry。

## Nightly 包

从 [nightly release](https://github.com/boa-z/devicehub-mask/releases/tag/nightly) 下载对应平台的归档，使用相邻的 `.sha256` 文件校验后完整解压。`devicehub-headless`、`dist/`、FFmpeg、netmuxd 和许可证文件必须保持在同一目录中。

在解压目录中启动仅监听回环接口的服务：

```sh
./devicehub-headless
```

Windows 使用 `devicehub-headless.exe`。打开进程输出的 URL。临时访问令牌位于 URL fragment 中，浏览器完成引导后会将其从地址栏移除。

## 局域网访问

局域网监听必须显式启用：

```sh
./devicehub-headless --listen 0.0.0.0:8080 --allow-lan
```

在其他电脑打开前，将输出 URL 中的 `127.0.0.1` 替换为服务器的局域网地址。内置服务目前提供令牌鉴权，但不提供 TLS 或用户账户，不要直接暴露到互联网。客户端需要固定凭据时，可通过 `--token-file` 提供至少 24 字符的 URL 安全令牌；Unix 上应将该文件权限设为 `0600`。

运行 `./devicehub-headless --help` 可查看数据目录、前端目录、设备选择、sidecar、usbmuxd 和可选回环 MCP 配置。

## 宿主集成

浏览器界面通过经过鉴权的 headless API 读取构建信息、能力声明、设置和诊断状态。headless 设置保存在 `<data-dir>/settings.json`，浏览器错误会转发到服务进程日志。浏览器全屏可用；窗口置顶、安装器更新、原生文件选择框、打开服务端目录、宿主剪贴板同步和设备音频等桌面专属能力会被明确禁用，不再错误调用 Tauri 命令。浏览器原生文件传输和音频传输是后续对齐阶段。

DeviceHub Mask 不安装、侧载、签名或升级 iOS 应用。桌面端与无头端都不会加入这些能力。
