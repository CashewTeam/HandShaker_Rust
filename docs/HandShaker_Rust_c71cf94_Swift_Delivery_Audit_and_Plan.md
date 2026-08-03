# HandShaker_Rust 最新代码审计与 Swift 交付准备计划

> 审计仓库：`CashewTeam/HandShaker_Rust`  
> 审计提交：`c71cf94cb654e8dcf15a5819d2176a94ff3bc132`  
> Cargo Workspace 版本：`0.7.3`  
> Application API 标称版本：`1.0.0`  
> FFI Rust 实现版本：`1.2.0`  
> GitHub Actions：`30790057088`  
> 审计日期：2026-08-03  
> 审计重点：CLI 迁移、Application 层、FFI、Swift 前端交付准备

---

## 1. 审计结论

当前 M8 已经完成了最重要的架构转折：

- 单 Cargo package 已拆为 Workspace；
- `handshaker-core`、`handshaker-application`、`handshaker-cli`、`handshaker-ffi` 已建立；
- CLI 的主要文件、剪贴板和媒体调用已开始经过 Application；
- Application 已建立 Runtime、Session、Transfer、事件和公共错误模型；
- FFI 已建立稳定 C ABI 的基本设施，包括资源所有权、panic 隔离、JSON Buffer、Runtime 句柄、Session ID、Transfer ID 和事件轮询；
- C 与 Swift 冒烟代码已存在；
- 当前提交在 macOS 14 ARM64 / Rust 1.97.1 下通过格式检查、197 项测试、Clippy `-D warnings` 和 release 构建。

但是，当前代码仍属于 **“Swift 集成原型可开始，正式后端交付尚未完成”** 的状态。

建议将当前成熟度理解为：

| 部分 | 审计估计 | 结论 |
|---|---:|---|
| Workspace 与内部物理分层 | 90% | 基本完成 |
| CLI 向 Application 迁移 | 70% | 仍是混合架构 |
| Application 业务覆盖 | 75% | API 较广，但生命周期、事件和错误语义未完全收口 |
| Application v1 契约稳定性 | 55% | 文档称已冻结，实际仍存在破坏性修改和 Core 泄漏 |
| FFI 基础设施 | 80% | 基础设计较扎实 |
| FFI 功能覆盖 | 40% | 只覆盖 GUI 所需功能的一部分 |
| Apple 二进制与 Swift 包装交付 | 30% | 有脚本和 smoke，但没有正式 XCFramework/Swift Package |
| Swift GUI 完整 MVP 后端准备 | 45% | 可做设备、目录和单文件传输原型，不能完整替代 CLI |

### 最终判断

当前可以立即启动的 Swift 工作：

- 建立 `HandShakerCore` Swift 包装层；
- 验证 Runtime 创建和关闭；
- 设备枚举；
- ADB/Wi-Fi/USB 连接；
- Session 快照；
- Ping；
- 目录列表；
- 新建目录；
- 单文件上传和下载；
- 传输状态轮询；
- Application/FFI 错误解码；
- 事件订阅线程模型验证。

当前不建议作为正式 Swift GUI 后端交付的原因：

1. FFI ABI 版本信息互相矛盾；
2. Application 的 Runtime/Session/Transfer 生命周期不够确定；
3. 传输进度没有完整进入事件流；
4. Core 主动推送事件没有桥接到 Application；
5. FFI 缺少大量 GUI 必需功能；
6. `state_dir` 和 `wire_log` 配置语义不真实；
7. Application 仍公开 `HandShakerClient` 过渡入口；
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

| 模块 | 测试数 |
|---|---:|
| `handshaker-core` | 120 |
| `handshaker-application` | 23 |
| CLI binary 单元测试 | 22 |
| CLI 集成测试 | 12 |
| `handshaker-ffi` | 19 |
| localization | 1 |
| 合计 | 197 |

该 CI 能证明：

- 当前 macOS ARM64 上 Workspace 可编译；
- 已有测试没有回归；
- Clippy 告警已清零；
- release 模式可构建。

该 CI 尚不能证明：

- C Header 与 Rust 函数签名完全一致；
- Swift smoke 可以实际编译运行；
- FFI 成功连接假设备或真机；
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

