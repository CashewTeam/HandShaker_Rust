# 11 错误码、异常场景与检测

## 11.1 协议层错误码

### SSPFileIOError（文件操作）

| 值 | 名称 | 含义 |
|---|---|---|
| 1 | FILE_IO_UNKNOW_ERROR | 未知文件 IO 失败 |
| 2 | FILE_IO_INVALID_NAME | 无效文件名 |
| 3 | FILE_IO_INVALID_SOURCE | 操作目标无效（如重命名的文件不存在） |
| 4 | FILE_IO_TARGET_ALREADY_EXIST | 同名文件已存在 |
| 5 | FILE_IO_PERMISSION_ERROR | 权限错误 |
| 6 | FILE_IO_INSUFFICIENT_DISK_SPACE_ERROR | 磁盘空间不足 |
| 7 | FILE_IO_MD5_CHECK_ERROR | MD5 校验失败 |
| 8 | FILE_IO_SYSTEM_FILE | 系统文件，无法修改 |
| 9 | FILE_IO_SDCARD_REMOVED | SD 卡被拔出 |
| 10 | FILE_IO_SDCARD_NO_PERMISSION | Android<5.0 时读写 SD 卡 |
| 11 | FILE_IO_PATH_OR_NAME_TOO_LONG | 文件名>255 / 路径>4096 |
| 12 | FILE_IO_CANCEL_ACTION | 取消相关操作 |

### SSPCancelErrorCode（取消）

| 值 | 名称 | 含义 |
|---|---|---|
| 1 | ERROR_CODE_UNKNOWN | 未知异常 |
| 2 | ERROR_CODE_SDCARD_REMOVED | SD 卡被拔出 |

### 握手 result（`SSPHandShakeResponse02.result`）

| 值 | 含义 |
|---|---|
| `'failed'` | 握手失败 |
| `'locked'` | 手机锁屏 |
| `'needauth'` | 手机处于授权确认窗口 |
| `base64(RSA_enc('ok'))` | 成功（解密得 `'ok'`） |

## 11.2 帧层异常

| 场景 | Android 行为 | 来源 |
|---|---|---|
| 上行 `length > 0x400000` 或 < 0 | 抛 `InvalidSSPPacketException`，断连 | `service/i.java:183,191` |
| payload 读不满 | 抛异常断连（"Not enough bytes for payload"） | `service/i.java:186-188` |
| 帧头读不满 9 字节 | 视为 EOF，断开 | `service/i.java:175-178` |
| RSA 验签失败（flag=1） | 回裸串 `"rsa verify failed"` | `decoder/a.java:183` |
| protobuf 解析失败 | `catch(com.a.a.o)` 静默丢弃 | `decoder/a.java:178-181` |
| 未知 flag | 日志 "Unexpected flag" 丢弃 | `g/h.java:340-342` |

> ✅ **抓包实测（2026-08）**：错误签名（flag=1）→ 手机回 `"rsa verify failed"`（带 8B 长度前缀），
> 与上表一致。

## 11.3 断连 / 掉线检测

### 心跳超时（Android）

- 默认 30000ms，可被握手协商调大（下限 30s），每 1s 检查一次（`e/a.java`）。
- 任何写操作刷新 lastBeat；超时 → 发 QUIT + 关连接。

### Mac 端判定

- `SFWifiDevice.maxRequestFailedTimes`：请求连续失败达到上限 → 判定设备掉线。
- `SFADBManager.checkKeepAliveHeartBeat:` + `lastHeartBeat`。
- `SFADBForwardKeepAliveOperation` 长连接承载心跳与 watch 推送；断开即通知 UI。

### USB 拔出

- Android：`USB_ACCESSORY_DETACHED` 接收器 → 关连接（`service/d.java:19-25`）。
- Mac：`SFUSBDeviceManager.deviceClosed:` / `SFWifiDeviceManager.deviceClosed:`。

### 连接关闭序列

