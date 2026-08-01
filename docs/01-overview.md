# 01 总体架构

## 1.1 两端角色

| | macOS HandShaker | Android HandShaker |
|---|---|---|
| 角色 | **客户端（Host）**，主动发起连接 | **服务端（Client/APK）**，监听连接、执行文件/媒体操作 |
| 包/二进制 | `HandShaker.app`（主程序）+ `SmartFinderCore.framework`（协议核心） | `com.smartisanos.smartfolder.aoa` |
| 主要类 | `SFDeviceClient`、`SSPManager`、`SFGenericDevice`、`SFADBManager`、`SFWifiDeviceManager`、`SFUSBDeviceManager` | `ConnectionManagerService`、`g.a`(Connection)、`g.h`(SspExecutorManager)、`decoder.a`(Decoder)、`g.j`(Transfer) |

> **角色澄清（三个维度，结论一致：Mac=客户端，Android=服务端）**：
>
> | 维度 | Mac 端 | Android 端 |
> |---|---|---|
> | TCP 连接 | 主动发起连接（dial） | 监听连接（`ServerSocket` accept） |
> | 请求/响应 | 发起请求（握手/列目录/下载/上传…），收响应与推送 | 执行请求，返回响应，并主动推送（媒体库变更、文件事件…） |
> | 锤子协议术语 | **host 端**（主机=电脑） | **client/APK 端**（手机里的 APK 程序） |
>
> `host`/`client` 是 Smartisan 在协议（proto 注释、字段名如 `host_min_client_version`）中的自用叫法：
> "host" 指电脑主机，"client" 指手机上的 APK——**与"谁主动、谁响应"无关，不表示角色反转**。

## 1.2 三种传输通道

| 通道 | 触发方式 | 手机侧监听 | 主机侧连接 |
|---|---|---|---|
| **USB AOA** | 插入 USB，MainActivity 收到 `USB_ACCESSORY_ATTACHED` | `UsbManager.openAccessory()` 拿到的 fd（bulk 流） | 同一 fd |
| **ADB forward** | Mac 运行 `adb forward tcp:<host> tcp:10086`，再启动 `AdbForwardService` | `new ServerSocket(ADB_PORT)` | 连接 `127.0.0.1:<hostPort>` |
| **WiFi** | Bonjour 广播 `_handshaker_ssp._tcp.` 或扫码 | `ServerSocket(0)` 随机端口 | 经 Bonjour 解析地址后连接 |

三种通道最终都汇入同一套帧解析管线（见 §1.4）。

## 1.3 协议分层

```
┌─────────────────────────────────────────────────┐
│ 应用层：文件管理 / 媒体库 / 剪贴板 / 照片同步       │
├─────────────────────────────────────────────────┤
│ SSP 消息层：protobuf (proto2, package smartsync) │
│  每个消息 field 1 = SSPRequestType（命令类型）    │
├─────────────────────────────────────────────────┤
│ 签名层：RSA-1024 SHA256withRSA，仅上行方向        │
│  payload = [128B 签名][protobuf 体]               │
├─────────────────────────────────────────────────┤
│ 帧层：sessionId + flag + 长度（上下行不对称）      │
├─────────────────────────────────────────────────┤
│ 传输层：AOA bulk / ADB forward TCP / WiFi TCP     │
└─────────────────────────────────────────────────┘
```

## 1.4 两端管线

**Android（服务端）**，`service/i.java`（TcpSocketManager 读线程）→ `g/b.java` → `g/h.java`
（SspExecutorManager 按 flag 分派）→ `decoder/a.java`（验签 + 按命令类型分发）→
各 Handler（`d/c.java` FileProcessor、`g/j.java` Transfer、`d/e.java` MediaProcessor 等）。

**macOS（客户端）**，`SFGenericDevice`（IO 抽象，USB/AOA/WiFi 各一子类）→ `SSPManager`
（会话队列 + 心跳线程）→ `SSPRequestOperation`（封装一帧请求）→ 各业务 Operation
（如 `SSPUploadFileRequestOperation`、`SSPDownloadFileRequestOperation`）。

## 1.5 关键常量

| 项 | 值 | 来源 |
|---|---|---|
| Bonjour 服务类型 | `_handshaker_ssp._tcp.` | `service/m.java:201`；Mac 二进制字符串 `_handshaker_ssp._tcp` |
| Bonjour 服务名 | `handshaker_ssp_` | `service/m.java:200` |
| 上行 payload 上限 | 4,194,304 (0x400000) | `service/i.java:183` |
| 下行分块缓冲 | 32767 (0x7fff)，数据 ≤32761 | `g/a.java:200,216` |
| 心跳超时 | 默认 30000ms，可被握手协商调大（下限 30s） | `e/a.java:21,82-84` |
| USB AOA 过滤 | manufacturer=`Smartisan`, model=`HandShaker`, version=`1` | `res/xml/accessory_filter.xml` |
| ADB 目标端口 | `tcp:10086`（另有 `tcp:19999`，音频相关） | Mac 二进制正则 `%@ tcp:(\d+) tcp:10086` |
| SSP 协议版本 | client 侧 "1"，min host "2.1.0"（type 1）/"2.5.0"（type 2） | `h/d.java:338-358` |

## 1.6 握手与命令流（概览）

```
1. 传输层建立（AOA / ADB / WiFi）
2. 握手阶段（flag=0，未签名）：
   - USB：交换 RSA 公钥 + 回加密 "ok"
   - WiFi：HANDSHAKE_REQUEST_01(31) → RESPONSE_01(32)
           HANDSHAKE_REQUEST_02(33) → RESPONSE_02(34)（可多次，最后带 result）
3. 业务阶段（flag=1，RSA 签名）：
   - GET_DEVICE_INFO_REQUEST(2) → RESPONSE（设备信息 + 各媒体库监控开关）
   - 文件/媒体/剪贴板/同步等业务请求
   - HEART_BEAT_REQUEST(1) 周期性心跳
4. 退出：QUIT_REQUEST(35) 或 flag=5
```

## 1.7 相关源码索引

- Android 主服务：`Android_jadx/sources/com/smartisanos/smartfolder/aoa/service/ConnectionManagerService.java`
- Android 帧读取：`.../aoa/service/i.java`；帧写出：`.../aoa/g/a.java`
- Android 分发：`.../aoa/g/h.java`；解码：`.../aoa/decoder/a.java`
- Mac 协议核心头：`macos/decompiled/headers/SmartFinderCore.h`
- Mac IO 协议接口：`macos/interfaces/Protocols/SmartFinderCore_Protocols.h`（`SFGenericDeviceIOProtocol`）
