# HandShaker_Rust 最新代码审计与 Swift 交付准备计划

> 审计仓库：`CashewTeam/HandShaker_Rust`
> 审计提交：`c71cf94cb654e8dcf15a5819d2176a94ff3bc132`
> Phase D 复核提交：`e01bc94`（本文件已按 Phase D 完成状态复核更新）
> Phase E 复核提交：`4c79380`（FFI 功能扩展 + photo sync 完成，ABI 1.4.0、50 个导出符号）
> Cargo Workspace 版本：`0.7.3`
> Application API 标称版本：`1.0.0-preview.1`（Phase D 后维持 preview，见 §5.2 P0-1）
> FFI Rust 实现版本：`1.2.0`（Header/文档/snapshot 已一致）
> GitHub Actions：`30790057088`
> 审计日期：2026-08-03（Phase D 复核同日）
> 审计重点：CLI 迁移、Application 层、FFI、Swift 前端交付准备

> **Phase D 复核结论（速览）**：原 P0 清单（§5.2）除 FFI 功能扩展外已全部关闭；
> CLI 迁移仅剩 `device discover` 直连 core；`session_client()` 过渡入口已删除；
> Core 事件已完整桥接；传输进度/终态/取消语义已闭环；真机 ADB 验收通过
> （基础 + sync 首次/增量/watch，0 forward 残留）。**Phase E（FFI 功能扩展）
> 已完成**：ABI 1.4.0、50 个导出符号（文件 stat/count/move/delete、剪贴板、
> 信任、发现、监控、批量传输、媒体/缩略图/EXIF、诊断、照片同步）。剩余主要缺口
> 集中在 **Apple SDK 交付（Phase F/G）与 CI 接入**。

---

## 1. 审计结论

当前 M8 已经完成了最重要的架构转折：

- 单 Cargo package 已拆为 Workspace；
- `handshaker-core`、`handshaker-application`、`handshaker-cli`、`handshaker-ffi` 已建立；
- CLI 的全部命令已迁移到 Application（唯一例外：`device discover` 仍直连 core）；
- Application 已建立 Runtime、Session、Transfer、事件和公共错误模型；
- FFI 已建立稳定 C ABI 的基本设施，包括资源所有权、panic 隔离、JSON Buffer、Runtime 句柄、Session ID、Transfer ID 和事件轮询；
- C 与 Swift 冒烟代码已存在；
- Phase D 复核时在 macOS 14 ARM64 / Rust 1.97.1 下通过格式检查、277 项测试、Clippy `-D warnings` 和 release 构建，并通过真机 ADB 验收。

当前代码仍属于 **“Swift 集成原型可开始，正式后端交付尚未完成”** 的状态，
但相比审计基线（c71cf94）已完成应用层闭环，剩余缺口集中在 FFI 导出面与 Apple 打包。

建议将当前成熟度理解为：

| 部分 | 审计估计 | Phase D 复核 |
|---|---:|---|
| Workspace 与内部物理分层 | 90% | 95%（`session_client()` 已删除，边界收口） |
| CLI 向 Application 迁移 | 70% | 98%（仅 `device discover` 直连 core） |
| Application 业务覆盖 | 75% | 90%（发现诊断、稳定身份、信任、文件计划、SyncService 全落地） |
| Application v1 契约稳定性 | 55% | 75%（明确 `1.0.0-preview.1`，DTO/事件 fixture 补齐，仍为 preview） |
| FFI 基础设施 | 80% | 95%（ABI 单一事实来源、Header 校验、C/Swift smoke 本地全过） |
| FFI 功能覆盖 | 40% | 95%（50 个符号：文件/剪贴板/信任/发现/监控/批量/媒体/诊断/照片同步全导出） |
| Apple 二进制与 Swift 包装交付 | 30% | 30%（无正式 XCFramework/Swift Package，Phase F/G） |
| Swift GUI 完整 MVP 后端准备 | 45% | 90%（后端与 FFI 能力齐备（含 sync），剩余 SDK 打包与 CI） |

### 最终判断

当前可以立即启动的 Swift 工作：

- 建立 `HandShakerCore` Swift 包装层；
- 验证 Runtime 创建和关闭；
- 设备枚举（含分通道 warnings）；
- ADB/Wi-Fi/USB 连接；
- Session 快照（含完整设备信息与 stable_id）；
- Ping；
- 目录列表；
- 新建目录；
- 单文件上传和下载（进度/终态事件齐备）；
- 传输状态轮询与取消；
- 事件订阅（Core 事件已桥接，含 ConnectionLost/Lagged/Closed）；
- 信任记录管理（Application 已具备，FFI 未导出）；
- Application/FFI 错误解码；
- 事件订阅线程模型验证。

当前不建议作为正式 Swift GUI 后端交付的原因（Phase E 复核后剩余）：

1. ~~FFI ABI 版本信息互相矛盾~~（已修复：Header/文档/snapshot 对齐 1.4.0，`scripts/generate-ffi-header.sh` 校验 50 个符号）；
2. ~~Application 的 Runtime/Session/Transfer 生命周期不够确定~~（已修复：确定性关闭、有界 join、孤儿任务消除）；
3. ~~传输进度没有完整进入事件流~~（已修复：progress/total/节流/终态无条件发布，批量任务带 item 进度）；
4. ~~Core 主动推送事件没有桥接到 Application~~（已修复：事件桥 + ConnectionLost/Clipboard/Media/RemoteFileChanged/SyncWatchApplied）；
5. ~~FFI 缺少大量 GUI 必需功能~~（已修复：Phase E 导出 21 个新符号 + photo sync 6 个，50 个符号覆盖文件/剪贴板/信任/发现/监控/批量/媒体/诊断/照片同步）；
6. ~~`state_dir` 和 `wire_log` 配置语义不真实~~（已修复：state_dir 全链路生效 + CLI `--state-dir`；wire_log 真实写入）；
7. ~~Application 仍公开 `HandShakerClient` 过渡入口~~（已删除，742f183）；
8. Swift 交付物仍是宿主架构的 `.a/.dylib`，没有正式 XCFramework；
9. CI 没有运行 C/Swift smoke，也没有 Linux/Windows FFI 验证和发布 artifact。

---

# 2. CI 与代码基线

## 2.1 GitHub Actions 结果

Actions Run `30790057088` 的 `checks` job 成功完成：

- `cargo fmt -- --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo build --release`

执行环境：

- macOS 14.8.7
- ARM64 runner
- Rust 1.97.1

测试统计：

| 模块 | 审计基线（c71cf94） | Phase D 复核（e01bc94） |
|---|---:|---:|
| `handshaker-core` | 120 | 124 |
| `handshaker-application` | 23 | 96 |
| CLI binary 单元测试 | 22 | 23 |
| CLI 集成测试 | 12 | 12 |
| `handshaker-ffi` | 19 | 21 |
| localization | 1 | 1 |
| 合计 | 197 | 277 |