### `session_client()` 仍公开 Core 类型

Application 当前公开：

```rust
pub async fn session_client(
    &self,
    session_id: SessionId,
) -> AppResult<Arc<HandShakerClient>>
```

代码注释明确说明这是 CLI 迁移过渡入口，并且“会在迁移完成后移除”。

这与 Application 文档中的以下承诺冲突：

```text
Application 不暴露 HandShakerClient
Application 是唯一业务契约
```

这意味着 Application v1 目前还没有真正冻结。

### CLI 同时依赖 Core 与 Application

CLI 的 `Cargo.toml` 仍直接依赖：

```toml
handshaker-core
handshaker-application
```

CLI 的 `AppSession` 同时持有：

```rust
runtime: Arc<HandShakerRuntime>
session_id: SessionId
client: Arc<HandShakerClient>
```

这不是错误，但说明当前是迁移期架构，而不是最终结构。

### Application 文档已经滞后

`docs/application-api-v1.md` 只列出较早的最小 API，而代码已经加入：

- count/stat/create/move/delete；
- clipboard；
- media；
- transfers；
- batch transfers。

因此“冻结契约”的真实内容没有一个完全同步的权威文档。

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

### 设备

- `device info` 仍直接读取 `client.device_info()`；
- `device ping` 仍直接调用 `client.ping()`；
- `device discover` 仍直接调用 Core；
- trust list/remove/reset 仍直接调用 Core。

其中 Application 已经有 `ping()`，但 CLI 尚未切换，说明迁移没有完全收尾。

### 文件传输编排

`fs pull/push` 的实际批量执行已经进入 Application，但 CLI 仍用 Core 做：

- `client.stat()`；
- `client.file_exists()`；
- 单文件/目录模式判断；
- 目标存在性检查。

这些业务判断的一部分最终应进入 Application，特别是未来 Swift GUI 也会需要的：

- 是否为目录；
- 是否需要递归；
- 覆盖冲突；
- 远端目标存在性；
- 批量任务预检。

否则 Swift 仍需要复制 CLI 的编排。

### Sync

以下仍在 CLI/Core：

- sync status；
- sync plan；
- sync run；
- sync watch；
- 台账管理；
- 冲突分析；
- 监控注册和事件处理。

如果 Swift 第一版不包含照片同步，可以后置；若 GUI 计划复刻原 HandShaker 自动照片同步，则这是正式交付阻塞项。

### Watch 和主动推送

`watch` 使用独立 Core 连接，并显式打开所有 callbacks。

这说明 Application 的普通 `connect()` 没有承担完整的推送事件桥接。

### Shell 和 Batch

Shell/Batch 作为 CLI 交互循环保留在 CLI 是合理的，但循环内部的每个业务命令最终仍应调用 Application，而不是要求持有 Core client。

## 4.3 CLI 迁移结论

当前 CLI 迁移属于：

```text
物理拆分已完成
主要静态业务方法已迁移
长连接、推送、信任、同步和部分编排仍依赖 Core
```

建议不要把“CLI 全量迁移完成”作为当前状态描述。

更准确的描述是：

> CLI 的常用文件、剪贴板和媒体用例已迁移到 Application；交互层、同步、监控、信任和若干预检仍处于过渡状态。

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

### P0-1：Application v1 实际未冻结

当前提交把：

```rust
BackendEvent::SessionStateChanged(SessionSnapshot)
```

改为：

```rust
BackendEvent::SessionStateChanged(Box<SessionSnapshot>)
```

虽然 JSON 不变，但这是 Rust Application API 的源码级破坏性修改。

`APPLICATION_API_VERSION` 仍是 `1.0.0`。

同时公开的 `session_client()` 泄露 Core 类型，并计划删除。

因此当前 Application API 应改为：

```text
1.0.0-preview / provisional
```

或者先完成收口后再正式冻结 `1.0.0`。

不建议让 Swift 包装层现在把所有 DTO/事件永久固化为稳定正式版本。

### P0-2：Session Registry 锁跨越网络 await

`list_files`、`count_files`、`stat_file`、`create_directory`、`move_path`、`delete_paths` 等方法在持有：

```rust
tokio::sync::Mutex<HashMap<SessionId, ActiveSession>>
```

