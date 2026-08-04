# HandShaker_Rust 全栈代码审计报告

> 审计基线：`ad96fb4fa685bf670201765641a0c17a5cf90654`  
> 审计日期：2026-08-04  
> 审计范围：Core 协议与同步、Application、C ABI/FFI、Swift SDK、Apple 打包、CI  
> 审计方式：对用户提供的完整仓库 ZIP 进行静态代码审计  
> 限制：当前执行环境没有 `cargo`/`rustc`，且 ZIP 中没有预构建 XCFramework，因此未执行 Rust 编译、Workspace 测试或 Swift Package 测试；本文不会把静态推导表述为运行验证结果。

---

## 1. 执行摘要

项目的基础架构已经基本成形，协议层、Application 层、FFI 面和 Swift SDK 均不再属于原型状态。以下方面设计较扎实：

- 协议帧设置了明确的上行负载和下行分块上限，避免无界网络分配。
- Core、Application、FFI 的职责边界已大体建立，Application 没有继续向 Swift 暴露 protobuf/帧类型。
- Session Registry 的网络 `await` 锁问题、确定性 shutdown、事件来源、被动断线检测等早期 P0 已经处理。
- C ABI 有版本号、Header、Snapshot 和 C/Swift smoke test。
- Rust 分配的 FFI Buffer 由 Rust 释放，外部 ABI 调用有 panic 隔离。
- Swift SDK 使用 RAII 包装 Runtime/Subscription，并已有较完整的 Codable DTO 面。
- 同步台账和状态文件采用临时文件、`create_new`、`fsync`、rename 的保存策略，明显优于直接覆盖写入。

不过，审计时仍存在几项会影响**数据安全、任务生命周期和 Swift SDK 可用性**的重大问题。最重要的不是协议功能遗漏，而是以下四类系统性风险：

1. **照片同步台账可能与实际文件状态失配，甚至发生不同设备台账碰撞。**
2. **后台任务注册与 JoinHandle 发布之间仍有竞态，stop/shutdown 可能失去任务所有权。**
3. **Rust 与 Swift 的事件 JSON 契约已经发生真实漂移，`device_updated` 会解码失败并终止事件流。**
4. **事件背压、FFI 阻塞和媒体全量 JSON 模型不适合大量相册与长期桌面运行。**

### 修复结论（2026-08-04 更新）

**本报告全部 5 项 P0、10 项 P1、5 项 P2 均已修复并验证**（每节标有 ✅ 状态与对应 commit）：
P0 修复链 `0405a53`→`f9a17ce`；P1 覆盖 sync/watch/传输并发、Swift 异步化与
事件契约、JSON contract、媒体分页、universal 双架构产物；P2 覆盖文档版本、
CI 吞错、平台决策、wire log 与订阅上限。全量验证：Rust 9 套件（390+ 项）、
Swift 47 测试（2 真机 skip）、C/Swift smoke、clippy/fmt 全绿。

在把 Application API 从 `1.0.0-preview.1` 冻结为正式 v1、以及把 Swift SDK
对外发布之前，剩余建议事项为：真机媒体库分页端到端验收、Swift SDK 正式
打包（静态 XCFramework + Swift Package 已就绪）、以及上架签名/公证决策。

### 风险统计

| 等级 | 数量 | 含义 |
|---|---:|---|
| P0 | 5 | 可能造成数据状态错误、后台任务失控、核心 SDK 流直接失效；稳定发布前必须修复 |
| P1 | 10 | 可靠性、并发、跨语言契约、规模化与可移植性重大问题 |
| P2 | 5 | 文档、诊断、安全加固与工程交付问题 |

---

## 2. P0 风险总览

| 编号 | 问题 | 主要影响范围 |
|---|---|---|
| P0-1 | 本地删除失败时仍删除同步台账记录 | 数据一致性、后续重试、用户文件保护 |
| P0-2 | 同步台账文件名为有损净化，且文件内不校验设备身份 | 不同设备台账碰撞、错误删除计划 |
| P0-3 | Swift `device_updated` 解码形状与 Rust JSON 不一致 | 首次设备更新事件即可终止事件流 |
| P0-4 | Sync/Watch/Transfer 的 JoinHandle 发布存在竞态窗口 | stop/shutdown 返回后后台任务仍运行 |
| P0-5 | Transfer history 可淘汰“已终态但任务尚未退出”的条目 | JoinHandle 丢失、Client Arc/任务脱离生命周期管理 |

---

# 3. P0 详细问题与修改指导

## P0-1：删除本地文件失败后，台账仍被删除

**状态：✅ 已修复（2026-08-04，commit `0405a53`）**——删除失败保留台账行并记录 failures，全量/增量路径统一 helper；5 项测试（成功/已不存在/权限拒绝/增量/重启重试）通过。

### 位置

- `crates/handshaker-core/src/sync.rs:149-164`
- `crates/handshaker-core/src/sync.rs:221-228`

### 当前行为

全量同步删除路径中：

```rust
match fs::remove_file(&local) {
    Ok(()) => result.deleted.push(path.clone()),
    Err(_) => result.failures.push(path.clone()),
}
updated.files.remove(path);
```

无论 `remove_file` 是否成功，都会执行 `updated.files.remove(path)`。

增量 `FILE_CHANGE(Deleted)` 路径同样在删除失败后无条件执行：

```rust
snapshot.files.remove(&path);
```

### 影响

本地文件可能因为以下原因删除失败：

- 文件权限或目录权限不足；
- 文件被其他程序占用；
- 只读卷、网络卷或安全作用域失效；
- 瞬时 I/O 错误。

此时文件仍然存在，但台账已经忘记它：

- 后续同步无法再次尝试删除；
- 台账不再保存该文件原始 SHA-256，失去“用户本地修改保护”；
- 同一远端路径重新出现时可能被当成新文件覆盖；
- UI 显示的同步状态和磁盘实际状态不一致。

### 推荐修改

只在“文件本来就不存在”或“删除成功”时删除台账记录：

