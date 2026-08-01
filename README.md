# HandShaker_Rust

HandShaker 是 Smartisan（锤子科技）已经停止维护的 Android 文件传输与设备管理工具。本仓库用于整理现有逆向资料，逐步编写通信协议文档，并为后续开发兼容原版 HandShaker 的跨平台 Rust 后端建立基础。

## 项目目标

1. 基于现有逆向文件，解析并完整记录 HandShaker 通信协议。
2. 优先完成 macOS 版 `HandShaker_CLI`，提供纯命令行使用入口，用于功能测试和 Agent 调用。
3. 在 CLI 后端基础上，实现现代化、可复用的通用跨平台 Rust 后端，使其能够与原版 HandShaker 互通，并为后续 GUI 开发提供基础。

## 跨平台开发顺序

1. 现代 ARM64 macOS
2. Linux
3. 其他平台

## 目录结构

- `docs/`：通信协议文档、研究记录和设计说明（含真实抓包验证报告）。
- `tools/capture/`：SSP 协议抓包验证工具（Python，经 adb forward 与真机对话并逐字节日志）。
- `Android_jadx/`、`android_smali/`、`macos/`：本地保留的逆向与反编译资料，不纳入 Git 记录，也不会由本仓库重新分发。

## 当前状态

- 已完成对 HandShaker 通信协议（SmartSync Protocol / SSP）的完整逆向解析，文档见 `docs/`
  （协议分层、Bonjour 发现、USB AOA/ADB/WiFi 传输、握手与信任、封帧格式、protobuf 模式、文件/媒体/同步、异常处理）。
- 已在真实设备（Smartisan OD103 / Android 7.1.1）上完成**抓包验证**（`docs/14-capture-validation.md`）：
  - ADB 通道：端口、上下行封帧、分块边界、`parseIoBuffer`（AES-256-CBC）、RSA 签名、下载/上传数据面。
  - **局域网通道**：Bonjour/mDNS 发现（`_handshaker_ssp._tcp` 全记录 + SRV 端口实测）、WiFi 握手与
    信任（TRUST_REMOVE / derived_key 重连免弹窗）、局域网传输（数据 MD5 一致）。
  - 验证工具见 `tools/capture/`。
- 暂未开始 Rust 后端实现。

## 后端规划

后端将先以 macOS ARM64 环境下的 `HandShaker_CLI` 为首个可用形态，覆盖命令行操作、协议功能验证和 Agent 调用场景；随后再逐步扩展到 Linux 及其他平台，并为 GUI 提供稳定的通用后端接口。