的 guard 时直接等待网络请求。

后果：

- 一个慢文件列表会阻塞整个 Runtime 的 Session Registry；
- 其他 Session 不能查询；
- disconnect/shutdown 可能等待该请求；
- 多设备支持被全局串行化；
- GUI 快速切换目录时更容易出现卡顿；
- 后续连接丢失时锁等待链更复杂。

正确做法：

```rust
let client = {
    let sessions = self.inner.sessions.lock().await;
    sessions.get(&id)?.client.clone()
}; // 立即释放锁

client.list_dir(...).await
```

所有网络 await 前都应释放 Registry 锁。

### P0-3：disconnect/shutdown 生命周期不确定

当前 `disconnect()`：

1. 从 Registry 移除 Session；
2. 尝试 `Arc::try_unwrap(client)`；
3. 如果传输任务仍持有 client，则直接返回成功；
4. 依赖最后一个 Arc Drop 时关闭 transport。

当前 `shutdown()`：

1. 发出取消；
2. 固定 sleep 50ms；
3. 尝试关闭 Session；
4. 如果仍有 Arc，则不能显式 close。

问题：

- `disconnect()` 返回成功时，物理连接可能仍在运行；
- 活动任务没有明确 join；
- 固定 50ms 不是生命周期保证；
- App 退出时可能未发送 QUIT；
- ADB forward、USB interface、Wi-Fi socket 清理时机不可预测；
- Swift 认为 Session 已关闭，但后台任务仍可能更新状态。

正式交付需要：

```text
SessionState: Ready → Disconnecting → Closed
cancel all session transfers
await tasks with bounded deadline
explicitly close client
remove session
publish final events
```

### P0-4：`state_dir` 实际无效

FFI 接受：

```json
{
  "state_dir_utf8": "..."
}
```

Application 的 RuntimeConfig 也保存 `state_dir`。

但 Core connect 仍使用默认的：

```rust
StateStore::discover()
```

因此调用者指定目录并没有控制信任记录和状态文件位置。

对 Swift macOS App 尤其重要：

- App Sandbox 需要把状态放到容器目录；
- GUI 需要明确备份/清理数据；
- 测试需要临时目录；
- 多个 Runtime 需要隔离状态；
- Wi-Fi derived key 不应落在不可控路径。

必须让 Application 创建和持有明确 `StateStore`，并传入 Core connect。

### P0-5：`wire_log_utf8` 被忽略

FFI 配置结构解析了 `wire_log_utf8`，但构造 RuntimeConfig 时固定：

```rust
wire_log: None
```

这是“API 接受但不生效”的危险行为。

应二选一：

- 正式支持；
- 从 FFI 配置中移除并返回未知字段/不支持错误。

### P0-6：Core 主动事件未桥接

Application EventHub 定义了：

- DeviceAdded/Updated/Removed；
- ClipboardChanged；
- MediaChanged；
- RemoteFileChanged；
- Warning；
- SessionStateChanged；
- TransferUpdated。

实际主要只发布：

- RuntimeStopping；
- Session Ready/Closed；
- Transfer terminal state。

普通 Application connect 没有：

- 启用完整 Core callbacks；
- 建立 Core EventSubscription 转发任务；
- 把文件、剪贴板、媒体和设备变更映射为 Application Event。

所以 FFI 的 `hs_subscribe_events()` 目前不是完整后端事件流。

Swift 不能依赖它实现：

- 文件目录实时刷新；
- 手机剪贴板更新；
- 媒体库增量刷新；
- 设备断开；
- Wi-Fi Session 丢失；
- watch 功能。

---

## 5.3 P1：Swift MVP 前应修复

### P1-1：错误映射过于粗糙

当前映射包括：

- 所有 `RemoteIo` → `RemotePathNotFound`；
- 所有 `LocalIo` → `LocalPathNotFound`；
- 所有 `Handshake` → `TrustRejected`；
- 所有 Timeout → `ConnectionLost`；
- 所有 Cancelled/Interrupted → `TransferCancelled`。

这会导致 GUI 无法区分：