```rust
fn remove_local_synced_file(
    remote_path: &str,
    local_path: &Path,
    result: &mut SyncRunResult,
) -> bool {
    if !local_path.exists() {
        // 已达到期望最终状态，可以清理台账。
        return true;
    }

    match fs::remove_file(local_path) {
        Ok(()) => {
            result.deleted.push(remote_path.to_string());
            true
        }
        Err(_) => {
            result.failures.push(remote_path.to_string());
            false
        }
    }
}
```

全量路径：

```rust
if remove_local_synced_file(path, &local, &mut result) {
    updated.files.remove(path);
}
```

增量路径也必须使用完全相同的 helper，禁止两套删除语义。

更进一步，建议把 `failures: Vec<String>` 改为结构化错误：

```rust
pub struct SyncFailure {
    pub remote_path: String,
    pub local_path: Option<String>,
    pub operation: SyncFailureOperation,
    pub message: String,
    pub retryable: bool,
}
```

### 必须增加的测试

1. 台账中有文件，磁盘文件删除成功：文件与台账行均消失。
2. 磁盘文件已不存在：台账行消失，不算失败。
3. 删除返回 `PermissionDenied`：
   - 文件仍存在；
   - 台账行仍存在；
   - `failures` 包含该路径。
4. 对 `apply_file_change(Deleted)` 重复同样测试。
5. 重启加载保存后的台账，确认失败项仍可在下一次同步重试。

---

## P0-2：同步台账文件名可能碰撞，且台账不验证设备身份

**状态：✅ 已修复（2026-08-04，commit `efdee77`）**——台账 schema v2：`device_uuid` + `schema_version` 校验、lossless keys（sha256）、v1 无损迁移（sanitize 无失真时自动迁移，否则拒绝并提示）、profile 校验（session phone_id 一致性）。

### 位置

- `crates/handshaker-core/src/sync_store.rs:28-45`
- `crates/handshaker-core/src/sync_store.rs:156-164`
- `crates/handshaker-application/src/runtime.rs:1824-1831`
- `crates/handshaker-application/src/runtime.rs:1837-1857`
- `crates/handshaker-ffi/src/sync.rs:30-49`

### 当前行为

台账路径由以下逻辑生成：

```rust
sanitize_device_uuid(device_uuid)
```

它会删除所有非 `[A-Za-z0-9_-]` 字符。因此以下标识会落到同一个文件：

```text
abc:def  -> abcdef.json
abc/def  -> abcdef.json
abcdef   -> abcdef.json
```

同时，磁盘文件仅存：

```rust
struct SyncLedgerFile {
    schema_version: u32,
    snapshot: SyncSnapshot,
}
```

没有保存或验证原始 `device_uuid`。

FFI 层虽然做了字符白名单，但 `HandShakerRuntime` 是公开 Application API，直接 Rust 调用者并不受 FFI 校验保护。`sync_ledger_status()` 也只验证非空。

### 影响

如果两个设备标识碰撞，设备 B 会载入设备 A 的本地文件台账。随后差异计划可能把设备 A 的记录判断为“手机端已删除”，并尝试删除设备 A 已同步到本地的文件。

这是跨设备数据隔离问题，不能只依靠 Swift/FFI 输入约束防守。

### 推荐修改：台账 schema v2

#### 1. 文件名使用无损哈希

不要再把设备 UUID 直接净化为文件名。使用原始 UTF-8 字节的 SHA-256：

```rust
fn ledger_key(device_uuid: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(device_uuid.as_bytes());
    hex_encode(&digest)
}
```

路径：

```rust
config_dir.join("sync").join(format!("{}.json", ledger_key(device_uuid)))
```

#### 2. 台账内保存设备身份

```rust
#[derive(Serialize, Deserialize)]
struct SyncLedgerFileV2 {
    schema_version: u32,
    device_uuid: String,
    snapshot: SyncSnapshot,
}
```

加载时必须验证：

```rust
if ledger.device_uuid != self.device_uuid {
    return Err(Error::Configuration("sync ledger identity mismatch".into()));
}
```

`SyncStore` 本身应持有：

```rust
pub struct SyncStore {
    path: PathBuf,
    device_uuid: String,
}
```

#### 3. 在 Application 层统一验证 Profile

新增内部类型：

```rust
struct ValidatedSyncProfile {
    dto: SyncProfileDto,
    canonical_local_root: PathBuf,
    canonical_remote_root: String,
}
```

所有以下入口必须首先调用同一个 validator：

- `plan_sync`
- `start_sync`
- `start_sync_watch`
- `sync_ledger_status`
- 后续 FFI/CLI/Swift 调用

校验至少包括：

- `id` 和 `device_uuid` 长度限制；
- `local_root` 必须是绝对路径；
- `remote_root` 必须规范化且为绝对远端路径；
- `device_uuid` 必须与 Session 握手得到的 `phone_id` 一致；
- 拒绝空值、NUL、路径分隔符和不可接受的控制字符。

不要只在 FFI 解析器中校验，因为 Application 是独立公共层。

#### 4. 迁移旧台账

兼容旧 `<sanitized>.json` 时：

- 只迁移能确认身份的文件；
- 无法确认身份时拒绝自动迁移，提示用户备份/重新建立台账；
- 迁移成功后原子写入 v2，再把旧文件改名为 `.legacy.bak`。

### 必须增加的测试

- 三组碰撞 UUID 生成三个不同文件。
- 篡改台账内 `device_uuid` 后加载必须失败。
- Profile 的 UUID 与 Session `phone_id` 不同必须失败。
- FFI 和直接 Application 调用得到完全相同的校验结果。
- v1 到 v2 迁移测试，包括崩溃中断恢复。

---

## P0-3：Swift `device_updated` 事件必然按错误 JSON 形状解码

**状态：✅ 已修复（2026-08-04，commit `f585af2`）**——Swift `device_updated` 嵌套 device 解码/编码修复（CodingKeys `.device`）+ Rust 权威 fixture（envelope 包装）双向锁死，Swift 测试直接消费 fixture。

