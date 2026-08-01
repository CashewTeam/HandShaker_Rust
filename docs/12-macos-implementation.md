# 12 macOS 端实现要点

macOS 端是**协议客户端**。代码分布在两个二进制：

- `HandShaker.app/Contents/MacOS/HandShaker`（主程序，UI + 同步逻辑，`HandShaker_Mac.m`）。
- `SmartFinderCore.framework`（协议核心：设备抽象、SSP、ADB/WiFi 传输）。

## 12.1 类结构（SmartFinderCore）

### 传输 / 设备抽象

| 类 | 职责 |
|---|---|
| `SFGenericDevice`（抽象） | 统一 IO 接口 + RSA 密钥 + 设备属性 + 握手状态机（`aoaHandShaking/aoaHandShakeOk`） |
| `SFUSBDevice : SFGenericDevice` | AOA USB（libusb + AOA control） |
| `SFWifiDevice : SFGenericDevice` | WiFi（`NSNetService` + `SFWifiSocket`） |
| `SFUSBDeviceManager` | libusb 监听（`allVendorIds`） |
| `SFWifiDeviceManager` | Bonjour 浏览 + 连接管理 |
| `SFWifiSocket` | 原生 POSIX socket，超时同步读写 |
| `SFADBManager` | ADB 通道 + 设备目录树 + 缩略图/媒体库调度 |
| `SFADBForwardOperation` | ADB forward 通道上的请求封装（GCDAsyncSocket） |
| `SFADBForwardHandshakeOperation` | 握手 |
| `SFADBForwardKeepAliveOperation` | 长连接：心跳 + watch + 媒体库变更推送 |
| `SFDeviceClient` | 设备级门面（持有 device + adbManager + sspManager） |

### SSP 层

| 类 | 职责 |
|---|---|
| `SSPManager <SFGenericDeviceIODelegate>` | 会话队列/字典、心跳线程、watch 回调、各业务入口 |
| `SSPRequestOperation` | 单帧请求封装（sessionId、data、lenData、timeout、cancel） |
| `SSP*RequestOperation` | 每命令一个（GetDirFiles/Upload/Download/Thumbnail/CreateFolder/Rename/Delete/Monitor/HeartBeat…） |
| `SSP*`（GPBMessage） | 66 个 protobuf 消息，模式见 [06](06-protobuf-schema.md) |
| `SFDeviceTrustStore` / `SFDeviceTrustRecord` | 信任记录持久化 |

### 数据模型

`SFFile`（目录树节点）、`SFImageFile`、`SFVideoFile`、`SFAudioFile`、`SFAlbum`、
`SFClipboard`、`SFDeviceTrustRecord` 等。

## 12.2 SFGenericDeviceIOProtocol（IO 契约）

```objc
- syncSendRequestData:withMSTimeout:error:      // 同步发送（握手阶段）
- syncReadResponseDataWithMSTimeout:error:      // 同步读取
- sendData:withSessionId:withFlag:              // 异步发送（带 sid + flag）
- sendHandShakeRequest01WithMSTimeout:error:    // 握手 01
- sendHandShakeRequest02WithMSTimeout:error:    // 握手 02
- sendHandShakeCancelRequestWithMSTimeout:error:
- sendQuitWithMSTimeout:error:
- sendRequestData:withSessionId:error:          // 业务请求
- sendFileData:withSessionId:error:             // 文件数据
- cancelRequest:withOldSessionId:
- maxDataPackageSize
```

对应实现：`SFGenericDevice getRequest00Data`（USB 密钥交换裸包）、`getRequest01WithError:`
`getRequest02WithError:`（握手 protobuf）、`getSignatureForData:`（SHA256withRSA 签名）、
`checkResult:`（解密 `'ok'`）。

## 12.3 ADB 通道命令（SmartFinderCore 二进制字符串还原）