- 远端权限不足；
- 文件已存在；
- 手机空间不足；
- 本地写入权限；
- 本地目标已存在；
- 首次 Wi-Fi 等待授权；
- 真正信任拒绝；
- 用户取消普通请求；
- 传输取消；
- ADB unauthorized/offline；
- USB 权限问题。

建议基于 Core `ErrorCode` 和 operation 映射，而不是只匹配 Error enum 大类。

### P1-2：设备发现吞掉错误

Application 的 `list_devices()`：

- ADB 错误被静默丢弃；
- Wi-Fi discovery 错误被静默丢弃；
- USB 错误会使整个请求失败。

结果：

```text
没有设备
ADB 未安装
ADB 启动失败
Wi-Fi mDNS 失败
```

可能都表现为空数组。

Swift 的设备页需要能够展示：

- 未安装 ADB；
- USB 权限不可用；
- Wi-Fi 发现失败；
- 当前确实没有设备。

建议返回：

```rust
DeviceDiscoveryResult {
    devices: Vec<DeviceDescriptor>,
    warnings: Vec<PublicError>,
}
```

或通过 Warning Event 报告分通道错误。

### P1-3：DeviceInfoDto 字段不完整

Core DeviceInfo 已有：

- external storage path；
- total disk size；
- used disk size；
- battery percentage；
- phone locked。

Application DTO 没有这些字段。

这些恰好是 GUI 设备详情、存储占用和状态栏需要的数据。

### P1-4：Wi-Fi DeviceId 不稳定

Application 文档称 Wi-Fi 应优先使用设备 UUID。

实际 discovery 生成：

```text
wifi:<address>:<dynamic-port>
```

HandShaker Wi-Fi 端口会动态变化，因此这个 ID 不适合作为：

- Swift 列表 diff identity；
- 最近设备；
- 自动重连；
- 用户偏好；
- 信任记录关联。

需要区分：

```text
DiscoveryEndpoint 临时地址
StableDeviceId 连接后 UUID
```

连接完成后应发布 Device identity reconciliation/update。

### P1-5：last_activity 没有真实更新

SessionSnapshot 存在 `last_activity_at_ms`，但多数请求执行后没有更新。

GUI 显示该字段会产生误导。

### P1-6：RuntimeStarted 等事件没有落实

Event enum 中有 RuntimeStarted，但 `create()` 没有发布。

DeviceAdded/Removed 也没有实际 discovery watcher。

需要清理“预留但看似可用”的事件，或者完整实现。

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

### 进度没有发布事件

Core 回调中提供：

```rust
TransferProgress {
    transferred,
    total
}
```

Application `set_progress()` 只更新 Registry：

```rust
snapshot.transferred_bytes = transferred
```

没有：

- 保存 total；
- 发布 `TransferUpdated`；
- 节流；
- 计算速度。

因此 `hs_subscription_next()` 收不到持续进度。

Swift 只能高频调用 `hs_transfer_get/list` 轮询。

### `total_bytes` 永远可能为空

Application 丢弃 Core progress 的 total。

这使 GUI 无法绘制确定进度条。

### 取消状态不完整

`cancel()`：

- 令牌 cancel；
- 状态设为 Cancelled；
- 不设置 `finished_at_ms`；
- 不立即发布事件。

### History 没有边界

代码注释说 bounded history，但实现是持续增长的 HashMap，没有容量或清理策略。

长时间运行 GUI 会积累任务。

### 取消下载可能破坏 Session

Core 已知下载取消会关闭 Session 以保证停止裸流接收。

Application 在任务取消后只把 Transfer 标记 Cancelled，没有同步：

- Session Failed/Closed；
- ConnectionLost；
- 建议重新连接。

Swift 可能继续使用一个实际上已经不可用的 Session。

### disconnect 和 transfer 关系不清晰

用户断开设备时：

- 是否取消所有属于该 Session 的任务；
- 是否等待；
- 是否允许后台完成；
- 是否发布每个任务终态；

目前没有正式契约。

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

### Application 已有但 FFI 未导出

文件：

- stat；
- exists；
- count；
- move/rename；
- delete。

剪贴板：

- list；
- set；
- delete；
- clear。

媒体：

- photo library；
- video library；
- audio library；
- thumbnails；
- EXIF。

批量：