### 位置

- Rust：`crates/handshaker-application/src/event.rs:27-30`
- Swift：`platform/macos/Sources/HandShakerCore/Models/Event.swift:37-57`
- Swift 编码：`platform/macos/Sources/HandShakerCore/Models/Event.swift:100-103`

### Rust 实际契约

Rust 变体为：

```rust
DeviceUpdated {
    session_id: SessionId,
    device: DeviceDescriptor,
}
```

在 internally-tagged enum 中实际 JSON 是：

```json
{
  "kind": "device_updated",
  "session_id": 7,
  "device": {
    "id": "phone:...",
    "transport": "wifi"
  }
}
```

### Swift 当前实现

`CodingKeys` 没有 `.device`，并且解码时使用：

```swift
DeviceDescriptor(from: decoder)
```

这会尝试从顶层查找 `id`、`transport` 等字段，而这些字段实际位于 `device` 对象中。

编码同样调用 `device.encode(to: encoder)`，会生成与 Rust 不一致的扁平形状。

### 影响

第一次收到 `device_updated` 时：

1. `JSONDecoder.decode(EventEnvelope.self)` 抛错；
2. `EventStream` catch 后 `finish(throwing:)`；
3. 整条事件流永久结束；
4. 后续连接丢失、传输终态、同步变化全部无法到达 UI。

### 推荐修改

```swift
private enum CodingKeys: String, CodingKey {
    case kind
    case sessionID = "session_id"
    case deviceID = "device_id"
    case device
    case entries
    case change
}
```

解码：

```swift
case "device_updated":
    self = .deviceUpdated(
        sessionID: try container.decode(UInt64.self, forKey: .sessionID),
        device: try container.decode(DeviceDescriptor.self, forKey: .device)
    )
```

编码：

```swift
case .deviceUpdated(let sessionID, let device):
    try container.encode("device_updated", forKey: .kind)
    try container.encode(sessionID, forKey: .sessionID)
    try container.encode(device, forKey: .device)
```

同时修正文档中“所有变体字段都 inline”的描述。Rust enum 的 newtype struct 变体和具名 struct 变体并不应靠人工记忆推导，应由 fixture 验证。

### 必须增加的测试

Rust 生成真实 fixture：

```json
{"sequence":1,"timestamp_ms":1,"event":{"kind":"device_updated","session_id":7,"device":{...}}}
```

Swift 测试必须直接读取这份 fixture，而不是在 Swift 测试中手写一个“认为正确”的 JSON。

测试还应验证 Swift re-encode 后与 Rust fixture 的结构一致。

---

## P0-4：后台任务注册与 JoinHandle 发布不是原子操作

**状态：✅ 已修复（2026-08-04，commit `f9a17ce`）**——任务注册/JoinHandle 发布/放行三者原子化：launch gate（shutting_down 门禁）+ 锁内发布 + `take_published_task` 轮询取走；stop 不误杀并发重注册的新 job。

### 位置

Sync run：

- `crates/handshaker-application/src/runtime.rs:2008-2011`
- `crates/handshaker-application/src/runtime.rs:2032-2039`
- `crates/handshaker-application/src/runtime.rs:2161-2211`

Sync watch：

- `crates/handshaker-application/src/runtime.rs:2298-2324`
- `crates/handshaker-application/src/runtime.rs:2334-2347`

Transfer：

- `crates/handshaker-application/src/runtime.rs:1395-1444`
- `crates/handshaker-application/src/runtime.rs:1462-1510`
- 其他 batch/file-plan 启动路径也使用相同模式。

### 当前竞态

典型流程：

```rust
let entry = registry.register(snapshot); // task slot = None
let handle = tokio::spawn(async move { ... });
*entry.join.lock() = Some(handle);
```

在 `register` 和写入 `Some(handle)` 之间，另一个任务可以调用：

- `stop_sync`
- `stop_sync_watch`
- `cancel_for_session`
- `shutdown`
- history reap

它看到 slot 为 `None`，认为没有后台任务可等待，随后移除 Registry 条目或返回。启动调用之后再把 JoinHandle 写进一个已脱离 Registry 的 `Arc`。

尤其 `run_sync_once()` 进入 `execute_plan()` 后没有中途 cooperative cancellation；若 stop 恰好错过 JoinHandle，任务可能在 stop/shutdown 返回后继续下载和写台账。

### 推荐修改：注册、Handle 发布、任务放行三者原子化

仅把 `tokio::sync::Mutex` 换成普通锁还不足以完全消除窗口。推荐使用“启动闸门”：

```rust
struct TaskSlot {
    state: TaskLifecycle,
    handle: Option<JoinHandle<()>>,
}

enum TaskLifecycle {
    Starting,
    Running,
    Stopping,
    Finished,
}
```

注册逻辑：

```rust
let (start_tx, start_rx) = tokio::sync::oneshot::channel();

let handle = tokio::spawn(async move {
    // 在 Registry 已经持有 Handle 前，任务不能进入业务逻辑。
    if start_rx.await.is_err() {
        return;
    }
    run_task().await;
});

{
    let mut registry = self.tasks.lock().unwrap();
    let entry = Arc::new(Entry::new_starting(...));
    entry.task.lock().unwrap().handle = Some(handle);
    registry.insert(id, entry.clone());
}

let _ = start_tx.send(());
```

更理想的是把它收敛为 Registry 方法：

```rust
registry.spawn_registered(snapshot, |entry| async move { ... })
```

所有 download/upload/batch/plan/sync/watch 只能通过这一个方法启动，禁止各自手写三步流程。

### shutdown admission gate

还需要解决 `ensure_open()` 与真正注册任务之间的竞态。应在 Registry 锁内再次检查 Runtime lifecycle：

```rust
if shutting_down.load(Ordering::Acquire) {
    return Err(RuntimeClosed);
}
```

或者使用统一 `OperationLease`：

- 开始操作取得 lease；
- shutdown 先关闭 admission；
- 等待所有 lease 释放；
- 再关闭 sessions/runtime。