Phase D 复核时的本地验证（与 CI job 相同命令）：

- `cargo fmt -- --check`、`cargo test`（277 项）、`cargo clippy --all-targets --all-features -- -D warnings`、`cargo build --release` 全部通过；
- `scripts/generate-ffi-header.sh`：ABI 23 个导出符号，Header/snapshot 同步；
- `scripts/run-ffi-smoke-tests.sh`：FFI smoke、C smoke、Swift smoke 全部通过；
- 真机 ADB 验收（`scripts/phase-d-acceptance.sh`，设备 Smartisan U2 Pro）：基础操作 + sync 首次/增量/watch + SIGINT 清理，0 forward 残留。

该 CI 能证明：

- 当前 macOS ARM64 上 Workspace 可编译；
- 已有测试没有回归；
- Clippy 告警已清零；
- release 模式可构建。

该 CI 尚不能证明（Phase D 后仍成立）：

- C Header 与 Rust 函数签名完全一致（本地脚本校验，CI job 未接入）；
- Swift smoke 可以实际编译运行（本地通过，CI job 未接入）；
- FFI 成功连接假设备或真机（FFI smoke 仍只覆盖错误/空路径）；
- x86_64 macOS 可用；
- Linux FFI 可用；
- Windows DLL/PInvoke 可用；
- 静态库在 Xcode 工程中可链接；
- dylib 能正确嵌入、签名和公证；
- App Sandbox 环境下状态目录、ADB、USB 和文件访问可用。

Actions Run 没有上传 artifact，因此当前不能把 CI 结果直接作为 Swift 维护者可下载的 SDK 交付。

---

# 3. Workspace 与内部分层审计

## 3.1 已完成部分

根 Workspace 已包含：

```text
crates/
├── handshaker-core
├── handshaker-application
├── handshaker-cli
└── handshaker-ffi
```

依赖主方向已经形成：

```text
handshaker-core
        ↑
handshaker-application
        ↑
   ┌────┴────┐
 CLI         FFI
```

这是正确方向。

Core 中仍保留：

- Transport；
- SSP framing；
- protobuf；
- 握手；
- Session；
- 文件、剪贴板、媒体和同步底层逻辑；
- 测试 fake；
- 原始强类型事件；
- 取消和传输实现。

Application 中已经建立：

- `HandShakerRuntime`；
- `RuntimeConfig`；
- Device/Session/File/Clipboard/Media DTO；
- `TransferRegistry`；
- `EventHub`；
- `PublicError`；
- 文件服务；
- 剪贴板服务；
- 媒体快照服务；
- 上传下载任务；
- 批量传输。

FFI 只依赖 Application，没有直接依赖 Core，这是正确的。

## 3.2 尚未完成的边界收口

> **Phase D 复核**：本节原列的三个问题已全部关闭——
> `session_client()` 已删除、`AppSession` 不再持有 Core client、
> `docs/application-api-v1.md` 已与代码同步。仅存 `device discover`
> 命令直连 core（见 §4.2）。

### ~~`session_client()` 仍公开 Core 类型~~（已删除，742f183）

Application 曾公开：

```rust
pub async fn session_client(
    &self,
    session_id: SessionId,
) -> AppResult<Arc<HandShakerClient>>
```

该过渡入口已随 CLI 最后一个调用点（watch/sync/shell）迁移完成而删除。
`AppSession` 现在只持有 `runtime: Arc<HandShakerRuntime>` 与
`session_id: SessionId`，CLI 全部命令经 Application 服务方法执行
（shell/batch 的 `cd` 与欢迎横幅取 `SessionSnapshot`）。

### CLI 依赖

CLI 的 `Cargo.toml` 仍直接依赖 `handshaker-core`（用于输出层类型与
`device discover` 命令），但**不再持有 Core client**；`fs rm`/`fs count`
按输出契约保留在 CLI 编排层，业务执行已走 Application。

### Application 文档

`docs/application-api-v1.md` 已同步 Phase D 全部新 API（发现诊断、
信任、文件计划、SyncService、monitor_folder、SyncWatchApplied 事件），
并记录 `session_client()` 移除与 `RemoteFileChangeDto` 扩展。

---

# 4. CLI 迁移审计

## 4.1 已经通过 Application 的能力

### 设备与连接

- `device list`：通过 Application；
- 普通命令连接：通过 `HandShakerRuntime::connect`；
- Session Registry：已使用；
- 关闭普通连接：通过 Application disconnect。

### 文件系统

已经通过 Application：

- `fs ls`
- `fs stat`
- `fs exists`
- `fs mkdir`
- `fs mv`
- `fs count`
- `fs rm`
- `fs pull/push` 的批量实际执行

### 剪贴板

已经通过 Application：

- list/get
- set
- delete
- clear

### 媒体

已通过 Application：

- photo library
- video library
- audio library
- thumbnails
- EXIF

这些迁移证明 Application 已经不只是空包装层。

## 4.2 仍直接使用 Core 的能力

> **Phase D 复核**：以下列表在 c71cf94 审计时成立；Phase D（D1–D6 +
> CLI 迁移）后只剩两项：

### 设备

- `device discover`（Wi-Fi mDNS 发现）仍直接调用 Core
  `HandShakerClient::discover_wifi_devices`。

Application 已有等价的 `discover_devices()`（含分通道 warnings，
D1 交付），CLI 侧未切换是计划内的最小迁移（输出适配差异：CLI
`device discover` 的 JSON 形状与 Application DTO 不同，切换时需保持
旧 envelope）。

其余原直连项均已迁移：

- ~~`device info` / `device ping`~~ → Application（D5，snapshot 提供设备信息）；
- ~~trust list/remove/reset~~ → Application TrustService（D3/D5）；
- ~~pull/push 的 stat/exists/覆盖预检~~ → Application `plan_download`/
  `plan_upload`（D4/D5，冲突翻译保持旧错误类与退出码）；
- ~~sync status/plan/run/watch~~ → Application SyncService（D6 + 742f183）；
- ~~watch 独立 Core 连接与回调~~ → Application `connect()` 事件桥 +
  `monitor_folder`（62c7664）；
- ~~shell/batch 循环内的 Core client~~ → `AppSession` 不再持有 client，
  循环保留在 CLI 是设计决定（742f183）。

## 4.3 CLI 迁移结论

当前 CLI 迁移属于：

```text
物理拆分已完成
全部业务命令已迁移（唯一例外：device discover）
AppSession 不再持有 Core client
```

更准确的描述是：

> CLI 的常用命令、同步、监控、信任与预检已全部迁移到 Application；
> 交互循环（shell/batch）保留在 CLI 是设计决定，循环内业务命令走
> Application；`device discover` 与 `fs rm/count` 输出适配保留在 CLI。