```
1. adb shell am force-stop com.smartisanos.smartfolder
2. adb shell am startservice --user 0 -n com.smartisanos.smartfolder/.AdbForwardService --ei ADB_PORT <port>
3. adb -s <serial> forward tcp:<hostPort> tcp:10086        // 正则：%@ tcp:(\d+) tcp:10086
4. GCDAsyncSocket connect 127.0.0.1:<hostPort>
```

- 手机侧 `AdbForwardService` 读取 `ADB_PORT` 后 `new ServerSocket(port)`（见 [03](03-connection-transport.md)）。
- `tcp:19999` 亦出现（与 `audioHttpServerSocketPort` 相关的第二条转发）。
- 状态检测：`dumpsys activity services com.smartisanos.smartfolder`、
  `dumpsys package com.smartisanos.smartfolder`、`am force-stop`。

## 12.4 音频 HTTP 服务

- `SFADBManager.startAudioHttpServer` / `audioHttpServerSocketPort` / `audioPlayUrl:` /
  `downloadAudioPlayUrl:toLocalPath:`。
- 字符串：`http://127.0.0.1:%d/`、`http://127.0.0.1:%lu/?file_path=%@` —— Mac 起本地 HTTP 服务，
  通过 ADB forward 打通，向手机请求音频流播放/下载。

## 12.5 目录树与路径（Mac 侧缓存）

`SFADBManager` 维护：`rootDir / sdcardRootDir / fromMacDir / downloadDir / audioDir / cameraDir /
videoDir / screenshotsDir / quickcaptureDir`。`fromMacDir`/`downloadDir` 为 Mac 概念目录，
手机端无对应常量（见 [08](08-file-operations.md) §8.12）。

## 12.6 文件传输流程（Mac 侧）

- 下载：`downloadFiles:toHostPath:...`（含 analysis/conflict/predownload/progress/postdownload/finish
  回调）→ `SSPDownloadFileRequestOperation`：收 header → 累积数据到 bufferPath → 计算 MD5 → 进度。
- 上传：`uploadFiles:to:needUpdateMediaLibrary:...` → `SSPUploadFileRequestOperation`：
  发 header → 等 ready → 读本地文件分块（flag=3）→ 收 type 18。
- 冲突处理：`conflictCallback`（存在同名/空间不足等），Mac 端可自动重命名 `newFilename`。

## 12.7 同步（Mac 侧）

- `SFSynchManager` + `SFSyncSession` + `SFSyncFile` + Process 链
  （`SFSyncNotifyBeginProcess`、`SFSyncDisableRealTimeSyncProcess`、
  `SFSyncCheckFreeDiskCapacityProcess`、`SFSyncStepGroup`）。
- 上次会话文件枚举存于 `filesDict1AfterSync` 用于 diff。
- 错误域 `com.smartisan.handshaker.filesync`（-1000/-1030/-1040/-1110/-1140）。
- `SFPhotoLocalDataProvider` 负责本地照片缓存与元数据。

## 12.8 账号/云（SmartFinderNetwork，与本协议无关）

`SmartFinderNetwork.h` 实际是 **账号/云同步 HTTP 层**（SFBaseRequest 体系：登录、注册、ticket、
token、TP 云同步、推送等），走 HTTPS + AFNetworking，与本机的 SSP 局域网协议**无直接关系**，
不在互通范围内。文档仅作区分，避免混淆。

## 12.9 Rust 实现的 Mac 侧参考

- 模拟 Mac（客户端）：实现 `SFGenericDeviceIOProtocol` 等价物：
  - 发送帧 `[sid:4][flag:1][len:4][payload]`；
  - 接收 `[sid:4][chunkLen:2]` 帧流并重组（普通响应含 8 字节总长前缀）；
  - 握手用同步收发；业务用 sid 关联。
- 会话：`sessionQueue/sessionDict` 按 sid 路由响应；心跳线程周期发 HEART_BEAT。
- 文件传输：下载收 header+帧流写盘；上传 header→ready→flag=3 分块。