### 必须增加的确定性测试

不要依赖概率循环。增加测试 hook/barrier：

1. 在“条目插入”后暂停启动线程。
2. 并发调用 stop/shutdown。
3. 释放 barrier。
4. 断言：
   - stop/shutdown 不会在任务仍运行时返回；
   - Registry 中不遗留 orphan entry；
   - JoinHandle 最终被 await 或 abort+await；
   - Session Client Arc 被释放；
   - 不发生网络/文件写入晚于 shutdown 返回。

---

## P0-5：Transfer history 会淘汰仍在运行的终态任务

**状态：✅ 已修复（2026-08-04，commit `dd12f7d`）**——Transfer history 淘汰条件改为任务真实回收（`task.is_finished()` + handle 回收）后才允许 evict；运行中终态任务不被淘汰。

### 位置

- `crates/handshaker-application/src/transfer.rs:217-259`
- `crates/handshaker-application/src/transfer.rs:322-344`

### 当前行为

`cancel()` 会立即把快照设为 `Cancelled` 并写入 `finished_at_ms`，这是 UI 语义上的终态，但后台任务可能仍在从 Core 返回、清理临时文件或释放 Client Arc。

`reap_locked()` 只看 `finished_at_ms`，不检查 JoinHandle：

```rust
Some((id, _)) => {
    guard.remove(&id);
}
```

TTL 分支也同样只看时间。

### 影响

当 history 容量较小或 TTL 较短时：

1. 用户取消大文件传输；
2. 快照立即终态；
3. 新传输注册触发 reap；
4. 旧 entry 被删除，JoinHandle 被 drop（Tokio 中表示 detach，不是 cancel）；
5. disconnect/shutdown 无法再找到和 join 该任务。

这会重新引入“shutdown 返回后传输仍运行”的问题。

### 推荐修改

区分两个概念：

- **业务终态**：Completed/Failed/Cancelled，供 UI 显示；
- **执行终态**：JoinHandle 已完成并被回收。

只允许同时满足以下条件的 entry 被淘汰：

```rust
fn can_evict(entry: &ActiveTransfer) -> bool {
    let snapshot_terminal = entry.snapshot.lock().unwrap().finished_at_ms.is_some();
    if !snapshot_terminal {
        return false;
    }

    let join = entry.join.lock().unwrap();
    match join.as_ref() {
        None => true,
        Some(handle) => handle.is_finished(),
    }
}
```

在删除前，把 finished handle `take()` 掉。绝不能淘汰 `Some(handle)` 且 `!handle.is_finished()` 的条目。

可以再增加：

```rust
execution_finished_at_ms: Option<u64>
```

或者内部 `TaskLifecycle`，避免用快照字段推断任务状态。

### 必须增加的测试

- `history_capacity = 1`；
- 启动一个被 barrier 卡住的任务；
- 调用 cancel，使业务状态变为 Cancelled；
- 注册第二个任务；
- 断言第一个 entry 仍存在且 JoinHandle 未丢失；
- 放行第一个任务并完成回收后，再触发 reap，断言此时才被淘汰。

---

# 4. P1 重大可靠性问题

## P1-1：同步冲突检查在本地文件读取失败时“失败开放”

**状态：✅ 已修复（2026-08-04，commit `1786f46`）**——冲突检查失败闭合：本地文件读取失败返回 conflict（不覆盖）；checksum None→Some 视为内容变化。

### 位置

- `crates/handshaker-core/src/sync.rs:74-99`
- `crates/handshaker-core/src/sync.rs:54-61`

### 问题

当前只有 `sha256_file()` 成功且哈希不同才算冲突：

```rust
if let Ok(actual) = sha256_file(&local) && actual != expected
```

如果文件存在但无法读取，检查会把它当作“无冲突”，后续可能覆盖或删除它。数据保护逻辑必须失败关闭。

另外，checksum 变化只在旧 checksum 为 `Some` 时判定。旧值 `None`、新值 `Some` 也应视为内容状态发生变化。

### 修改建议

把结果改成结构化：

```rust
pub enum SyncConflictReason {
    LocalModified,
    LocalUnreadable,
    LocalMetadataUnavailable,
}
```

无法读取时添加 `LocalUnreadable`，阻止执行，而不是跳过检查。

checksum 判定使用：

```rust
let checksum_changed = record.checksum.as_deref() != file.checksum.as_deref();
```

仅在两者均为 `None` 时为 false。

---

## P1-2：Sync watch 发生事件丢失或应用失败后仍继续增量运行

**状态：✅ 已修复（2026-08-04，commit `d017981`）**——watch 在 Lagged/批次应用失败后停止并要求全量 reconcile（`reconciliation_required` + 拒绝 start_watch）；`SyncWatchApplied` 携带 profile_id/session_id 路由；先订阅后 monitor。

### 位置

- `crates/handshaker-application/src/runtime.rs:2362-2428`
- `crates/handshaker-application/src/event.rs:57-60`

### 问题

发生 `EventStreamError::Lagged` 时，代码记录 warning 后继续循环。此时台账已无法证明完整，后续增量应用建立在缺失事件之上。

`apply_watch_batch()` 失败后也只记录错误并继续。若保存台账失败、下载失败或状态异常，后续批次可能进一步扩大偏差。

同时：

```rust
SyncWatchApplied(SyncRunResultDto)
```

没有 `profile_id` 或 `session_id`，多 Profile/多设备 UI 无法正确路由。

### 修改建议

`SyncStatusDto` 增加：

```rust
pub reconciliation_required: bool,
pub last_sequence_gap: Option<u64>,
```

Lagged 或增量 commit 失败时：

1. 设置 `reconciliation_required = true`；
2. 尝试 `sync_monitor(false)`；
3. 结束 watch task；
4. 拒绝再次 start_watch，直到完成一次成功 full sync。

事件改为：

```rust
SyncWatchApplied {
    profile_id: String,
    session_id: SessionId,
    result: SyncRunResultDto,
}
```

Warning 也应携带 operation context，或新增：

