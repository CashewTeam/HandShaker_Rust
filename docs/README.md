# HandShaker 文档

本文档基于对以下逆向材料的分析整理而成：

- **Android 端（协议服务端）**：`Reference/Android_jadx/sources/`（jadx 反编译）、`Reference/android_smali/`（smali，用于核验）、
  以及 APK 内置的权威源文件 `Reference/Android_jadx/resources/main/proto/SmartSyncProtocol.proto`。
- **macOS 端（协议客户端）**：`Reference/macos/decompiled/headers/`（626 个 ObjC 类头文件）、
  `Reference/macos/HandShaker_Mac.m`（Hopper 反编译主程序）、`Reference/macos/analysis/*.json`（方法索引）、
  `Reference/macos/HandShaker.app`（二进制字符串核验）。

目标：完整记录 HandShaker 的通信协议，为 Rust 兼容后端（与原版互通）提供实现依据。

## 一句话结论

HandShaker 的局域网设备发现使用 **Apple Bonjour（mDNS/DNS-SD）**，服务类型为
`_handshaker_ssp._tcp.`；设备接入通道有 **USB AOA**、**ADB forward**、**WiFi TCP** 三种；
应用层协议是自研的 **SmartSync Protocol（SSP）**，消息体为 **protobuf（proto2, package `smartsync`）**，
请求帧带 `sessionId + flag` 头，大文件数据面走裸二进制分块流，传输前使用 **RSA-1024/SHA256 签名**。

## 文档分类

### 项目入口

- [../CHANGELOG](../CHANGELOG.md) — 版本变更与当前开发状态
- [GitHub Wiki / Command-Line](https://github.com/CashewTeam/HandShaker_Rust/wiki/Command-Line) — 命令行教程

### 协议

协议事实、消息定义、兼容行为和真实设备验证记录：

- [protocol/01-overview](protocol/01-overview.md) — 协议总览、传输通道与消息流
- [protocol/02-bonjour-discovery](protocol/02-bonjour-discovery.md) — Bonjour/mDNS 设备发现
- [protocol/03-connection-transport](protocol/03-connection-transport.md) — USB AOA / ADB / WiFi 传输
- [protocol/04-handshake-trust](protocol/04-handshake-trust.md) — 握手、密钥交换与信任
- [protocol/05-message-framing](protocol/05-message-framing.md) — 线路封帧、签名与数据面
- [protocol/06-protobuf-schema](protocol/06-protobuf-schema.md) — protobuf 模式与字段定义
- [protocol/07-command-reference](protocol/07-command-reference.md) — 请求类型与交互时序
- [protocol/08-file-operations](protocol/08-file-operations.md) — 文件操作协议
- [protocol/09-media-library](protocol/09-media-library.md) — 媒体库、缩略图、EXIF 与剪贴板
- [protocol/10-photo-sync](protocol/10-photo-sync.md) — 照片同步与实时监控
- [protocol/11-errors-exceptions](protocol/11-errors-exceptions.md) — 协议错误码与异常场景
- [protocol/13-verification-status](protocol/13-verification-status.md) — 验证状态与源码索引
- [protocol/14-capture-validation](protocol/14-capture-validation.md) — 真实设备抓包验证报告

### 架构

- [architecture/architecture](architecture/architecture.md) — Workspace 分层、Application 数据流与生命周期
- [architecture/12-macos-implementation](architecture/12-macos-implementation.md) — 原版 macOS 端实现要点

### API

- [api/application-api-v1](api/application-api-v1.md) — `handshaker-application` v1 冻结契约
- [api/ffi-v1](api/ffi-v1.md) — `handshaker-ffi` C ABI 契约
- [api/ffi-abi-snapshot](api/ffi-abi-snapshot.md) — 生成的 ABI 导出快照

### 归档

已完成里程碑、迁移基线、测试报告、审计报告和历史计划：

- [archive/README](archive/README.md) — M1–M8、Phase D、Swift 交付审计及历史计划/验收记录

## 术语

- **Host（主机端）**：macOS/Windows 上运行的 HandShaker（客户端），发起连接。
- **Client / APK（设备端）**：Android 手机上的 `com.smartisanos.smartfolder.aoa`（服务端），监听连接。
- **SSP**：SmartSync Protocol，本协议的应用层。
- **AOA**：Android Open Accessory（USB 配件模式）。
- **ADB**：Android Debug Bridge（经 `adb forward` 建立的 TCP 隧道）。
- **Bonjour/mDNS/DNS-SD**：Apple 的零配置网络发现（`NSNetService`/`NsdManager`）。
- **sessionId**：会话标识，客户端（Mac）在每帧头中携带，用于关联请求与响应 / 数据流。

## 阅读建议

实现互通前请先读 `protocol/01`（总览）→ `protocol/05`（封帧）→ `protocol/04`（握手）→
`protocol/06`（消息模式）→ `protocol/07`（命令）。文件/媒体/同步属于上层语义，按需查阅。
`protocol/13` 列明验证状态；`protocol/14` 是真实抓包验证报告；
`tools/capture/` 提供可复现的验证工具。

## 验证状态

关键未确认项（ADB 端口、下行分块边界、`parseIoBuffer`）已于真实设备抓包验证完毕，详见
[protocol/14-capture-validation](protocol/14-capture-validation.md)。