---

# 5. Application 层审计

## 5.1 优点

### API 方向正确

`HandShakerRuntime` 作为统一入口是正确的。

Session 和 Transfer 使用稳定 ID：

```rust
SessionId(u64)
TransferId(u64)
```

而不是向 GUI 暴露 Core Session 或 Rust 指针。

### DTO 与 Core 分离

Application 为以下类型建立了 DTO：

- Device；
- Session；
- File；
- Clipboard；
- Media；
- Transfer；
- PublicError。

这使未来 Core 内部重构不必直接破坏 Swift JSON 契约。

### 路径规则集中

Application 已集中处理远端相对路径与 root path，避免 Swift、GTK、CLI 分别实现。

### 错误码模型已建立

已经有稳定字符串 token 和数值分区，为 GUI 判断错误种类建立了基础。

### 传输任务模型已形成

上传下载使用：

```text
start → TransferId → get/list/cancel
```

而不是让 FFI 长时间返回文件内容。

这适合 Swift 任务中心。

---

## 5.2 P0：正式交付前必须修复

> **Phase D 复核**：P0-1 至 P0-6 已全部关闭（见各条目）；剩余 P0 级
> 缺口已不在 Application，而在 FFI 导出面（Phase E）。

### ~~P0-1：Application v1 实际未冻结~~（已关闭）

`APPLICATION_API_VERSION` 已改为 `1.0.0-preview.1`（方案一），
`session_client()` 已删除，公开 DTO/事件 fixture 已补齐；
冻结前允许破坏性源码级修改，冻结后升 major。

### ~~P0-2：Session Registry 锁跨越网络 await~~（已关闭）

所有方法已统一为短临界区模式：

```rust
let session = self.session_handle(id).await?;
let client = session.client.clone(); // guard 已释放
client.operation().await
```

`session_handle` 在短临界区内 clone `Arc<ActiveSession>` 后立即释放
Registry 锁（含 `sync_ledger_status` 等无 session 路径）。

### ~~P0-3：disconnect/shutdown 生命周期不确定~~（已关闭）

`disconnect()`/`shutdown()` 已重构为确定性关闭：

1. 原子进入 `Disconnecting`（终态幂等）；
2. 从 Registry 移除（拒绝新工作）；
3. `cancel_for_session` 取消传输并**有界 join**（超时则 abort + await）；
4. 显式 `client.close()`（最后持有者；QUIT + transport cleanup）；
5. 有界等待事件桥退出；
6. 发布最终 `Closed` 事件；
7. 异常发布 `Warning`（可观察，不吞错）。

真机验证：SIGINT 中断 sync run/watch 后 0 forward 残留（e433d5a 修复
了 `run_sync_job` 嵌套 spawn 导致的孤儿任务——abort 外层不级联、client
永不 close 的问题）。

### ~~P0-4：`state_dir` 实际无效~~（已关闭）

`state_dir` 全链路真实生效：

- Core 公开 `StateStore::from_dir` 与 `connect_with_state`；
- 信任记录（D3）、host UUID、sync ledger（D6）均写入
  `<state_dir>/sync/<device_uuid>.json`；
- CLI 新增全局 `--state-dir` 参数（真机验收使用）；
- 多 Runtime 隔离与测试 tempdir 均有测试覆盖。

### ~~P0-5：`wire_log_utf8` 被忽略~~（已关闭）

`wire_log` 已真实写入 Core（`RuntimeConfig.wire_log` → WireLog 创建，
权限/0600 处理有测试）；FFI 配置 `wire_log_utf8` 与 CLI `--wire-log`
均生效，默认关闭。

### ~~P0-6：Core 主动事件未桥接~~（已关闭）

Application connect 已建立事件桥任务：Core typed events 映射为
`BackendEvent`（`ConnectionLost`/`SessionStateChanged`/
`ClipboardChanged`/`MediaChanged`/`RemoteFileChanged` 携带 DTO payload，
`SyncWatchApplied` 携带批次结果，未知事件安全 `Warning`，不泄露
protobuf）。`RemoteFileChangeDto` 含 `files`/`statuses` 完整元数据
（Phase D/D2 扩展）。

---

## 5.3 P1：Swift MVP 前应修复

> **Phase D 复核**：P1-1 至 P1-5 已关闭；P1-6 部分（RuntimeStarted
> 仍未在 `create()` 发布）。

### ~~P1-1：错误映射过于粗糙~~（已关闭）

CLI 错误映射已按 `PublicErrorCode` 分区（`app_error`，退出码语义保持）；
Application `from_core_error` 保留 Core `ErrorCode` 上下文，区分
not found/permission/already exists/no space/unauthorized/offline/
trust/connection lost/取消来源（UserCancelled/RemoteCancelled）等。
`detail` 不含密钥与文件正文。

### ~~P1-2：设备发现吞掉错误~~（已关闭）

Application `discover_devices()` 返回：

```rust
DeviceDiscoveryResult {
    devices: Vec<DeviceDescriptor>,
    warnings: Vec<PublicError>,
}
```

ADB 不可用/unauthorized、Wi-Fi mDNS 失败、USB 枚举失败分别报告
warning，不再静默吞错；`list_devices()` 保留为兼容包装。

### ~~P1-3：DeviceInfoDto 字段不完整~~（已关闭）

DTO 已补齐 `external_storage_path`/`disk_size`/`used_disk_size`/
`battery_percentage`/`phone_locked`（serde fixture + 真机输出验证）。

### ~~P1-4：Wi-Fi DeviceId 不稳定~~（已关闭）

`DeviceDescriptor` 新增 `stable_id`：连接后以 `phone:<uuid>` 回填并发布
identity 事件（D2），发现 endpoint 仅作临时身份；旧 JSON 反序列化兼容
（缺失字段 → None）。

### ~~P1-5：last_activity 没有真实更新~~（已关闭）

`last_activity_at_ms` 由 `AtomicU64` 维护，真实网络活动（请求、事件、
ping、sync）后更新，并有单元测试。

### P1-6：RuntimeStarted 等事件没有落实（部分）

`BackendEvent::RuntimeStarted` 仍在枚举中，但 `create()` 未发布
（与审计基线相同）；`DeviceAdded/Removed` 没有独立 discovery watcher。
建议在 Phase E/F 一并处理：发布 `RuntimeStarted`，或从枚举中移除并
升级 Application API minor。

---

# 6. Transfer 模型审计

## 6.1 当前可用能力

Swift 目前可以：

- start upload；
- start download；
- get transfer；
- list transfers；
- cancel transfer；
- 通过最终事件获知完成或失败；
- Rust 直接读写文件，不跨 FFI 传大文件。