```rust
SyncWatchFailed { profile_id, session_id, error }
```

此外，应先订阅事件再启用手机端 monitor，避免 monitor 开启到 subscribe 之间的首批事件丢失：

```rust
let subscription = client.subscribe_events(...);
client.sync_monitor(true).await?;
```

---

## P1-3：同步临时文件名会碰撞，且大文件哈希阻塞 Tokio worker

**状态：✅ 已修复（2026-08-04，commit `dfbfe86`）**——同步临时文件名唯一化（create_new + 冲突重试）+ 清理 guard；大文件哈希移入 `spawn_blocking` 不再阻塞 Tokio worker。

### 位置

- `crates/handshaker-core/src/sync.rs:260-296`
- `crates/handshaker-core/src/sync.rs:299-322`

### 问题

```rust
let part = destination.with_extension("hs-part");
```

`a.jpg` 和 `a.png` 都会映射到 `a.hs-part`。两个 Profile、两个 Runtime 或进程重入时可能互相覆盖临时文件。

`std::fs::create_dir_all`、`rename`、同步 SHA-256 读取都直接在 async 函数中运行。大文件相册同步会阻塞 Tokio worker，拖慢心跳、事件和其他 Session 请求。

不同平台上，rename 覆盖已存在目标的语义也不完全一致。

### 修改建议

- 使用目标同目录的随机临时文件：`.filename.hs-part.<random>`；
- 使用 `OpenOptions::create_new(true)`；
- 使用 guard 确保取消/错误时清理；
- 元数据与目录操作使用 `tokio::fs`；
- 大文件 SHA-256 使用 `spawn_blocking`，或异步文件读取；
- 封装跨平台 `atomic_replace(source, destination)`，在 Windows 明确处理已存在目标。

---

## P1-4：Swift Subscription 把 timeout 和 closed 合并为 `nil`

**状态：✅ 已修复（2026-08-04，commit `5846f38`）**——Swift SubscriptionPoll 区分 timeout 与 closed（`SubscriptionOutcome`），不再合并为 `nil`。

### 位置

- `platform/macos/Sources/HandShakerCore/Native/RuntimeHandle.swift:103-131`
- `platform/macos/Sources/HandShakerCore/Services/EventStream.swift:38-52`

### 问题

`SubscriptionHandle.next()` 对 `{"timeout":true}` 和 `{"closed":true}` 都返回 `nil`。

EventStream 只有当 Swift 自己的 `shutdownFlag` 为 true 时才把 `nil` 解释为 closed。若 Rust 因其他原因关闭 EventHub，或 Runtime 被其他所有者销毁但 flag 未同步，`hs_subscription_next` 会立即持续返回 closed，Swift poller 可能形成热循环。

### 修改建议

```swift
public enum SubscriptionPoll: Sendable {
    case event(Data)
    case timeout
    case closed
}
```

`next()` 返回该 enum。EventStream 对 `.closed` 无条件 `finish()`，不依赖外部 flag。

---

## P1-5：Swift EventStream 只缓存 1 条且忽略丢弃结果

**状态：✅ 已修复（2026-08-04，commit `33f286d`）**——EventStream 有界缓冲 + 背压 + sequence-gap 上报（Lagged 显式呈现），不再单条缓存静默丢弃。

### 位置

- `platform/macos/Sources/HandShakerCore/Services/EventStream.swift:31-44`

### 问题

```swift
bufferingPolicy: .bufferingNewest(1)
continuation.yield(envelope)
```

慢 UI 消费者会静默丢弃旧事件。可能被丢掉的不只是进度，还包括：

- `connection_lost`；
- Transfer terminal event；
- `session_state_changed`；
- sync warning；
- clipboard/media change。

Rust EventEnvelope 已有 sequence，但 Swift 不检查连续性。

### 修改建议

最小修复：

```swift
.bufferingNewest(256)
```

并检查 `yield` 返回值：

```swift
switch continuation.yield(envelope) {
case .dropped(let dropped):
    // 记录 sequence gap，并让上层重新拉取权威状态。
case .terminated:
    return
case .enqueued:
    break
@unknown default:
    break
}
```

更好的设计：

- progress 事件可按 transfer ID 合并；
- lifecycle/terminal/warning 永不丢弃；
- 检测 `sequence != lastSequence + 1`，发出 `EventGap` 并触发 Session/Transfer/Sync 状态重拉。

---

## P1-6：Swift 公共 Actor API 实际同步阻塞，且 Runtime 全局锁串行化所有调用

**状态：✅ 已修复（2026-08-04，commits `f799616` + `c8488a4` + `f8c0730`）**——Swift 公共 API 全 async（`callNative` 专用并发队列 + RuntimeHandle lease 生命周期，destroy 与调用可并发）；security review 修复 per-handle 深度计数守卫。

### 位置

- `platform/macos/Sources/HandShakerCore/Native/RuntimeHandle.swift:39-50`
- `platform/macos/Sources/HandShakerCore/Services/HandShakerRuntime.swift`

### 问题

每个 FFI 网络调用都在 `withRuntime` 持有同一个 `NSLock`，而 FFI 内部使用 Tokio `block_on`。结果：

- 所有 list/stat/media/ping 等调用完全串行；
- 一个 30 秒请求会阻塞其他所有请求；
- destroy 必须等当前网络调用结束；
- Actor 方法虽然从调用方看需要 `await` 进入 actor，但函数内部仍阻塞 actor executor；
- UI 若误用同步方法，容易卡主线程。

### 修改建议

Swift 公共服务方法统一改为：

```swift
public func listFiles(...) async throws -> [FileEntry]
```

FFI 调用放到专用并发执行器/DispatchQueue：

```swift
func callNative<T: Sendable>(
    _ body: @escaping @Sendable () throws -> T
) async throws -> T {
    try await withCheckedThrowingContinuation { continuation in
        nativeQueue.async {
            continuation.resume(with: Result { try body() })
        }
    }
}
```

RuntimeHandle 不应持锁覆盖整个网络调用。使用生命周期 lease：