- batch upload；
- batch download；
- recursive trees。

### Core/CLI 有但 Application 或 FFI 未完成

- Wi-Fi discover diagnostics；
- trust list/remove/reset；
- folder monitor；
- Core typed event bridge；
- sync plan/run/status/watch；
- update file info；
- photo sync；
- media incremental merge；
- monitor lifecycle；
- Connection loss event。

## 7.3 P0：ABI 版本不一致

Rust 实现：

```rust
ABI_VERSION_MAJOR = 1
ABI_VERSION_MINOR = 2
ABI_VERSION_PATCH = 0
```

C Header 顶部注释仍写：

```text
ABI version: 1.1.0
```

`docs/ffi-v1.md` 也仍以 1.1.0 为标题和主版本说明，并把 create directory 标为未导出。

这会造成：

- Swift wrapper 依据错误文档生成；
- SDK 包版本判断错误；
- 维护者无法确认 1.2 symbols；
- 未来兼容策略失去可信度。

`generate-ffi-header.sh` 只检查 symbol 名称是否存在，不检查：

- 函数签名；
- 参数类型；
- 返回类型；
- ABI 版本；
- Header 注释；
- docs。

因此没有捕获该问题。

必须建立单一事实来源，例如：

```text
ffi-api.toml / ffi_api.rs
    ↓
生成 header
生成 ABI version 文档
生成 Swift compatibility constants
```

至少应让 CI 编译 C smoke，并对：

```c
hs_abi_version_minor() == 2
```

做断言。

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

| 功能 | Core | Application | FFI | Swift 交付判断 |
|---|---:|---:|---:|---|
| Runtime 生命周期 | ✅ | ✅ | ✅ | 需补强 shutdown |
| ADB 枚举 | ✅ | ✅ | ✅ | 可集成，需错误诊断 |
| Wi-Fi 发现 | ✅ | ✅ | ✅ | 可集成，ID/错误需补强 |
| USB 枚举 | ✅ | ✅ | ✅ | 可集成，需 macOS 权限验证 |
| ADB 连接 | ✅ | ✅ | ✅ | 可集成 |
| Wi-Fi 信任连接 | ✅ | ✅ | ✅ | state_dir 阻塞正式交付 |
| USB AOA 连接 | ✅ | ✅ | ✅ | 需真机/Xcode App 验证 |
| Session 快照 | ✅ | ✅ | ✅ | 可集成 |
| 设备完整信息 | ✅ | 部分 | 部分 | 需补字段 |
| Ping | ✅ | ✅ | ✅ | 可集成 |
| 文件列表 | ✅ | ✅ | ✅ | 可集成，先修锁跨 await |
| Stat/exists/count | ✅ | ✅ | ❌ | 未交付 |
| 新建目录 | ✅ | ✅ | ✅ | 可集成 |
| 重命名/移动 | ✅ | ✅ | ❌ | 未交付 |
| 删除/回收站 | ✅ | ✅ | ❌ | 未交付 |
| 单文件上传 | ✅ | ✅ | ✅ | 需补进度/生命周期 |
| 单文件下载 | ✅ | ✅ | ✅ | 需补进度/Session 失效 |
| 传输取消 | ✅ | ✅ | ✅ | 需补事件和终态 |
| 批量/目录上传下载 | ✅ | ✅ | ❌ | 未交付 |
| 剪贴板列表/写入/删除 | ✅ | ✅ | ❌ | 未交付 |
| 照片库 | ✅ | ✅ | ❌ | 未交付 |
| 视频库 | ✅ | ✅ | ❌ | 未交付 |
| 音频库 | ✅ | ✅ | ❌ | 未交付 |
| 缩略图 | ✅ | ✅ | ❌ | 需先设计二进制 ABI |
| EXIF | ✅ | ✅ | ❌ | 未交付 |
| 文件主动变更 | ✅ | 预留 | 预留 | 未交付 |
| 剪贴板主动变更 | ✅ | 预留 | 预留 | 未交付 |
| 媒体主动变更 | ✅ | 预留 | 预留 | 未交付 |
| 照片同步 | ✅ | ❌ | ❌ | 后置或新阶段 |
| 信任记录管理 | ✅ | ❌ | ❌ | 正式 Wi-Fi 设置页需要 |
| CLI shell/batch | ✅ | 不适用 | 不适用 | GUI 不需要 |
| CLI fallback | ✅ | — | — | 建议保留诊断入口 |