这是正确基础。

## 6.2 主要问题

> **Phase D 复核**：本节原列问题已全部关闭（M8.1 Phase C + Phase D
> 真机验证），保留原描述作对照。

### ~~进度没有发布事件~~（已关闭）

`TransferSnapshot` 维护 `transferred_bytes`/`total_bytes`/started/
finished 时间；进度按 100ms/256KiB 节流发布 `TransferUpdated`
（真机实测 30MB 下载 108 个事件，约 10–20 次/秒），完成/失败/取消
无条件立即发布。

### ~~`total_bytes` 永远可能为空~~（已关闭）

`total_bytes` 由 Core progress 保存并进入快照与事件。

### ~~取消状态不完整~~（已关闭）

`cancel()` 触发 CancellationToken、设置终态（`finished_at_ms`）、
立即发布 `Cancelled`；本地/手机端取消经 `TransferCancelled`/
`RemoteCancelled` 区分；后台任务结果不能回写终态（单向状态机 +
终态不覆盖）；plan 批量执行接入取消 token 后返回 `Cancelled` 且
session 同步处理。

### ~~History 没有边界~~（已关闭）

RuntimeConfig 支持 `transfer_history_capacity`（默认 64）与
`transfer_history_ttl`，超限按容量/TTL 清理。

### ~~取消下载可能破坏 Session~~（已关闭）

下载取消导致的 Session 失效已同步处理：取消后 Session 不可用即发布
`Failed`/`ConnectionLost`，后续请求返回 `SessionClosed`；真机验收了
“传输中取消 → ConnectionLost → Session Failed → 清理无残留”。

### ~~disconnect 和 transfer 关系不清晰~~（已关闭）

disconnect 契约：取消该 Session 全部 transfer → 有界 join → 显式
QUIT → 发布每个任务终态与 Session `Closed`（§5.2 P0-3）。

---

# 7. FFI 完成度审计

## 7.1 FFI 基础设施评价

当前做得较好的部分：

- `rlib/staticlib/cdylib`；
- C ABI；
- 不透明 Runtime/Subscription handle；
- `SessionId/TransferId` 使用 u64；
- Rust 内存由 Rust 释放；
- `HsByteBuffer`；
- `HsCallResult`；
- JSON success/error；
- PublicError JSON；
- `catch_unwind`；
- NULL 检查；
- invalid UTF-8；
- invalid JSON；
- Event queue pull；
- 传输任务 ID；
- Swift/C smoke 源码。

基础设施可以继续沿用。

## 7.2 当前导出能力

### 已导出

- ABI version；
- Runtime create/shutdown/destroy；
- list devices；
- connect；
- disconnect；
- session snapshot；
- list files；
- create directory；
- ping；
- start upload/download；
- cancel/get/list transfers；
- subscribe/next/destroy events。

### ~~Application 已有但 FFI 未导出~~（Phase E 复核）

Phase E（ABI 1.3.0）后已全部导出：

- 文件：stat/exists（stat 的 optional 结果）/count/move/delete
  → `hs_stat_file`/`hs_count_files`/`hs_move_path`/`hs_delete_paths`；
- 剪贴板 → `hs_clipboard_list/set/delete/clear`；
- 媒体 → `hs_media_photo_library/video_library/audio_library/
  thumbnail/fetch_exif`（缩略图走磁盘 cache path）；
- 批量 → `hs_transfer_start_batch_download/upload`（后台任务 + item
  进度 + batch_result）；
- 信任 → `hs_trust_list/remove/reset`；发现 → `hs_discover_devices`；
  监控 → `hs_monitor_folder`；诊断 → `hs_runtime_diagnostics`。

### ~~Core/CLI 有但 Application 或 FFI 未完成~~（Phase E 复核）

Application 层已全部完成（Phase D）；FFI 导出面已补齐（Phase E）：

- ~~Wi-Fi discover diagnostics~~ → `discover_devices()` warnings（D1 + E3）；
- ~~trust list/remove/reset~~ → TrustService（D3）+ FFI（E3）；
- ~~folder monitor~~ → `monitor_folder()` 服务 + `RemoteFileChanged` 事件
  （62c7664）+ FFI（E 补充）；
- ~~Core typed event bridge~~ → 事件桥（Phase C/C1，含 ConnectionLost）；
- ~~sync plan/run/status/watch~~ → SyncService（D6，FFI 按计划后置）；
- ~~photo sync~~ → SyncService（D6，真机验收）；
- ~~monitor lifecycle~~ → `start_sync_watch/stop_sync_watch` 与 watch 任务（D6）；
- ~~Connection loss event~~ → 请求级检测 + 事件（Phase C/C5）。

仍缺（Application 与 FFI 均无）：

- update file info（Core 有，未上架 Application/FFI）；
- media incremental merge（Core 有，未上架 Application/FFI）；
- sync FFI（Application SyncService 已具备，按计划后置 minor 追加）。

## 7.3 ~~P0：ABI 版本不一致~~（已修复）

审计基线的矛盾（Rust 常量 1.2.0 vs Header 注释 1.1.0 vs
`docs/ffi-v1.md` 1.1.0）已修复：

- Header 注释、`docs/ffi-v1.md`、新增 `docs/ffi-abi-snapshot.md` 全部
  对齐 ABI 1.2.0；
- `scripts/generate-ffi-header.sh` 生成 Header 并校验 ABI 常量与
  snapshot；`scripts/check-ffi-abi.py` 校验符号/签名/ABI 版本注释；
- Swift smoke 同时检查 major/minor；
- C smoke 编译 Header 并链接。

单一事实来源仍建议落地（`ffi-api.toml` 生成 Header/文档/Swift
constants），当前靠脚本校验保证一致（Phase F 可选优化）。

## 7.4 JSON ABI 的可用边界

JSON 作为第一版复杂 DTO ABI 是合理的。

但需要注意：

- Media thumbnail 当前是 `Vec<u8>`；
- Serde JSON 会把它序列化成整数数组；
- 大批缩略图会产生极大 JSON 和重复拷贝；
- Swift 解码成本高；
- FFI Buffer 峰值内存高。

媒体 FFI 不应直接把全部 thumbnail bytes 放入大 JSON。

建议：

- 媒体列表不含二进制；
- thumbnail 单独请求；
- 返回单个/批量二进制 Buffer 描述；
- 或让 Rust 写入缓存文件并返回 URL/path；
- Swift 使用 NSCache/磁盘缓存；
- 限制批大小和单响应大小。

## 7.5 Handle 安全

当前 C API 使用 `void*`，Header 使用 `HsRuntime*`/`HsSubscription*`，在 ABI 上可工作。

但没有防止：

- use-after-free；
- double destroy；
- 跨 Runtime 误用 Session ID；
- destroy 与正在执行的调用并发。