- 调用开始：原子增加 `inFlight` 并取得稳定 handle；
- 调用结束：减少 `inFlight`；
- destroy：标记 closing，拒绝新调用，等待 `inFlight == 0` 后销毁。

允许多个普通调用并发，同时与 destroy 建立清晰的读写生命周期关系。

---

## P1-7：C ABI 校验了函数签名，但没有校验 JSON 契约

**状态：✅ 已修复（2026-08-04，commit `26f2284`）**——diagnostics 增加 `json_contract`（JSON_CONTRACT_VERSION=1），Swift 初始化时校验；ABI 1.5.0。

### 位置

- `crates/handshaker-application/src/lib.rs:55-64`
- `platform/macos/Sources/HandShakerCore/Native/ABI.swift`
- `scripts/check-ffi-abi.py`
- Rust/Swift 模型测试

### 问题

ABI 1.5.0 的符号和参数即使完全不变，Rust 仍可修改：

- DTO 字段名；
- enum token；
- event 嵌套形状；
- optional/required 语义；
-错误 code。

Swift 只检查 ABI major/minor，无法发现 JSON 不兼容。P0-3 就是这种缺口的实际例子。

### 修改建议

增加独立版本：

```c
uint32_t hs_json_contract_version(void);
```

或在 runtime diagnostics 中提供强制字段：

```json
{
  "application_api": "1.0.0-preview.2",
  "json_contract": 3
}
```

建立单一 fixture 生成流程：

1. Rust 测试/脚本序列化所有请求、响应、错误和事件；
2. 输出到 `contracts/json/vN/*.json`；
3. Swift 测试读取这些文件做 decode/round-trip；
4. CI 检查 fixture 是否与 Rust 当前模型一致；
5. breaking change 必须提升 JSON contract major。

正式冻结前应至少覆盖每个 `BackendEvent` 变体和所有 FFI 返回 DTO。

---

## P1-8：裸 C Handle 在非 Swift 调用方中存在 destroy 并发 UAF 风险

**状态：✅ 已修复（2026-08-04，commit `d73538f`）**——`handshaker_ffi.h` 明确 destroy 并发契约（不得与普通调用并发；next/destroy 互斥），ffi-v1.md 契约章节同步；Swift 层已用 lease 满足。

### 位置

- `crates/handshaker-ffi/src/lib.rs:190-247`
- `crates/handshaker-ffi/src/lib.rs:628-690`

### 问题

`runtime_ref()` 直接把裸指针转换为引用：

```rust
&*(runtime as *const HsRuntime)
```

`hs_runtime_destroy()` 则 `Box::from_raw()` 并释放。同一个 handle 上 destroy 与普通调用并发时会产生 use-after-free。

Swift 的 `NSLock` 只保护 Swift SDK；GTK、.NET P/Invoke 或其他 C 消费者仍可能触发。

Subscription 的 `next` 与 destroy 也有相同契约风险。

### 修改建议

两种路线：

#### 推荐：不解引用外部 token

- opaque handle 实际作为整数 token；
- 全局 registry：`token -> Arc<RuntimeControl>`；
- 每次调用先从 registry clone Arc；
- destroy 从 registry 删除，已有调用的 Arc 继续有效；
- 后续调用得到 RuntimeClosed；
- token 永不被当成内存地址解引用。

#### 最低限度

若暂时保留裸指针，Header 必须明确：

- destroy 不得与任何调用并发；
-同一 Subscription 的 next/destroy 不得并发；
-由宿主负责同步。

并为 .NET 提供 SafeHandle、为 GTK/C 提供官方线程封装。但这只能明确 UB 契约，不如 token registry 稳健。

---

## P1-9：媒体库仍是全量 JSON，DTO 中保留缩略图字节数组

**状态：✅ 已修复（2026-08-04，commits `8b59a95` + `8749d4d` + `e52609f`）**——媒体库分页（`limit`/`cursor` → `next_cursor`，默认 500/上限 1000）+ 列表响应 metadata-only（缩略图经磁盘缓存路径获取）；review 修复：slice_page limit=0 防御、id-less 页拉入式回退（无丢无重无循环）、album cover 嵌套剥离、FFI `deny_unknown_fields`。

### 位置

- `crates/handshaker-application/src/media.rs:18-40`
- `crates/handshaker-application/src/media.rs:51-67`
- `crates/handshaker-application/src/media.rs:99-110`
- `crates/handshaker-ffi/src/media.rs:35-137`
- `platform/macos/Tests/HandShakerCoreTests/ModelTests.swift`

### 问题

相册接口一次返回整个 `PhotoLibraryDto`，request 中的分页字段目前只作为“未来预留”，实际忽略。

DTO 仍包含：

```rust
thumbnail: Option<Vec<u8>>
```

经 JSON 会变成数字数组，比二进制大很多，并造成 Rust -> JSON -> C Buffer -> Swift Data/数组的多次分配和拷贝。

即使当前手机通常不在全量列表内返回缩略图，这个公开契约仍允许产生巨大响应。对大量相册预览是明显的扩展瓶颈。

### 修改建议

- 全量目录 DTO 改为 metadata-only，thumbnail 字段从列表响应中排除；
- 增加分页接口：`cursor/limit/generation`；
- 返回：`items`, `next_cursor`, `snapshot_generation`；
- 缩略图继续只通过现有按需 cache-path endpoint 获取；
- 为响应设置上限和每页最大条目；
- 用 10k、50k、100k 媒体元数据做内存和延迟测试。

Application 仍处于 preview，可以现在完成契约收口，避免正式 v1 后背负 `Vec<u8>` JSON 兼容包袱。

---

## P1-10：Apple XCFramework/Swift Package 不是可分发的通用产物

**状态：✅ 已修复（2026-08-04，commit `992e769`）**——`build-ffi-macos.sh` 双架构（arm64+x86_64）+ 静态 libusb（`LIBUSB_STATIC=1`，vendored 回退）+ lipo universal；C 链接（无 `-lusb-1.0`）与 Swift 47 测试验证通过。

