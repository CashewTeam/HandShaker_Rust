# HandShaker 通信协议文档

本文档基于对以下逆向材料的分析整理而成：

- **Android 端（协议服务端）**：`Android_jadx/sources/`（jadx 反编译）、`android_smali/`（smali，用于核验）、
  以及 APK 内置的权威源文件 `Android_jadx/resources/main/proto/SmartSyncProtocol.proto`。
- **macOS 端（协议客户端）**：`macos/decompiled/headers/`（626 个 ObjC 类头文件）、
  `macos/HandShaker_Mac.m`（Hopper 反编译主程序）、`macos/analysis/*.json`（方法索引）、
  `macos/HandShaker.app`（二进制字符串核验）。

目标：完整记录 HandShaker 的通信协议，为 Rust 兼容后端（与原版互通）提供实现依据。

## 一句话结论

HandShaker 的局域网设备发现使用 **Apple Bonjour（mDNS/DNS-SD）**，服务类型为
`_handshaker_ssp._tcp.`；设备接入通道有 **USB AOA**、**ADB forward**、**WiFi TCP** 三种；
应用层协议是自研的 **SmartSync Protocol（SSP）**，消息体为 **protobuf（proto2, package `smartsync`）**，
请求帧带 `sessionId + flag` 头，大文件数据面走裸二进制分块流，传输前使用 **RSA-1024/SHA256 签名**。

## 文档分类

| 文档 | 内容 |
|---|---|
| [01-overview](01-overview.md) | 总体架构、三种传输通道、协议分层、消息流 |
| [02-bonjour-discovery](02-bonjour-discovery.md) | Bonjour/mDNS 设备发现（Mac 浏览 / Android 广播 / 二维码） |
| [03-connection-transport](03-connection-transport.md) | USB AOA / ADB / WiFi 传输通道建立与生命周期 |
| [04-handshake-trust](04-handshake-trust.md) | 握手、RSA 密钥交换、PBKDF2 派生密钥、信任状态机 |
| [05-message-framing](05-message-framing.md) | 线路封帧格式、flag 语义、签名、数据面分块、取消 |
| [06-protobuf-schema](06-protobuf-schema.md) | 完整 protobuf 模式（66 个消息 + 全部枚举与字段编号） |
| [07-command-reference](07-command-reference.md) | 全部请求类型、消息语义、典型交互时序 |
| [08-file-operations](08-file-operations.md) | 文件操作：列目录/计数/存在/建目录/重命名/删除/下载/上传/监控 |
| [09-media-library](09-media-library.md) | 媒体库（照片/视频/音频）、缩略图、EXIF、剪贴板 |
| [10-photo-sync](10-photo-sync.md) | 照片同步与实时同步监控（含 FILE_CHANGE 状态机） |
| [11-errors-exceptions](11-errors-exceptions.md) | 错误码、异常场景、心跳/断连/锁屏/版本兼容检测 |
| [12-macos-implementation](12-macos-implementation.md) | macOS 端实现要点（类映射、ADB 命令、音频 HTTP 服务） |
| [13-verification-status](13-verification-status.md) | 已验证/待验证清单与源码引用索引 |
| [14-capture-validation](14-capture-validation.md) | **真实抓包验证报告**（ADB 端口、封帧/分块、parseIoBuffer、签名、上下行数据面、**局域网 mDNS 发现 + WiFi 握手/信任 + 传输**） |
| [15-adb-v0.1-baseline](15-adb-v0.1-baseline.md) | Rust CLI ADB v0.1 基线自动化与真机验收报告 |
| [16-m1-events-cancellation](16-m1-events-cancellation.md) | M1 公共事件订阅、慢消费者契约与请求取消模型 |
| [17-m1-device-validation](17-m1-device-validation.md) | M1 Smartisan U2 Pro 受控事件与清理验收报告 |
| [18-m2-wifi-trust](18-m2-wifi-trust.md) | M2 WiFi 发现、连接与持久化信任（设计与实现记录） |
| [19-m3-directory-watch](19-m3-directory-watch.md) | M3 目录监控与设备/剪贴板主动推送（设计与实现记录） |
| [20-m4-media-library](20-m4-media-library.md) | M4 媒体库与缩略图（设计与实现记录） |
| [21-m5-exif-batch](21-m5-exif-batch.md) | M5 EXIF 拉取、媒体库增量合并与批量/递归传输（设计与实现记录） |
| [22-m6-photo-sync](22-m6-photo-sync.md) | M6 照片同步与实时同步（设计与实现记录） |
| [23-m7-usb-aoa](23-m7-usb-aoa.md) | M7 USB AOA 连接（设计与实现记录） |
| [architecture](architecture.md) | M8 架构：Workspace 分层与数据流 |
| [application-api-v1](application-api-v1.md) | M8 应用服务模型 v1（冻结契约） |
| [ffi-v1](ffi-v1.md) | M8 handshaker-ffi C ABI v1（契约与接入） |
| [m8-migration](m8-migration.md) | M8 迁移记录（提交序列与兼容性结论） |
| [m8-test-report](m8-test-report.md) | M8 测试报告（186 测试与 smoke 验证） |

## 术语

- **Host（主机端）**：macOS/Windows 上运行的 HandShaker（客户端），发起连接。
- **Client / APK（设备端）**：Android 手机上的 `com.smartisanos.smartfolder.aoa`（服务端），监听连接。
- **SSP**：SmartSync Protocol，本协议的应用层。
- **AOA**：Android Open Accessory（USB 配件模式）。
- **ADB**：Android Debug Bridge（经 `adb forward` 建立的 TCP 隧道）。
- **Bonjour/mDNS/DNS-SD**：Apple 的零配置网络发现（`NSNetService`/`NsdManager`）。
- **sessionId**：会话标识，客户端（Mac）在每帧头中携带，用于关联请求与响应 / 数据流。

## 阅读建议

实现互通前请先读 `01`（架构）→ `05`（封帧）→ `04`（握手）→ `06`（消息模式）→ `07`（命令）。
文件/媒体/同步属于上层语义，按需查阅。`13` 列明验证状态；`14` 是真实抓包验证报告；
`tools/capture/` 提供可复现的验证工具。

## 验证状态

关键未确认项（ADB 端口、下行分块边界、`parseIoBuffer`）已于真实设备抓包验证完毕，详见
[14-capture-validation](14-capture-validation.md)。