这些属于 C ABI 调用方责任，但 Swift wrapper 必须封装：

- Actor/锁；
- 单次 shutdown；
- deinit destroy；
- 不暴露裸指针；
- 不允许 destroy 与 call 并发；
- Session/Transfer 对 Runtime 保持强引用。

---

# 8. Apple/Swift 交付审计

## 8.1 当前已有

- C Header；
- module map；
- `.a`；
- `.dylib`；
- macOS build script；
- Swift smoke；
- RAII Runtime 示例；
- Codable 解码示例；
- ABI major 检查。

## 8.2 当前不足

### 没有 XCFramework

`build-ffi-macos.sh` 只构建当前 host 架构并复制：

```text
libhandshaker_ffi.a
libhandshaker_ffi.dylib
header
modulemap
```

没有：

- arm64 + x86_64 合并；
- `xcodebuild -create-xcframework`；
- Swift Package binaryTarget；
- checksums；
- release zip；
- Debug symbols；
- licenses；
- code signing 策略。

对于现代 ARM64 macOS 内部开发，host `.a` 可以先用；正式交付应至少提供 arm64 XCFramework。

### Smoke 未进入 CI

仓库已有 `scripts/run-ffi-smoke-tests.sh`，但当前 Actions job没有执行它。

所以当前绿色 CI 并未验证：

- Header 编译；
- module map；
- C link；
- Swift import；
- Swift link；
- Swift runtime call。

### Smoke 只验证错误和空路径

Swift smoke 覆盖：

- Runtime；
- list devices 全关闭得到空数组；
- 不存在 Session 的传输错误；
- 不存在 Session 的 mkdir/ping 错误。

没有覆盖：

- connect 成功；
- session snapshot；
- list files 成功；
- transfer progress；
- transfer success；
- event success；
- cancellation；
- shutdown with active transfer。

需要 FFI 可用的 fake backend 或注入 transport。

### App Sandbox 适配缺失

正式 macOS GUI 需要决定：

- 是否启用 App Sandbox；
- ADB executable 从哪里获得；
- 用户选择 adb 路径还是内置；
- USB libusb entitlement/权限；
- 本地文件 security-scoped bookmark；
- 上传文件访问生命周期；
- 下载目录授权；
- state/trust 文件目录；
- wire log 文件选择；
- dylib 签名；
- Hardened Runtime；
- notarization。

当前 FFI 接口以 path string 接受本地文件路径，但 Swift 必须确保 security scoped access 在整个异步传输期间保持开启。

---

# 9. Swift 前端功能交付矩阵

状态定义：

- **可集成**：可以立即用于 Swift 原型；
- **需补强**：后端存在，但正式 GUI 使用前应修复；
- **未交付**：FFI 或 Application 缺失；
- **后置**：可不纳入第一版 GUI。

| 功能 | Core | Application | FFI | Swift 交付判断（Phase D 复核） |
|---|---:|---:|---:|---|
| Runtime 生命周期 | ✅ | ✅ | ✅ | 可集成（确定性关闭已验证） |
| ADB 枚举 | ✅ | ✅ | ✅ | 可集成（分通道 warnings） |
| Wi-Fi 发现 | ✅ | ✅ | ✅ | 可集成（stable_id 已回填） |
| USB 枚举 | ✅ | ✅ | ✅ | 需 macOS 权限验证 |
| ADB 连接 | ✅ | ✅ | ✅ | 可集成（真机通过） |
| Wi-Fi 信任连接 | ✅ | ✅ | ✅ | 可集成（state_dir 已生效） |
| USB AOA 连接 | ✅ | ✅ | ✅ | 需真机/Xcode App 验证 |
| Session 快照 | ✅ | ✅ | ✅ | 可集成 |
| 设备完整信息 | ✅ | ✅ | ✅ | 可集成（字段已补全） |
| Ping | ✅ | ✅ | ✅ | 可集成 |
| 文件列表 | ✅ | ✅ | ✅ | 可集成 |
| Stat/exists/count | ✅ | ✅ | ✅ | 可集成（ABI 1.3） |
| 新建目录 | ✅ | ✅ | ✅ | 可集成 |
| 重命名/移动 | ✅ | ✅ | ✅ | 可集成（ABI 1.3） |
| 删除/回收站 | ✅ | ✅ | ✅ | 可集成（ABI 1.3） |
| 单文件上传 | ✅ | ✅ | ✅ | 可集成（进度/终态齐备） |
| 单文件下载 | ✅ | ✅ | ✅ | 可集成（Session 失效语义正确） |
| 传输取消 | ✅ | ✅ | ✅ | 可集成（终态/事件/区分来源） |
| 批量/目录上传下载 | ✅ | ✅ | ✅ | 可集成（后台任务 + item 进度 + batch_result，ABI 1.3） |
| 剪贴板列表/写入/删除 | ✅ | ✅ | ✅ | 可集成（ABI 1.3） |
| 照片库 | ✅ | ✅ | ✅ | 可集成（ABI 1.3） |
| 视频库 | ✅ | ✅ | ✅ | 可集成（ABI 1.3） |
| 音频库 | ✅ | ✅ | ✅ | 可集成（ABI 1.3） |
| 缩略图 | ✅ | ✅ | ✅ | 磁盘 cache path（ABI 1.3，不经过 JSON 数字数组） |
| EXIF | ✅ | ✅ | ✅ | 可集成（ABI 1.3） |
| 文件主动变更 | ✅ | ✅ | ✅ | 事件已桥接（RemoteFileChanged 含 files/statuses） |
| 剪贴板主动变更 | ✅ | ✅ | ✅ | 事件已桥接（ClipboardChanged） |
| 媒体主动变更 | ✅ | ✅ | ✅ | 事件已桥接（MediaChanged） |
| 照片同步 | ✅ | ✅ | ✅ | 可集成（ABI 1.4：hs_sync_plan/start/status/stop/start_watch/stop_watch；后台运行 + status 轮询/事件；编排由调用方完成） |
| 信任记录管理 | ✅ | ✅ | ✅ | 可集成（ABI 1.3） |
| 目录监控 | ✅ | ✅ | ✅ | 可集成（ABI 1.3，事件流已交付） |
| CLI shell/batch | ✅ | 不适用 | 不适用 | GUI 不需要 |
| CLI fallback | ✅ | — | — | 建议保留诊断入口 |

---

# 10. 推荐后续开发计划

建议把后续工作定义为 **M8.1：Swift Delivery Readiness**，而不是继续称为简单 M8 收尾。

---

## Phase A：契约和文档止血

优先级：P0 —— **Phase D 复核：已全部完成**（A1 见 §7.3，A2 见 §5.2 P0-1，
A3 见 §3.2/§11；README 与迁移文档已同步 Phase D 状态）。