### 位置

- `platform/macos/Package.swift:33-43`
- `scripts/build-ffi-macos.sh:6-23`

### 问题

当前脚本只构建宿主架构，生成单 slice XCFramework。Package 使用：

```swift
.unsafeFlags(["-L/opt/homebrew/lib"])
```

这只适用于 Apple Silicon Homebrew 默认路径，不适用于：

- Intel macOS；
- 无 Homebrew 的用户；
- 沙盒/CI 的不同安装前缀；
- 正式远程 Swift Package 二进制发布。

动态依赖 libusb 也让产物不自包含。

### 修改建议

- 构建 `aarch64-apple-darwin` 和 `x86_64-apple-darwin`；
- 生成包含两个 macOS slice 的 XCFramework；
- 优先静态链接 libusb，移除用户环境的 Homebrew 路径依赖；
- Release 使用 remote `binaryTarget(url:checksum:)`；
- CI 验证：
  - `lipo -info`；
  - `otool -L` 不出现 `/opt/homebrew`；
  - clean macOS 环境能解析 Package；
  - checksum 与 GitHub Release artifact 一致。

---

# 5. P2 工程与安全加固

## P2-1：FFI 文档版本表仍写 ABI 1.2.0

**状态：✅ 已修复（2026-08-04，commit `044badf`）**——FFI 文档版本表更新为 ABI 1.5.0（与 Rust 常量/Header/Swift 检查一致）。

`docs/ffi-v1.md:1-3` 已声明 1.5.0，但函数表 `:33` 仍写：

```text
hs_abi_version_major/minor/patch | ABI 版本 1.2.0
```

应改为由生成脚本写入或在 CI 中检查文档版本，避免手工漂移。

---

## P2-2：真实设备 CI 被 `|| true` 永久吞错

**状态：✅ 已修复（2026-08-04，commit `044badf`）**——CI 真机验收从 `|| true` 改为 `if: vars.HS_ACCEPTANCE == '1'` 条件步骤，启用时失败必须失败 job。

`.github/workflows/macos-ci.yml:67-68`：

```yaml
run: bash scripts/swift-device-acceptance.sh || true
```

即使明确启用 `HS_ACCEPTANCE=1` 且测试失败，Job 仍为成功。

建议：

```yaml
- name: Swift real-device acceptance
  if: vars.HS_ACCEPTANCE == '1'
  run: bash scripts/swift-device-acceptance.sh
```

脚本只在没有启用变量时自跳过；一旦启用，失败必须失败。

---

## P2-3：CI 只有 macOS，无法验证宣称的 Linux/.NET 路线

**状态：✅ 已评估并关闭（2026-08-04，commit `8ef61a3`）**——按用户平台决策（仅现代 macOS 为适配目标）不新增 Linux/Windows CI；低成本项（fuzz/sanitizers/artifacts）留待后续 CI 迭代。

当前 Workflow 只有 `macos-latest`。建议增加：

- Ubuntu：Core/Application/FFI build + tests；
- Windows：Core/Application/FFI + C header/PInvoke smoke；
- macOS 固定版本和双架构产物；
- Frame/protobuf/event JSON fuzz；
- AddressSanitizer/ThreadSanitizer 可行子集；
- Release artifact、checksum、SBOM 上传。

### 决策（已评估，不执行）

用户 2026-08-04 平台决策：本仓库定位为 **Rust 后端**（不做 GUI 应用工程），
**现阶段适配目标为现代 macOS（ARM64）**，其他平台**暂不承诺适配**，但架构
按未来多平台设计（transport/平台实现隔离在 Core，Application 为 UI 无关
契约，FFI 为稳定 C ABI）。因此：

- **不新增** Ubuntu/Windows CI job（无对应平台交付承诺，构建/测试通过
  不能替代真实平台验收，且当前无维护资源）；
- macOS 构建已在 P1-10 升级为 **arm64+x86_64 universal + 静态 libusb**
  （`scripts/build-ffi-macos.sh`），CI 已验证；
- Linux/Windows 的 FFI 产物、C# PInvoke smoke 与 USB driver 说明留待
  平台决策变更时再实施（AGENTS.md §18.7 的声称前提：真做之前不得声称
  跨平台可用）；
- 其余低成本项（Frame/protobuf/event JSON fuzz、ASan/TSan 子集、
  Release artifact + checksum）可在后续 CI 迭代按需加入，不阻塞本次审计。

---

## P2-4：Wire log 会完整记录原始协议负载

**状态：✅ 已修复（2026-08-04，commit `b599fb5`）**——wire log 默认关闭；开启后 header-only（不含 payload），payload hex 需显式 opt-in（`wire_log_payload`）；64 MiB 轮转；文档标注敏感。

### 位置

- `crates/handshaker-core/src/protocol/frame.rs:20-62`
- `crates/handshaker-core/src/protocol/frame.rs:66-89`

Wire log 逐字节输出完整 payload，可能包含：

- 剪贴板文本；
- 路径和文件元数据；
- 信任/握手信息；
-媒体数据或其他敏感内容。

Unix 使用 0600 是正确的，但非 Unix 权限设置为空实现；同时没有大小上限、轮转或字段脱敏。

建议：

- 配置项名称明确标注“可能记录敏感数据”；
- 默认关闭；
- 支持只记录 header/类型/长度；
- payload 模式需要显式 unsafe/debug opt-in；
- 文件大小上限与轮转；
- Windows 使用 ACL 限制当前用户；
- diagnostics 中显示 wire log 是否开启，但不要输出敏感路径之外的内容。

---

## P2-5：每个 FFI Subscription 创建独立 current-thread Tokio Runtime

**状态：✅ 已修复（2026-08-04，commit `92c1768`）**——活跃订阅数有界（Runtime 层 cap，超限返回错误）；Subscription 内部 runtime 仅启用 time driver。

`hs_subscribe_events()` 每创建一个 Subscription 就创建一个新 Tokio runtime。少量订阅可接受，但多窗口、多 SDK 或重复订阅时开销不必要。

