# 真机回归验证

[English](../en/device-regression.md) | [文档目录](README.md)

修改 runtime、传输、媒体或宿主边界后使用此清单。每次记录 commit、设备型号、iOS 版本、UDID、传输方式、日期和结果。CI 通过不能证明真机行为正常。

## 自动只读 USB 检查

通过 USB 连接且只连接一台已解锁、已信任的设备。用 `idevice_id -l` 获取 UDID，然后运行：

```sh
npm run verify:device -- --udid <UDID>
```

当 `idevice_id` 不可用、USB 设备数量不为一或传入的 UDID 不匹配时，命令会在测试前失败。它不会启动桌面应用，只会串行执行以下只读检查：

- 心跳响应
- 设备信息与开发者模式状态
- 原生截图
- 描述文件列表
- syslog 读取
- 公共 AFC 根目录列表
- 已安装 App 发现与图标读取
- sysmontap 进程字段和采样归一化

该命令明确排除配对与撤销信任、Developer Image 挂载、网络抓包、App 生命周期操作、重启与关机、AFC 写入和描述文件变更。

## 桌面端人工回归

仅在测试人员明确授权时启动当前源码构建的应用。启动前确认可执行文件路径，避免误用已经安装的 release 应用。

### USB 会话与媒体

- 通过 USB 连接预期设备，核对型号、iOS 版本和 UDID。
- 确认 WebCodecs 接收、解码并呈现 HEVC 帧，且没有回退至原生视频解码器。
- 将设备停留在静止画面，确认不会误报视频停止或显示重连提示。
- 确认设备声音可听，静音和音量控制正常。
- 主动执行重新连接，确认画面、声音、输入和只读服务均恢复。

相关日志包括 `selected CoreDevice transport`、`selected video decoder backend decoder_backend=Browser`、关键帧接收、浏览器呈现指标、音频 RTP 检测和会话重连状态。

### 输入

- 验证点按、持续按下、拖动和双触点多点触控。
- 验证键盘映射只触发预期控件，并且能正常释放。
- 验证 Home、音量、锁定和其他已开放的硬件按键。

### App 与 AFC

- 加载 App 列表和图标，然后启动、停止并重新启动一个现有 App。
- 启动和停止 App 控制台，确认输出属于当前选择的 App。
- 列出并读取 AFC 内容。仅在测试人员明确授权修改测试数据时执行写入和取消操作。

### Wi-Fi 连续性

- 完成 USB 授权后，确认同一台物理设备显示为支持 Wi-Fi。
- 拔出数据线，确认活动会话保持或通过 Wi-Fi 恢复连接。
- 短暂中断 Wi-Fi，确认服务监督能恢复画面、声音、输入、App 和 AFC。
- 确认 USB 与 Wi-Fi 发现结果对应预期 UDID，并且不会为同一台物理设备建立两个并发会话。

## 证据记录

至少按以下模板把每次结果记录到 issue、pull request 或发布说明：

```text
Commit:
日期：
设备型号：
iOS 版本：
UDID 指纹：
传输：USB / Wi-Fi
自动只读检查：
WebCodecs：
音频：
输入：
App：
AFC：
重连：
相关日志路径或摘录：
结果与失败项：
```