### A1. 修复 ABI 单一事实来源（已完成）

- Header 改为 1.2.0 ✅；
- `docs/ffi-v1.md` 改为 1.2.0 ✅；
- 更新已导出函数矩阵 ✅；
- Swift smoke 同时检查 major/minor ✅；
- Header sync 检查签名，而不只检查名称 ✅（`check-ffi-abi.py`）；
- CI 编译 C Header ✅（本地脚本，CI job 待接入）；
- 增加 ABI snapshot ✅（`docs/ffi-abi-snapshot.md`）。

### A2. 重新定义 Application 冻结状态（已完成，方案一）

`APPLICATION_API_VERSION = 1.0.0-preview.1`；`session_client()` 已移除；
DTO/error/event fixture 补齐；冻结后任何破坏升 major。

### A3. 更新 README 和迁移文档（已完成）

README 追加 Phase D 里程碑；`docs/m8-migration.md` §4/4.2 记录
CLI 行为变化与 watch/sync 契约变化；`docs/application-api-v1.md`
同步全部新 API 与 `session_client()` 移除。

---

## Phase B：Runtime 与并发模型修复

优先级：P0 —— **Phase D 复核：B1–B5 已全部完成**。

### ~~B1. 消除 Registry 锁跨 await~~（已完成）

统一短临界区模式（§5.2 P0-2），`session_handle` 返回 `Arc<ActiveSession>`。

### ~~B2. 设计确定性 Session 关闭~~（已完成）

`disconnect` 按 1–8 步确定性执行（§5.2 P0-3），包含状态 watch、
transfer 取消、有界 join、显式 QUIT、Closed 事件、Registry 移除。

### ~~B3. 修复 shutdown~~（已完成）

删除固定 sleep；单次执行、拒绝新操作、并行关闭全部 Session、
join 全部任务、关闭 EventHub、destroy 不静默遗留任务
（订阅者收 `{"closed":true}`）。

### ~~B4. 支持 caller-provided StateStore~~（已完成）

Core `StateStore::from_dir` + `connect_with_state`；Application
`state_dir` 覆盖信任/host UUID/sync ledger；CLI `--state-dir`；FFI
配置一致（§5.2 P0-4）。

### ~~B5. 处理 wire log~~（已完成）

正式支持（§5.2 P0-5），默认关闭，文档与 UI 提示危险。

---

## Phase C：事件与传输模型完成

优先级：P0/P1 —— **Phase D 复核：C1–C5 已全部完成**。

### ~~C1. Bridge Core EventSubscription~~（已完成）

事件桥任务：callbacks → Core receiver → BackendEvent 映射；Session
关闭时终止；未知事件安全 Warning；不泄露 protobuf（§5.2 P0-6）。

### ~~C2. 完成传输事件~~（已完成）

progress 写入 transferred/total，100ms/256KiB 节流，10–20 次/秒，
终态无条件发布（§6.2）。

### ~~C3. 完成取消语义~~（已完成）

finished_at_ms + 立即终态事件 + 后台结果不覆盖 + User/Remote 取消
区分 + 下载取消的 Session 同步（§6.2）。

### ~~C4. 有界任务历史~~（已完成）

`transfer_history_capacity`（默认 64）/`transfer_history_ttl`（§6.2）。

### ~~C5. 连接丢失事件~~（已完成）

请求级检测：Session `Failed` + `ConnectionLost` 事件 + 后续请求
`SessionClosed` 快速失败；传输中取消 → ConnectionLost 已验证。

---

## Phase D：Application 业务闭环

优先级：P1 —— **Phase D 复核：D1–D6 已全部完成**（本 Phase 名称即
`docs/HandShaker_Phase_D_Application_Closure_Plan.md`）。

### ~~D1. 设备发现结果带 warnings~~（已完成）

`DeviceDiscoveryResult { devices, warnings }` + per-transport 诊断
（§5.3 P1-2）。

### ~~D2. 完整 DeviceInfoDto~~（已完成）

5 个缺失字段补齐 + stable_id + identity 事件（§5.3 P1-3/P1-4）。

### ~~D3. 稳定 Wi-Fi identity~~（已完成）

`DeviceDescriptor.stable_id` = `phone:<uuid>`，连接后回填并发布
identity 事件（§5.3 P1-4）。

### ~~D4. TrustService~~（已完成）

Application `list_trust_records`/`remove_trust_record`/
`reset_wifi_trust`，state_dir 真实生效（D3 提交 752d567）。

### ~~D5. 文件操作统一预检~~（已完成）

`plan_download`/`plan_upload`（FileConflictKind 六类冲突）+
`execute_file_plan`（可取消、失败分类、transport 级失败标记连接丢失）。

### ~~D6. SyncService~~（已完成）

`plan_sync`/`start_sync`/`get_sync_status`/`stop_sync`/
`start_sync_watch`/`stop_sync_watch`/`last_sync_result`/
`sync_ledger_status`；ledger 路径由 state_dir 管理；watch 批次经
`SyncWatchApplied` 事件发布；真机首次/增量/watch 验收通过。

---

## Phase E：FFI 功能扩展

建议 ABI 版本：`1.3.0` 或 `1.4.0`——**已采用 1.3.0**。

> **Phase E 复核：E1–E6 已全部完成**（提交 0797c79/b82c46f + Application
> 3eecc6a；ABI 1.4.0、50 个导出符号；子代理 B/C 实现、父代理验证提交；
> photo sync FFI 后置追加（4c79380））。

### ~~E1. 文件 FFI~~（已完成）

`hs_stat_file`/`hs_count_files`/`hs_move_path`/`hs_delete_paths`
（JSON request/response；stat 缺省 "."；count 含 depth/exclusions；
delete 含 trash/sync 选项）。`exists` 由 stat 的 optional 结果表达。

### ~~E2. Clipboard FFI~~（已完成）

`hs_clipboard_list/set/delete/clear`。

### ~~E3. Trust FFI~~（已完成）

`hs_trust_list`/`hs_trust_remove`（`device_id`，自动 `phone:` 前缀归一）/
`hs_trust_reset`（`endpoint`+`expected_device_id`），并追加
`hs_discover_devices`（DeviceDiscoveryResult 带分通道 warnings）。

### ~~E4. Batch FFI~~（已完成）

`hs_transfer_start_batch_download/upload`：后台任务模型，进度/取消/结果
复用 `hs_transfer_get/list/cancel`；`TransferSnapshot` 扩展
`item_count/completed_items/failed_items/current_item/batch_result`
（serde 兼容）；Application `start_batch_download/upload` 复用
`execute_file_plan` 后台执行体（3eecc6a）。

### ~~E5. Media FFI~~（已完成）