---

# 10. 推荐后续开发计划

建议把后续工作定义为 **M8.1：Swift Delivery Readiness**，而不是继续称为简单 M8 收尾。

---

## Phase A：契约和文档止血

优先级：P0

### A1. 修复 ABI 单一事实来源

- Header 改为 1.2.0；
- `docs/ffi-v1.md` 改为 1.2.0；
- 更新已导出函数矩阵；
- Swift smoke 同时检查 major/minor；
- Header sync 检查签名，而不只检查名称；
- CI 编译 C Header；
- 增加 ABI snapshot。

验收：

```text
Rust constants
C Header
FFI docs
Swift constants
C smoke
```

全部一致。

### A2. 重新定义 Application 冻结状态

二选一：

方案一：

```text
APPLICATION_API_VERSION = 1.0.0-preview.1
```

完成收口后再冻结。

方案二：

保留 1.0.0，但：

- 移除 `session_client()`；
- 处理 `Box<SessionSnapshot>` 破坏性变更；
- 补完整文档；
- 对所有公开 DTO 做 fixture；
- 后续任何破坏必须升 major。

推荐方案一，因为目前尚无正式外部 SDK 消费者。

### A3. 更新 README 和迁移文档

修复：

- 旧 `src/` 目录描述；
- FFI 1.1/1.2 混乱；
- CLI “全量迁移”措辞；
- Application 真实功能矩阵；
- Swift 当前可用范围。

---

## Phase B：Runtime 与并发模型修复

优先级：P0

### B1. 消除 Registry 锁跨 await

对所有方法统一使用：

```rust
let session = self.session_handle(id).await?;
let client = session.client.clone();
// guard 已释放
client.operation().await
```

Session 内部可使用：

```rust
Arc<ActiveSession>
```

Registry 存：

```rust
HashMap<SessionId, Arc<ActiveSession>>
```

### B2. 设计确定性 Session 关闭

建议 ActiveSession 增加：

```rust
state: watch::Sender<SessionState>
transfers: HashSet<TransferId>
event_task: JoinHandle<()>
closing: CancellationToken
```

disconnect：

1. 原子变为 Disconnecting；
2. 拒绝新任务；
3. 取消属于 Session 的 transfers；
4. 等待任务结束，带 deadline；
5. 停止事件转发；
6. 显式 Core close；
7. 发布 Closed；
8. 从 Registry 移除。

### B3. 修复 shutdown

删除固定 50ms sleep。

Runtime shutdown：

- 只执行一次；
- 拒绝新操作；
- 并行关闭全部 Session；
- join 全部后台任务；
- 返回包含清理失败的结果；
- 关闭 EventHub；
- destroy 不应静默遗留任务。

### B4. 支持 caller-provided StateStore

修改 Core/Application API，让 connect 接受明确 StateStore 或 state path。

确保：

- `state_dir` 真正生效；
- trust 存储位于 App Container；
- 测试可用 tempdir；
- 多 Runtime 可隔离；
- FFI 配置与行为一致。

### B5. 处理 wire log

- 正式支持 `wire_log_utf8`；
- 或从 ABI 删除并升 ABI major；
- 推荐支持，但需要 Swift 设置页明确危险提示；
- 默认关闭。

---

## Phase C：事件与传输模型完成

优先级：P0/P1

### C1. Bridge Core EventSubscription

Application connect 时：

- 设置需要的 EventCallbacks；
- 创建一个 Core event receiver task；
- 映射 Core ClientEvent → BackendEvent；
- Session 关闭时终止；
- 未知事件映射安全 Warning/Unknown；
- 不泄露 protobuf。

### C2. 完成传输事件

progress callback：

- 写入 transferred；
- 写入 total；
- 按时间或字节阈值节流；
- 发布 TransferUpdated。

建议节流：

```text
不超过 10–20 次/秒
终态无条件发布
```

### C3. 完成取消语义

取消时：

