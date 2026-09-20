# 03 连接传输通道

三种通道最终都收敛到同一套帧管线（`decoder/a.java` + `g/h.java`）。本文描述各通道如何建立与拆除。

## 3.1 USB AOA（Android Open Accessory）

- **过滤**：`res/xml/accessory_filter.xml` → `manufacturer="Smartisan" model="HandShaker" version="1"`。
- **Manifest**：`uses-feature android.hardware.usb.accessory required=true`；MainActivity 带
  `USB_ACCESSORY_ATTACHED` intent-filter + `@xml/accessory_filter`。
- **权限**：`MainActivity.m()`（`MainActivity.java:502-516`）`UsbManager.requestPermission(...)`;
  收到 `USB_PERMISSION` 广播后再连接（`MainActivity.java:116-133`）。
- **打开通道**（`service/a.java:36-53`）：`UsbManager.openAccessory(accessory)` → `ParcelFileDescriptor`
  → `FileInputStream`/`FileOutputStream` → 包装成读写端。读侧用 `BufferedInputStream(stream, 16384)`。
- **断开**：`USB_ACCESSORY_DETACHED` 接收器（`service/d.java:19-25`）→ 关连接。
- AOA 协议命令（`ACCESSORY_START` 等）由系统驱动完成，App 层只见 bulk 字节流，直接承载 SSP 帧。

Mac 端对应 `SFUSBDeviceManager`（libusb 监听）+ `SFUSBDevice`（发送 AOA control 字符串、
`sendStartAccessoryControl`、`getAOAVersion`、`isSupportAOAv2`），同样只做字节流。

## 3.2 ADB forward（推荐的主测试通道）

**手机端**不内置 adb daemon，而是被宿主机用 `adb forward` 把端口映射到手机的 `ServerSocket`。

Android 端（`ConnectionManagerService.java:122-128`）：

```java
Bundle extras = intent.getExtras();
if (extras != null) {
    int i3 = extras.getInt("ADB_PORT");   // 由宿主通过 startService 传入
    this.d.a(i3);                          // l.a(int) → new ServerSocket(port)
}
```

- `ConnectionManagerService` 声明 `android:exported="true"`（可被 `adb shell am startservice` 启动并传参）。
- `l.a(int)`（`service/l.java:77-79`）→ `i.a(int)`（`service/i.java:111-132`）= `new ServerSocket(port)`。
- App 内唯一端口来源就是 `ADB_PORT` extra，无硬编码 10086/19999。

**Mac 端**（SFADBManager / SFADBForwardOperation），由二进制字符串还原出的命令序列：

1. `adb shell am force-stop com.smartisanos.smartfolder`（清旧实例）
2. `adb shell am startservice --user 0 -n com.smartisanos.smartfolder/.AdbForwardService --ei ADB_PORT <port>`
3. `adb -s <serial> forward tcp:<hostPort> tcp:10086`（正则 `%@ tcp:(\d+) tcp:10086`；另有 `tcp:19999`）
4. `GCDAsyncSocket` 连接 `127.0.0.1:<hostPort>`，开始 SSP。

说明：

- 10086 / 19999 是手机侧期望的监听端口（ADB_PORT 传同样值）；19999 与音频 HTTP 服务相关。
- ✅ **已抓包确认（2026-08）**：手机端 `am startservice ... --ei ADB_PORT 10086` 后
  `/proc/net/tcp` 显示 `0x2766`（10086）LISTEN；`adb forward tcp:10086 tcp:10086` 后即可通信。
- 所有请求/响应在 ADB 通道上走与 WiFi 完全相同的帧格式。
- `SFADBManager` 持有多条队列：`adbOperationQueue`、`adbForwardOperationQueue`、
  `keepAliveOperationQueue`、`thumbnailOperationQueue`；并有 `SFADBForwardHandshakeOperation`、
  `SFADBForwardKeepAliveOperation`（长连接）等子操作。

## 3.3 WiFi TCP

见 [02-bonjour-discovery](02-bonjour-discovery.md)。要点：手机 `ServerSocket(0)` 随机端口，
经 NsD 广播；Mac 解析地址后 `SFWifiSocket` 连接；同一 IP 已有连接时关闭旧连接
（`service/i.java:213-235`）。

## 3.4 连接生命周期（Android 侧）

```
onStartCommand / accept / openAccessory
  → new g.a Connection（启动读线程 g.b）
  → g.h SspExecutorManager.a(packet) → decoder.a(sid, flag, payload)
  → 握手完成 → g.c onConnected：
      mIsConnected=true, FolderApp.e=deviceUuid
      绑定 MediaScannerService，启动心跳定时器
  → 业务请求...
  → 断开 g.c onDisconnected：
      停心跳, 解绑 MediaScanner, 通知 UI
```

### 主动关闭（graceful）

Android 侧 `g.a.Connection.a(boolean)`（`g/a.java:95-115`）：

1. 构造 `QUIT_REQUEST`（type=35）protobuf，分配新 sid，经写出端发送。
2. 中断读线程、关闭 reader、关闭 writer。

Mac 侧 `sendQuitWithMSTimeout:error:` 对应。

### 被动关闭

- 宿主发 flag=5（QUIT_FLAG）→ `g/h.java:336-339` → `Connection.b(false)`。
- 心跳超时（见 §3.5）。
- 帧头非法 / payload 不完整 → 抛异常断连。

## 3.5 心跳 / 保活

- 协议消息：`HEART_BEAT_REQUEST(1)` 请求 `{type, host_timestamp}`；响应 `{type, host_timestamp, client_timestamp}`。
  Android 响应实现 `g/j.java:264-266`：回显 host 时间戳 + 返回本地最后心跳时刻。
- Android 监控器 `HeartBeatChecker`（`e/a.java`）：
  - 默认超时 **30000ms**（`e/a.java:21`），握手可调大（request01 字段11 秒），下限 30s。
  - 每 **1s** 检查一次（`e/a.java:75`）。
  - 任何写操作都会刷新 `lastBeat`（`g/a.java:229-234` 写路径调 `e.a().c()`）。
  - 判定：`uptimeMillis - lastBeat > timeout` → 发 QUIT + 关连接。
- Mac 侧 `SFADBManager.checkKeepAliveHeartBeat:` / `SSPManager.checkKeepAliveHeartBeat`、
  `SFADBForwardKeepAliveOperation`（长连接，还负责 watch 目录、媒体库变更推送）。

## 3.6 Rust 实现要点

- 推荐用 ADB forward 做兼容性验证：宿主机 `adb forward` + 向 `127.0.0.1` 发 SSP 帧即可，
  无需处理 USB 驱动。
- 作为模拟 Mac（客户端）：主动方需实现：启动 ADB forward / 连接 WiFi 地址 → 发握手 → 心跳。
- 作为模拟手机（服务端）：监听端口 → 收帧解析 → 回帧，需实现 §05 的不对称封帧。