`hs_media_photo_library/video_library/audio_library/fetch_exif`；
`hs_media_thumbnail` 采用方案 1（Rust 磁盘 cache）：bytes 写入
`<state_dir>/thumbnails/<kind>-<fnv1a64(path)>.thumb` 并返回
`cache_path`（已存在复用，不经过 JSON 数字数组）。

### ~~E6. Diagnostics FFI~~（已完成）

`hs_runtime_diagnostics`：ABI/Application API/crate/platform/arch/
adb 探测（失败不报错）/state_dir/wire_log/active sessions/active
transfers/capabilities。

### ~~E7. Photo-sync FFI~~（已完成，ABI 1.4.0，提交 4c79380）

`hs_sync_plan`/`hs_sync_start`（后台运行立即返回 profile_id）/
`hs_sync_status`/`hs_sync_stop`/`hs_sync_start_watch`/`hs_sync_stop_watch`
（50 个导出符号）。编排由调用方完成：plan → start → status 轮询或事件
（SyncWatchApplied/TransferUpdated/Warning）→ start_watch（需先跑完
一次全量，手机处于 SYNCING 状态）→ stop_watch/stop。

### Phase E 遗留与后置项（已评估，记录在案）

- **FFI 真机成功路径**：smoke 仍为错误/空路径（无设备可连）；真机成功
  路径验证建议在 Swift wrapper 阶段（Phase F）执行——届时按 §19 真机
  验收清单覆盖 ADB/Wi-Fi/USB、文件、媒体、批量、sync。
- **CI 接入**：ABI 校验（`scripts/generate-ffi-header.sh`）与 C/Swift
  smoke（`scripts/run-ffi-smoke-tests.sh`）仍是本地脚本；接入 CI 与
  Linux/Windows FFI 验证属 Phase F/G。
- **update file info / media incremental merge**：Core 已有
  （`media_merge.rs` 的 `apply_photo/apply_video/apply_audio` 与
  UpdateFileInfo 请求），Application/FFI 未上架——非 MVP 必需，按需
  minor 追加。
- **缩略图缓存**：state_dir 未配置时回退系统默认目录（macOS
  `~/Library/Application Support/handshaker`、Linux XDG/HOME，与 core
  一致）；沙箱宿主应显式配置 state_dir。缓存无 TTL（幂等覆盖 + 原子
  写 + 全命中跳过拉取）——有意设计，避免与宿主缓存策略冲突。

---

## Phase F：Swift SDK 交付

优先级：P1

建议新建 Swift Package：

```text
HandShakerCore/
├── Package.swift
├── Sources/
│   ├── CHandShakerFFI/
│   └── HandShakerCore/
├── Tests/
└── Artifacts/
    └── HandShakerFFI.xcframework
```

Swift 层建议：

```text
HandShakerRuntimeActor
DeviceService
DeviceSession
FileService
TransferCenter
ClipboardService
MediaService
EventStream
HandShakerError
```

关键规则：

- 所有 C 调用不在 MainActor；
- Runtime 用 actor 串行保护；
- RuntimeHandle 不暴露；
- Session 强引用 Runtime；
- Transfer 强引用 Runtime；
- `AsyncThrowingStream<EventEnvelope>` 包装 subscription_next；
- shutdown/destroy 单次；
- Codable 模型与 JSON fixture 测试；
- ABI 兼容检查；
- App 只依赖 Swift protocol，不依赖 C 类型。

### security-scoped URL

上传/下载开始前：

- Swift 获得 URL；
- `startAccessingSecurityScopedResource()`；
- 保持 token 到 Transfer 终态；
- 完成/失败/取消后停止访问。

不能只把路径传给 Rust 后立即释放权限。

---

## Phase G：Apple 构建和发布

### G1. XCFramework

至少构建：

```text
aarch64-apple-darwin
```

如果需要 Intel：

```text
x86_64-apple-darwin
```

生成静态 XCFramework，避免裸 dylib 嵌入复杂度。

### G2. CI

新增 Jobs：

#### macOS ARM64

- fmt；
- clippy；
- workspace tests；
- release；
- C smoke；
- Swift smoke；
- static link smoke；
- XCFramework；
- Swift Package tests。

#### Linux（当前阶段暂时不做，仅 macOS）

- workspace tests；
- FFI build；
- C smoke；
- exported symbols check。

#### Windows（当前阶段暂时不做，仅 macOS）

- workspace build/tests；
- DLL build；
- Header compile；
- 最小 C# P/Invoke smoke。

### G3. Artifact

Actions 上传：

```text
HandShakerFFI.xcframework.zip
headers.zip
checksums.txt
symbols/debug info
license notices
```

### G4. 真机验收

Swift wrapper 真机测试：

- ADB；
- Wi-Fi 首次信任；
- Wi-Fi 重连；
- USB AOA（断开连接后重连需要真人帮助）；
- list files；
- mkdir/move/delete；
- upload/download MD5；
- cancel transfer；
- clipboard；
- media snapshot；
- app quit with active transfer；
- device disconnect during transfer。

---

# 11. Agent 可执行任务拆分

> **Phase E 复核**：1–20 已完成（14–20 为 Phase E + sync FFI，见 §10）；21–35
> （Apple SDK、CI 与跨平台）仍待做。

## Swift Delivery P0 —— 已完成

1. ~~修复 FFI ABI 1.2 Header/文档不一致~~（§7.3，现 1.3.0）
2. ~~将 Application API 标记为 preview 或完成正式冻结~~（§5.2 P0-1）
3. ~~移除 Session Registry 锁跨 await~~（§5.2 P0-2）
4. ~~重构 disconnect/shutdown 为确定性生命周期~~（§5.2 P0-3）
5. ~~让 RuntimeConfig.state_dir 真正生效~~（§5.2 P0-4）
6. ~~让 wire_log 配置生效或从 ABI 移除~~（§5.2 P0-5）
7. ~~桥接 Core 事件到 Application EventHub~~（§5.2 P0-6）
8. ~~完善传输 progress/total/cancel/final events~~（§6.2）
9. ~~建立有界 Transfer history~~（§6.2）
10. ~~移除公开 `session_client()` 并完成 CLI 必要迁移~~（§3.2/§4）

## Swift MVP 功能 —— 已完成（Phase E）

11. ~~补完整 DeviceInfoDto~~（§5.3 P1-3）
12. ~~新增 DeviceDiscoveryResult warnings~~（§5.3 P1-2）
13. ~~Application TrustService~~（Phase D/D4）
14. ~~FFI stat/count/move/delete~~（E1，hs_stat_file/hs_count_files/hs_move_path/hs_delete_paths）
15. ~~FFI clipboard~~（E2，hs_clipboard_list/set/delete/clear）
16. ~~FFI media metadata~~（E5，photo/video/audio library + fetch_exif）
17. ~~设计并实现 thumbnail binary/cache API~~（E5，磁盘 cache path）
18. ~~FFI batch transfer task~~（E4，后台任务 + item 进度 + batch_result）
19. ~~FFI diagnostics~~（E6，hs_runtime_diagnostics）
20. ~~FFI photo sync~~（ABI 1.4，hs_sync_plan/start/status/stop/start_watch/
    stop_watch；后台运行 + 事件/轮询汇报，提交 4c79380）

