# HandShaker Protocol

HandShaker 是 Smartisan（锤子科技）已经停止维护的 Android 文件传输与设备管理工具。本仓库用于整理现有逆向资料，逐步编写通信协议文档，并为后续开发兼容原版 HandShaker 的跨平台 Rust 后端建立基础。

## 项目目标

1. 基于现有逆向文件，解析并完整记录 HandShaker 通信协议。
2. 实现现代化、可复用的通用跨平台 Rust 后端，使其能够与原版 HandShaker 互通，并为后续 GUI 开发提供基础。

## 跨平台开发顺序

1. 现代 ARM64 macOS
2. Linux
3. 其他平台

## 目录结构

- `docs/`：通信协议文档、研究记录和设计说明。
- `Android_jadx/`、`android_smali/`、`macos/`：本地保留的逆向与反编译资料，不纳入 Git 记录，也不会由本仓库重新分发。

## 当前状态

项目目前处于初始化阶段，已完成 Git 仓库和文档目录的建立。暂未开始具体的协议解析，也未加入 Rust 后端或其他实现代码。