- 设置 `finished_at_ms`；
- 立即发布 Cancelled；
- 最终后台任务结果不能覆盖终态；
- 区分 UserCancelled 与 RemoteCancelled；
- 下载取消导致 Session 关闭时发布 Session Failed/Closed。

### C4. 有界任务历史

RuntimeConfig 加：

```rust
transfer_history_capacity
transfer_history_ttl
```

完成任务按容量/时间清理。

### C5. 连接丢失事件

任何请求发现 Session 已关闭时：

- 更新 SessionState；
- 发布 ConnectionLost；
- 取消相关任务；
- 后续请求返回 SessionClosed。

---

## Phase D：Application 业务闭环

优先级：P1

### D1. 设备发现结果带 warnings

新增：

```rust
DeviceDiscoveryResult {
    devices,
    warnings,
}
```

或独立 diagnostics。

### D2. 完整 DeviceInfoDto

补充：

- external_storage_path；
- disk_size；
- used_disk_size；
- battery_percentage；
- phone_locked；
- connection transport；
- capabilities。

### D3. 稳定 Wi-Fi identity

建立：

```rust
DiscoveredEndpoint
StableDeviceIdentity
```

连接前使用 endpoint ID，连接后使用 phone UUID，并发 identity update 事件。

### D4. TrustService

Application 增加：

- list trust；
- remove trust；
- reset trust；
- trust status；
- clear all。

### D5. 文件操作统一预检

把可跨 UI 复用的逻辑移入 Application：

- stat；
- existence；
- file/folder classification；
- overwrite conflict；
- recursive requirement；
- destination collision；
- batch plan；
- dry-run plan model。

CLI 只负责参数和确认。

### D6. SyncService

若 Swift 第一版包含照片同步：

- SyncConfig DTO；
- plan；
- run task；
- status；
- conflict DTO；
- watch start/stop；
- sync events；
- cancellation；
- ledger path 由 Runtime state_dir 管理。

若第一版不包含，明确标记为第二阶段，不要半导出。

---

## Phase E：FFI 功能扩展

建议 ABI 版本：`1.3.0` 或 `1.4.0`

### E1. 文件 FFI

新增：

- `hs_stat_file`
- `hs_count_files`
- `hs_move_path`
- `hs_delete_paths`

`exists` 可由 stat 的 optional 结果表达，避免重复 API。

### E2. Clipboard FFI

新增：

- `hs_clipboard_list`
- `hs_clipboard_set`
- `hs_clipboard_delete`
- `hs_clipboard_clear`

### E3. Trust FFI

新增：

- `hs_trust_list`
- `hs_trust_remove`
- `hs_trust_reset`

### E4. Batch FFI

新增后台任务模型，不建议同步 block_on 整批完成：

```text
start_batch_upload/download
BatchTransferId
progress
cancel
result
```

可以统一扩展现有 TransferSnapshot，增加：

- item_count；
- completed_items；
- failed_items；
- current_item。

### E5. Media FFI

列表与元数据用 JSON。

缩略图单独设计：

方案优先级：

1. Rust 磁盘 cache + 返回 cache path；
2. 单图片 byte buffer；
3. 批量 buffer table；
4. 不推荐 JSON 数字数组；
5. base64 只适合小型调试接口。

### E6. Diagnostics FFI

增加：

```text
hs_runtime_diagnostics
```

返回：

- ABI；
- Application API；
- Rust crate；
- platform/arch；
- adb path；
- adb available/version；
- state dir；
- active sessions；
- active transfers；
- supported capabilities。

方便 Swift 设置页与错误反馈。

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

#### Linux

- workspace tests；
- FFI build；
- C smoke；
- exported symbols check。

#### Windows

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
- USB AOA；
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

建议按以下顺序建立 Issue/Agent Task。

## Swift Delivery P0

1. **修复 FFI ABI 1.2 Header/文档不一致**
2. **将 Application API 标记为 preview 或完成正式冻结**
3. **移除 Session Registry 锁跨 await**
4. **重构 disconnect/shutdown 为确定性生命周期**
5. **让 RuntimeConfig.state_dir 真正生效**
6. **让 wire_log 配置生效或从 ABI 移除**
7. **桥接 Core 事件到 Application EventHub**
8. **完善传输 progress/total/cancel/final events**
9. **建立有界 Transfer history**
10. **移除公开 `session_client()` 并完成 CLI 必要迁移**