## Apple SDK

20. **建立 Swift Package 封装**（未开始）
21. **实现 Runtime actor/RAII**（未开始）
22. **实现 Codable DTO 与 PublicError**（未开始）
23. **实现 Event AsyncThrowingStream**（未开始）
24. **实现 TransferCenter**（未开始）
25. **实现 security-scoped URL 生命周期**（未开始）
26. **构建静态 XCFramework**（未开始）
27. **CI 运行 C/Swift smoke**（未开始）
28. **上传可下载 artifact**（未开始）
29. **加入 fake-device FFI 成功路径测试**（未开始）
30. **进行 Swift 真机验收**（未开始）

## 后续跨平台（当前目标仅 macos）

31. **Linux GTK Rust 示例直接依赖 Application**（未开始）
32. **Windows cdylib 导出检查**（未开始）
33. **C# SafeHandle/PInvoke smoke**（未开始）
34. **Application 多平台路径与状态目录测试**（未开始）

---

# 12. Swift 正式交付 Definition of Done

只有满足以下条件，才建议把 Rust 后端标为“Swift GUI 可正式交付”。

> **Phase D 复核**：已勾选项为完成；剩余缺口集中在 FFI 导出面
> （Phase E）与 Apple 打包（Phase F/G）。

## 契约

- [x] ABI/Header/docs 版本一致（1.2.0，脚本校验）；
- [x] Application API 无计划删除的公开 Core 类型（`session_client()` 已移除）；
- [x] DTO/error/event fixture 完整（Phase D 补齐）；
- [ ] Swift wrapper 有兼容性检查（Phase F）。

## 生命周期

- [x] 无 Registry 锁跨网络 await；
- [x] disconnect 确定性关闭；
- [x] shutdown join 所有任务；
- [x] App 退出无残留连接（真机验证 0 forward 残留）；
- [x] Session 丢失可观察（ConnectionLost + Failed）；
- [x] 下载取消后的 Session 状态正确（真机验证）。

## 事件

- [x] Core typed events 已桥接；
- [x] 传输进度事件完整（节流 + 终态无条件）；
- [x] total bytes 可用；
- [x] 取消/完成/失败终态完整；
- [x] Lagged/Closed 可恢复或明确处理。

## 功能

- [x] 文件浏览、stat、mkdir、move、delete（Application + FFI 全导出，ABI 1.3）；
- [x] 上传、下载、取消（单文件 + 批量后台任务，item 进度与 batch_result 齐备）；
- [x] 剪贴板（FFI 已导出）；
- [x] 设备完整信息（DTO 全字段，FFI snapshot 已含）；
- [x] 媒体接口（photo/video/audio library + thumbnail cache path + EXIF，FFI 已导出）；
- [x] Wi-Fi trust 管理（FFI 已导出，trust_list/remove/reset）；
- [ ] 明确是否包含 sync（后端已具备 SyncService，FFI 未包装；需 Swift 决策）。

## Apple

- [x] state_dir 在 App Container 生效（配置真实生效 + CLI `--state-dir`）；
- [ ] security-scoped URL 流程验证（Phase F）；
- [ ] XCFramework（Phase G）；
- [ ] Swift Package（Phase F）；
- [ ] C/Swift smoke 进入 CI（本地全过，CI job 未接入）；
- [ ] ARM64 真机/设备验收（CLI 真机已过；Swift wrapper 层未验）；
- [ ] dylib/static linking 策略确定（Phase G）；
- [ ] 签名、公证、libusb/ADB 策略记录（Phase G）。

## CI

- [ ] macOS C/Swift 成功路径测试（本地脚本有，CI 未接入；fake device 路径缺）；
- [ ] release artifacts；
- [ ] ABI/header signature check（本地脚本有，CI 未接入）；
- [ ] 无设备和 fake device 两类测试（fake device FFI 测试未做）。

---

# 13. 建议的近期交付策略

不需要等待全部后端功能完成才启动 Swift。

推荐并行路线：

## Swift 前端现在开始

Swift 团队先完成：

- GUI 架构；
- BackendClient；
- MockBackendClient；
- CLIBackendClient 保底；
- FFI Runtime wrapper；
- 设备列表；
- Session；
- 文件列表；
- 任务中心；
- 空状态和错误状态。

FFI 当前只作为可切换实验后端：

```text
Mock
CLI
FFI Preview
```

## Rust 团队优先修 P0

先完成（Phase D 复核：1–7 已完成，剩余 8–10）：

1. ~~ABI 一致~~；
2. ~~Runtime 生命周期~~；
3. ~~事件和传输~~；
4. ~~state_dir~~；
5. ~~文件完整 FFI~~（FFI 只导出 list/mkdir/ping/传输；stat/move/delete 待 Phase E）；
6. ~~clipboard FFI~~（待 Phase E）；
7. ~~XCFramework/Swift Package~~（待 Phase F/G）；
8. FFI 功能闭环（stat/count/move/delete、剪贴板、媒体、信任、批量、sync、diagnostics）；
9. CI 接入 ABI 校验与 C/Swift smoke，上传 artifact；
10. fake-device FFI 成功路径测试。

## 正式切换

当上述完成后：

```text
FFIBackendClient 成为默认
CLIBackendClient 保留为诊断/回退
```

---

# 14. 最终评级

审计基线 `c71cf94` → Phase E 复核 `b82c46f`：

```text
Core 后端功能：成熟
CLI：成熟；Application 迁移已收口（仅 device discover 直连 core）
Application：架构正确，preview 明确（P0/P1 已关闭，fixture 补齐）
FFI：功能覆盖 95%（ABI 1.4.0、50 符号：文件/剪贴板/信任/发现/监控/批量/
      媒体/诊断/照片同步全导出；Header/snapshot 校验 + C/Swift smoke 本地全过）
Swift 集成：可以开始（后端与 FFI 能力齐备）
Swift 正式交付：暂不建议（待 Phase F/G：Swift Package、XCFramework、CI）
```

一句话结论：

> 项目已完成应用层业务闭环与 FFI 功能导出面（Phase D + Phase E：发现诊断、
> 稳定身份、信任、文件计划、SyncService、事件桥、传输语义、44 个 FFI 符号，
> 真机 ADB 验收通过）；下一阶段应集中完成 Apple SDK 打包与 CI 接入
> （Phase F/G），而不应继续扩张协议功能。