- 主动：`QUIT_REQUEST(35)`（Mac `sendQuitWithMSTimeout:error:`；Android `Connection.a(true)`）。
- 被动：flag=5（QUIT_FLAG）；SD 卡拔出；心跳超时；帧非法。

## 11.4 锁屏处理

- **仅 USB 通道**拦截：Android 锁屏时对握手请求回裸串 `"locked"`（`decoder/a.java:44-53`）。
- 解锁后（`USER_PRESENT`）重放缓存的握手包（`decoder/a.java:208-222`）。
- WiFi 通道不拦截；`SSPGetDeviceInfoResponse.phone_locked` 上报锁屏状态。

## 11.5 版本兼容检测

- 手机对 `GET_DEVICE_INFO_REQUEST`：本机 APK 版本 < 宿主要求的 `host_min_client_version` →
  提示更新 + 断开（`decoder/a.java:66-84`）。
- UI：宿主 versionCode < 对应类型最低值 → “宿主版本过低”提示 + 停止连接（`MainActivity.java:331-356`）。
- 映射：type1 min `2.1.0`/333；type2 min `2.5.0`/12（`h/d.java:338-358`）。
- 锤子系统安全版本 < 2.5.8 → 弹安全升级提示（`decoder/a.java:224-232`）。

## 11.6 SD 卡 / 存储异常

| 场景 | 表现 |
|---|---|
| SD 卡拔出 | 文件操作返回 `FILE_IO_SDCARD_REMOVED(9)`；下载流被 `stopWriteFile` 中止；取消码 2 |
| 磁盘空间不足 | 上传头响应 `FILE_IO_INSUFFICIENT_DISK_SPACE_ERROR(6)` |
| 外置 SD 无权限 | `FILE_IO_SDCARD_NO_PERMISSION(10)` / `external_storage_permission` |
| DocumentsContract 树丢失 | `DocumentFileChangeObserver` 发 SD 移除事件（`h/q.java:61-115`） |

## 11.7 上传/下载中途失败

- 上传：手机删除半成品文件，回 `CANCEL_REQUEST(36)`（`d/c.java:682-699`）。
- 下载：服务器 `Connection$c.b()`（stopWriteFile）抛 `c.d`（IOException）中止文件流（`g/a.java:244-250`）。
- 客户端可发 flag=2（payload=sid）主动取消任意请求。
  > ⚠️ **抓包实测（2026-08）**：flag=2 只影响排队任务与上传会话，**不中断进行中的下载流**
  > （手机会把剩余文件发完）。真正的下载中断需服务器 `stopWriteFile`。

## 11.8 信任/握手异常

- 公钥指纹不匹配（MD5 不符）→ 不建立公钥 → 握手失败。
- `derived_key` 不符 → `RESPONSE_02(..., 'failed')`。
- 用户拒绝信任（TRUST_NO/UNKNOW）→ `'failed'`，不建立连接。
- 握手阶段超时：Mac 30s 无响应 → `TrustCancelEvent`（`MainActivity.java:91-92,446`）。
- `TRUST_REMOVE`：Mac 发现信任已失效 → request02 带 TRUST_REMOVE，手机清除本地信任数据。

## 11.9 Mac 端文件同步错误域

`NSErrorDomain = com.smartisan.handshaker.filesync`：

| code | 场景 |
|---|---|
| -1000 | 同步项缺失 / 一般失败 |
| -1030 | 照片数据提供者失败 |
| -1040 | 实时同步更新失败 |
| -1110 / -1140 | 文件传输失败 |

另有 `com.smartisan.handshaker.photo.data.provider` 域。

## 11.10 Rust 实现建议

1. 帧解析错误必须断连（两端一致）。
2. 心跳：客户端主动每 ≤30s 发一次 HEART_BEAT；服务端（模拟手机）也应按 30s 检查。
3. 握手 result 必须按 §11.1 语义解析 `'failed'/'locked'/'needauth'/base64(RSA_enc('ok'))`。
4. 文件操作错误统一映射为 `SSPFileIOError` 返回。
5. 锁屏、SD 卡移除、版本过低属于需要主动上报/断连的异常路径。
