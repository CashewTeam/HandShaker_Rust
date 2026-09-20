# Changelog

本文件按版本记录 HandShaker Rust 的新增、变更、修复和验证结果。协议事实、架构契约和
历史实施材料分别见 [`docs/README.md`](docs/README.md)、
[`docs/architecture/architecture.md`](docs/architecture/architecture.md) 和
[`docs/archive/README.md`](docs/archive/README.md)。

## Unreleased

### Planned

- 将 `device discover` 的 Wi-Fi mDNS 发现迁入 Application，保持 CLI 输出、warnings 和退出码兼容。
- 建立 Linux/Windows CI，验证 FFI 产物、Header/ABI 和最小消费者。
- 为 GTK/.NET 补充同一 Application/FFI 契约的最小 smoke/示例；完整 GUI 应用不在本仓库交付范围内。
- 持续回归 Runtime/Session/Transfer 的取消、关闭、事件终态和多 Runtime 行为。

## 0.7.5 — 2026-09

### Fixed

- 修复缩略图混合缓存批次：只为缺失项回源，避免零字节或非普通文件被误判为缓存命中。
- 将设备身份和媒体 metadata revision 纳入缓存键，避免同路径内容更新后继续使用旧缩略图。
- 为 FFI 缩略图结果提供逐项稳定失败原因。

### Added

- Swift SDK 增加 `thumbnailStream`，支持小批次、有界并发和本地缓存路径渐进产出。

### Compatibility

- Application API `1.0.0`、FFI ABI `1.5.0` 和 CLI JSON schema `1` 保持不变。

## 0.7.4 — 2026-09

### Fixed

- 同步崩溃恢复按当前 profile 的 `local_root` 校验 journal，并在恢复 ledger 后重新计算 diff 与本地冲突。
- 修复 Swift 媒体分页请求中 `limit`/`cursor` 被作为字面量发送的问题。
- 固定 Apple 构建最低部署目标，避免构建机 SDK 版本污染 vendored libusb 产物。

### Compatibility

- Application API `1.0.0`、FFI ABI `1.5.0` 和 CLI JSON schema `1` 保持不变。

## 0.7.3 — 2026-08

### Added

- 完成 Application 业务闭环：设备发现 warnings、稳定设备身份、信任、文件预检与执行计划、同步
  plan/run/status/stop/watch，以及 CLI 业务迁移。
- 将 Core typed events 映射为 Application 稳定事件，补齐 ConnectionLost、Transfer、媒体、剪贴板、
  远端文件和同步 watch 事件。
- FFI ABI 扩展至 `1.5.0`、52 个导出符号，覆盖文件元数据、照片同步、媒体增量合并、媒体分页、
  诊断和批量传输。
- 完成 Swift Package、静态 arm64+x86_64 XCFramework、C/Swift smoke 和 macOS CI。

### Changed

- Runtime/Session/Transfer 生命周期改为确定性关闭；取消、超时、连接丢失和 EventHub 终态具有稳定语义。
- 媒体库采用 metadata-only 分页，缩略图通过磁盘缓存按需加载。
- FFI JSON contract、destroy 并发约束、订阅容量和 wire log 行为正式记录。
- Application API v1 冻结为 `1.0.0`，后续破坏性变更必须递增 major。

### Fixed

- 完成审计中的 P0/P1/P2 问题修复，包括同步台账、后台任务注册、watch 对账、临时文件、Swift 事件模型、
  FFI 生命周期和构建产物自包含性。

### Validation

- Rust 测试、clippy、fmt、C/Swift smoke 和 macOS CI 通过；Swift 真机验收按环境选择性运行。

## 0.7.2 — 2026-08

### Added

- FFI ABI 升至 `1.2.0`，增加 `hs_create_directory` 和 `hs_ping`。

### Changed

- `fs rm/count`、clipboard 和 media 命令迁移到 Application；CLI 仅保留参数、确认、输出和兼容适配。
- 建立 FFI Header 生成、ABI 检查、Linux 构建脚本和 Apple 产物暂存流程。

## 0.7.1 — 2026-08

### Added

- FFI ABI 升至 `1.1.0`，增加下载、上传、取消、查询和列表传输任务接口。

### Changed

- CLI 连接统一使用 `HandShakerRuntime`；文件列表、路径检查和批量 push/pull 迁移到 Application。

## 0.7.0 — 2026-08

### Added

- 建立 Cargo Workspace：`handshaker-core`、`handshaker-application`、`handshaker-cli`、`handshaker-ffi`。
- 建立 Application Runtime、Session、Transfer、事件和 PublicError 模型。
- 建立手写 C ABI、Buffer 所有权、panic 隔离和基础 C/Swift smoke 测试。

### Changed

- CLI JSON 契约保持兼容，跨语言调用统一以 Application 为业务入口。

## 0.6.1 — 2026-08

### Added

- 增加 `handshaker batch` 长连接批量会话，复用单个 Session 顺序执行 stdin 命令并保持心跳。

## 0.6.0 — 2026-08

### Added

- 增加 USB AOA 传输通道，复用 ADB 裸握手、Session 和上层业务 API。
- 支持 accessory 枚举、AOA identification、USB 设备选择、文件传输、剪贴板和资源清理。

### Validation

- Smartisan OD103 真机完成 USB 连接、文件、MD5、剪贴板、重命名和清理验收。

## 0.5.0 — 2026-08

### Added

- 增加单向照片同步：plan、run、status、watch、原子同步台账和 FILE_CHANGE 增量处理。
- 增加冲突检测、失败聚合、幂等重跑和同步状态恢复。

## 0.4.1 — 2026-08

### Added

- 增加 `fs push/pull --dry-run`、一次性区间下载、`UPDATE_FILE_INFO` 和受控批内并发。

## 0.4.0 — 2026-08

### Added

- 增加 EXIF 拉取、本地解析、媒体库变更增量合并和批量/递归上传下载。
- CLI `fs push/pull` 支持多目标、递归、批量进度和失败聚合。

## 0.3.0 — 2026-08

### Added

- 增加照片、视频、音频媒体库、相册、缩略图和媒体变更事件。
- CLI 增加 `media photo|video|audio|thumbnail`。

## 0.2.0 — 2026-08

### Added

- 增加目录监控、设备主动推送、剪贴板变更、设备信息事件和 `watch` 命令。

## 0.1.5 — 2026-08

### Added

- 增加 Wi-Fi mDNS 发现、REQUEST_01/02 握手、持久化信任和 `trust list/remove/reset`。

## 0.1.0 — 2026-08

### Added

- 固化 ADB v0.1 基线：设备选择、握手、封帧、签名、文件操作、上传下载、剪贴板和 CLI JSON/JSONL 输出。
- 建立 SSP 协议文档、proto2 schema、真实设备抓包验证和基础 fake SSP 测试。