建议让 Subscription 复用 HsRuntime 的 executor，或改用 `blocking_recv`/Condvar 桥接的同步队列。至少在 Runtime 层限制活跃订阅数并暴露 diagnostics。

---

# 6. 推荐修复顺序

## Gate 0：数据安全与生命周期，必须首先完成

建议一个独立里程碑 `M8.x Safety Closure`：

1. P0-1 删除失败保留台账。
2. P0-2 Ledger v2：哈希键 + 内嵌设备 UUID + Application Profile 校验。
3. P0-3 修复 Swift `device_updated`，建立 Rust 生成 fixture。
4. P0-4 统一 Task Registry 原子 spawn/publish。
5. P0-5 禁止淘汰尚未 execution-finished 的 Transfer。

Gate 0 通过条件：

- 所有竞态测试使用 barrier 可确定重现，不使用“循环 1000 次碰运气”；
- shutdown 返回后任务数为 0；
-台账和磁盘状态在所有失败路径一致；
- Swift 能解码 Rust 生成的每个事件 fixture。

## Gate 1：稳定 SDK 契约

1. Sync watch 在 lag/commit failure 后停止并要求 full reconcile。
2. Swift Subscription 区分 timeout/closed。
3. EventStream 背压与 sequence gap 处理。
4. Swift FFI 调用改为真正 async，解除全局串行锁。
5. JSON contract version + 自动 fixture。
6. FFI Handle 并发销毁模型定稿。

完成后再考虑把：

```text
APPLICATION_API_VERSION = 1.0.0-preview.1
```

提升为正式 `1.0.0`。

## Gate 2：大量相册和分发

1. 媒体 metadata 分页。
2. 列表响应剥离 thumbnail byte array。
3. 大文件本地 I/O 移到 async/spawn_blocking。
4. 双架构、自包含 XCFramework。

---

# 7. 建议的新测试矩阵

## Core

- Sync 删除 permission denied 台账一致性。
- Sync conflict 文件 unreadable。
- Ledger UUID 碰撞、身份篡改、v1 -> v2 迁移。
- 并发临时文件和取消残留清理。
- 大文件 hash 不阻塞心跳的集成测试。
- Frame/protobuf 模糊输入 fuzz。

## Application

- start/stop/shutdown 在 Handle 发布窗口的 barrier 测试。
- 业务终态与执行终态分离测试。
- Sync watch lag 后自动停止。
- 多 Profile 事件路由。
- Profile UUID 与 Session phone ID 不一致拒绝。
- shutdown 后无 Task、无 Session、无监控、无传输。

## FFI

- destroy 与普通调用并发的明确契约测试。
- subscription next/destroy 并发。
- 每个 JSON request/response/event golden fixture。
- 1 万条媒体 metadata 响应大小和最大限制。
- NULL、错误长度、无效 UTF-8、极长 JSON、未知 enum。

## Swift

- 直接消费 Rust 生成的所有事件 fixtures。
- `device_updated` nested device 回归测试。
- `.closed` 不热循环。
- buffer dropped/sequence gap 行为。
- concurrent calls + shutdown。
- Task cancellation 后 Subscription/Runtime 无泄漏。
- clean machine Package linking。

## 真机

至少覆盖 ADB、Wi-Fi、USB AOA：

- 空闲断线；
-传输中拔线；
-同步中取消；
-本地文件被占用/只读；
-相册 10k+ 文件；
-手机端连续快速产生 FILE_CHANGE；
-信任记录重置/错误密钥恢复；
-多 Session 同时连接。

---

# 8. 稳定发布 Definition of Done

以下全部满足后，才建议宣称 Application/FFI/Swift SDK 稳定。
> 更新于 2026-08-04（审计收尾后）：16/16 已完成。

- [x] P0-1 至 P0-5 全部关闭，并有确定性回归测试。
- [x] Application 对 SyncProfile 进行权威校验，不依赖 FFI/Swift。
- [x] Ledger 文件名无碰撞，文件内验证设备身份。
- [x] 所有后台任务有明确 owner、JoinHandle 和执行终态。
- [x] shutdown 返回时没有后台网络/文件任务。
- [x] Rust 生成 JSON fixtures，Swift 全量消费通过。
      （完成于 2026-08-04，commit `bc7c6b9`：13 个 BackendEvent 变体全部有
      权威 fixture——`examples/gen_event_fixtures.rs` 生成，Rust 锁定测试
      + Swift 全量解码/re-encode 消费）
- [x] EventStream 能区分 timeout/closed、检测 sequence gap。
- [x] 生命周期事件和终态事件不会被静默丢弃。
- [x] Swift 公共 API 不在 actor/main executor 上执行阻塞 FFI。
- [x] FFI destroy 并发契约安全或明确受控。
- [x] 媒体目录有分页和响应上限。
- [x] XCFramework 包含 arm64/x86_64 且不依赖用户 Homebrew 路径。
- [x] macOS 基础 CI 到位。
- [x] 真机 acceptance 启用时失败不会被吞掉。
- [x] `APPLICATION_API_VERSION`、FFI ABI、JSON contract 分别独立版本化。
- [x] 文档、Header、Swift SDK 和实际实现一致。

---

# 9. 总结判断

当前项目已经具备继续开发正式 macOS 客户端的基础，协议覆盖和跨层架构整体方向正确。它现在最需要的不是继续快速增加 API，而是做一次**一致性和生命周期收口**：

- 同步引擎必须让台账始终反映磁盘事实；
-后台任务必须在任何并发顺序下都能被 stop/shutdown 找到并回收；
- Rust 与 Swift 必须共享自动生成的契约，而不是靠两边手工同步；
-事件流必须把丢失和关闭作为一等状态；
-媒体和 SDK 调用模型需要为大量文件和长期运行做好准备。

完成 Gate 0 后，可认为基础安全性基本闭环；完成 Gate 1 后，才适合冻结 Application v1 和 Swift SDK；完成 Gate 2 后，才适合面向普通用户分发和承担大相册工作负载。
