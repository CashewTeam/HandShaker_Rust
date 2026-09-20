<p align="center">
  <img src="docs/handshaker_rust.png" alt="HandShaker Rust" width="180">
</p>

<h1 align="center">HandShaker Rust</h1>

<p align="center">
  <a href="https://github.com/CashewTeam/HandShaker_Rust/actions/workflows/macos-ci.yml">
    <img src="https://github.com/CashewTeam/HandShaker_Rust/actions/workflows/macos-ci.yml/badge.svg" alt="macOS CI">
  </a>
  <a href="https://github.com/CashewTeam/HandShaker_Rust/blob/main/Cargo.toml">
    <img src="https://img.shields.io/badge/version-0.7.5-blue" alt="Workspace version 0.7.5">
  </a>
  <a href="https://github.com/CashewTeam/HandShaker_Rust/blob/main/Cargo.toml">
    <img src="https://img.shields.io/badge/rust-2024-orange?logo=rust" alt="Rust 2024 edition">
  </a>
  <a href="https://github.com/CashewTeam/HandShaker_Rust/blob/main/docs/api/application-api-v1.md">
    <img src="https://img.shields.io/badge/Application%20API-1.0.0-4c1" alt="Application API 1.0.0">
  </a>
  <a href="https://github.com/CashewTeam/HandShaker_Rust/blob/main/docs/api/ffi-v1.md">
    <img src="https://img.shields.io/badge/FFI%20ABI-1.5.0-4c1" alt="FFI ABI 1.5.0">
  </a>
</p>

HandShaker Rust 是基于原版 HandShaker 的通信协议开发的跨平台 Rust 后端实现，目标是重写已经停止维护的 HandShaker，让 HandShaker 可以继续在 macOS Arm64 原生运行，并在后续支持 Linux 系统。

## 项目目标

- 兼容原版 HandShaker 的 SSP 通信协议。
- 提供可复用的 Rust Application 服务层和稳定的 C ABI。
- 支持 macOS Arm64 与 Linux 原生运行，并保持未来跨平台扩展能力。

## 项目结构

- `crates/handshaker-core/`：协议、传输、握手、Session 和底层领域能力。
- `crates/handshaker-application/`：UI 无关的 Runtime、Session、Transfer、事件和业务服务。
- `crates/handshaker-cli/`：`handshaker` 命令行客户端及终端输出适配。
- `crates/handshaker-ffi/`：稳定 C ABI，供 Swift、.NET 等语言包装层使用。
- `docs/`：协议、架构、API 和归档文档。
- `proto/smartsync.proto`：权威 SSP proto2 schema。
- `tools/capture/`：协议抓包与复现工具。

## 文档入口

- [Changelog](CHANGELOG.md)：版本变更与当前开发状态。
- [协议、架构与 API 文档](docs/README.md)：按协议、架构、API、归档分类的技术文档。
- [命令行教程 Wiki 文档](https://github.com/CashewTeam/HandShaker_Rust/wiki/Command-Line)：CLI 安装、设备选择、文件传输和自动化用法。
- [开发计划](plan.md)：后端能力现状、限制和后续计划。
- [AGENTS.md](AGENTS.md)：开发约束、协议不变量和验证要求。

## 快速开始

```sh
cargo build --release
./target/release/handshaker --help
```

使用 CLI 前需要安装 Android SDK Platform-Tools，并确保 `adb` 已加入 `PATH`。完整命令说明请参阅
[命令行教程 Wiki 文档](https://github.com/CashewTeam/HandShaker_Rust/wiki/Command-Line)。

## 版本与兼容身份

Cargo Workspace 版本、Application API、FFI ABI 和 CLI JSON schema 独立维护。手机端兼容身份固定为
原版 HandShaker 的 `host_app_version = 2.5.6`、`host_app_version_code = 408`，不随 Cargo 版本变化。