## Swift MVP 功能

11. **补完整 DeviceInfoDto**
12. **新增 DeviceDiscoveryResult warnings**
13. **Application TrustService**
14. **FFI stat/count/move/delete**
15. **FFI clipboard**
16. **FFI media metadata**
17. **设计并实现 thumbnail binary/cache API**
18. **FFI batch transfer task**
19. **FFI diagnostics**

## Apple SDK

20. **建立 Swift Package 封装**
21. **实现 Runtime actor/RAII**
22. **实现 Codable DTO 与 PublicError**
23. **实现 Event AsyncThrowingStream**
24. **实现 TransferCenter**
25. **实现 security-scoped URL 生命周期**
26. **构建静态 XCFramework**
27. **CI 运行 C/Swift smoke**
28. **上传可下载 artifact**
29. **加入 fake-device FFI 成功路径测试**
30. **进行 Swift 真机验收**

## 后续跨平台

31. **Linux GTK Rust 示例直接依赖 Application**
32. **Windows cdylib 导出检查**
33. **C# SafeHandle/PInvoke smoke**
34. **Application 多平台路径与状态目录测试**

---

# 12. Swift 正式交付 Definition of Done

只有满足以下条件，才建议把 Rust 后端标为“Swift GUI 可正式交付”。

## 契约

- [ ] ABI/Header/docs 版本一致；
- [ ] Application API 无计划删除的公开 Core 类型；
- [ ] DTO/error/event fixture 完整；
- [ ] Swift wrapper 有兼容性检查。

## 生命周期

- [ ] 无 Registry 锁跨网络 await；
- [ ] disconnect 确定性关闭；
- [ ] shutdown join 所有任务；
- [ ] App 退出无残留连接；
- [ ] Session 丢失可观察；
- [ ] 下载取消后的 Session 状态正确。

## 事件

- [ ] Core typed events 已桥接；
- [ ] 传输进度事件完整；
- [ ] total bytes 可用；
- [ ] 取消/完成/失败终态完整；
- [ ] Lagged/Closed 可恢复或明确处理。

## 功能

- [ ] 文件浏览、stat、mkdir、move、delete；
- [ ] 上传、下载、取消；
- [ ] 剪贴板；
- [ ] 设备完整信息；
- [ ] MVP 所需媒体接口；
- [ ] Wi-Fi trust 管理；
- [ ] 明确是否包含 sync。

## Apple

- [ ] state_dir 在 App Container 生效；
- [ ] security-scoped URL 流程验证；
- [ ] XCFramework；
- [ ] Swift Package；
- [ ] C/Swift smoke 进入 CI；
- [ ] ARM64 真机/设备验收；
- [ ] dylib/static linking 策略确定；
- [ ] 签名、公证、libusb/ADB 策略记录。

## CI

- [ ] macOS C/Swift 成功路径测试；
- [ ] Linux build/test；
- [ ] Windows FFI build；
- [ ] release artifacts；
- [ ] ABI/header signature check；
- [ ] 无设备和 fake device 两类测试。

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

先完成：

1. ABI 一致；
2. Runtime 生命周期；
3. 事件和传输；
4. state_dir；
5. 文件完整 FFI；
6. clipboard FFI；
7. XCFramework/Swift Package。

## 正式切换

当上述完成后：

```text
FFIBackendClient 成为默认
CLIBackendClient 保留为诊断/回退
```

---

# 14. 最终评级

当前提交 `c71cf94`：

```text
Core 后端功能：成熟
CLI：成熟，但 Application 迁移尚未完全收口
Application：架构正确，仍属于 preview
FFI：基础设施良好，功能覆盖不足
Swift 集成：可以开始
Swift 正式交付：暂不建议
```

一句话结论：

> 当前项目已经跨过“能否做 Swift FFI”的门槛，但尚未跨过“能否让 Swift GUI 只依赖 FFI 并稳定发布”的门槛。下一阶段不应继续扩张协议功能，而应集中完成 Runtime 生命周期、事件桥接、传输语义、FFI 功能闭环和 Apple SDK 打包。
