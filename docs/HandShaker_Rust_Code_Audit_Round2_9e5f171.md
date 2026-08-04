# HandShaker Rust / Application / FFI / Swift SDK 第二轮代码审计

> 审计基线：`9e5f171f52669c7c49d9c6b0a1368ca256fad565`  
> 输入：`HandShaker_Rust-main-3.zip`  
> 审计日期：2026-08-04  
> 审计性质：上一轮审计修复后的**复审**，重点验证修复是否真正闭环，并重新扫描回归风险。

---

## 1. 执行摘要

这一轮代码的整体成熟度已经明显高于上一轮：Core 协议功能、Application DTO/Service、FFI 导出和 Swift SDK 的业务覆盖均已接近可供真实 GUI 开发使用的状态。上一轮发现的多项直接缺陷确实已经修复，包括：

- 本地删除失败时不再错误删除同步台账；
- 同步台账设备 UUID 文件名碰撞已改为 SHA-256，并在文件内校验设备身份；
- Swift `device_updated` 嵌套对象解码已与 Rust 一致；
- Sync watch 在事件丢失或增量应用失败后会停止并要求全量 reconcile；
- 本地冲突检查已改为失败闭合；
- 同步临时文件唯一化、大文件哈希移入 `spawn_blocking`；
- Swift timeout/closed、事件序列缺口、RuntimeHandle lease 等已大幅改善。

但仓库中“20/20 全部关闭”的结论仍然偏乐观。当前仍存在 **3 项 P0、6 项 P1、5 项 P2**。其中 P0 主要集中在：

1. **同一手机的多个同步 Profile 共用一个台账，可能跨目录污染甚至删除错误目录中的文件。**
2. **同步文件副作用与台账提交不是事务，取消、崩溃或强制 abort 后可能形成不可恢复的一致性窗口。**
3. **Transfer 和 Sync Watch 的任务发布仍未与 shutdown/reap 完全原子化，后台任务仍可能脱离 Runtime 生命周期。**

因此本轮结论是：

- 可以继续开展 Swift GUI 集成、真机联调和业务功能开发；
- **不建议把当前版本定义为稳定的 1.0 SDK/同步引擎发布候选；**
- 先关闭本报告 3 项 P0，再进行一次带 Rust 编译、线程消毒器/故障注入和真机断连的发布候选审计。

### 风险统计

| 等级 | 数量 | 含义 |
|---|---:|---|
| P0 | 3 | 可能导致用户文件/台账错误、shutdown 后任务存活或跨 Profile 数据污染 |
| P1 | 6 | 可靠性、安全边界、内存控制、SDK 契约或大规模数据性能问题 |
| P2 | 5 | 发布工程、诊断、文档和输入契约完善项 |

---

## 2. 上一轮审计问题复核

### 2.1 上一轮 P0 复核

| 原编号 | 本轮状态 | 复核结论 |
|---|---|---|
| P0-1 删除失败仍删除台账 | **已修复** | 全量与增量删除均通过 `remove_local_synced_file`，只有删除成功或文件已不存在时才移除记录。 |
| P0-2 台账文件名碰撞/不校验设备身份 | **原问题已修复，但闭环不完整** | SHA-256 文件名和 v2 内嵌 `device_uuid` 已正确实现；但台账仍只按设备而不是 Profile scope 区分，形成新的跨 Profile 污染问题。 |
| P0-3 Swift `device_updated` 解码错误 | **已修复** | Swift 已从 `device` 嵌套字段解码，并有 Rust fixture。 |
| P0-4 JoinHandle 发布竞态 | **部分修复** | Sync run 使用 oneshot gate；但 Transfer 仍是 register → spawn → publish，Sync Watch 在网络 await 之后才 spawn/publish。 |
| P0-5 终态 Transfer 被提前淘汰 | **部分修复** | 已检查 `JoinHandle::is_finished()`；但 `join == None` 仍被视为可淘汰，而 `None` 同时表示“尚未发布”。 |

### 2.2 上一轮 P1 复核

| 原编号 | 本轮状态 | 复核结论 |
|---|---|---|
| P1-1 冲突检查失败开放 | 已修复 | 文件读取/哈希失败会形成冲突，不再覆盖本地文件。 |
| P1-2 Watch 丢事件后继续运行 | 已修复 | Lagged 或 apply/commit 失败会停止 watch 并设置 `reconciliation_required`。 |
| P1-3 临时文件碰撞/哈希阻塞 Tokio | 已修复 | 随机临时名、清理 guard、`spawn_blocking` 已落地。 |
| P1-4 timeout 与 closed 混同 | 已修复 | Swift `SubscriptionPoll` 已区分。 |
| P1-5 单事件缓冲且静默丢弃 | 基本修复 | 缓冲增至 256，并在 drop/sequence gap 时显式失败；注释仍保留旧的 bufferingNewest(1) 描述。 |
| P1-6 Swift Actor 被普通 FFI 调用阻塞 | 大部分修复 | 普通调用移入并发 DispatchQueue；但 EventStream 的同步 poll 仍由继承 Actor 上下文的 `Task` 执行。 |
| P1-7 JSON 契约没有版本 | 部分修复 | 已增加 `json_contract`，但 Swift 用 `>=` 接受未来破坏性版本，检查方向错误。 |
| P1-8 C Handle destroy 并发 UAF | 契约化关闭 | C Header 已明确禁止并发 destroy，Swift lease 正确；裸 C 调用方仍必须遵守契约。 |
| P1-9 媒体全量 JSON/缩略图 | 部分修复 | 缩略图已剥离，FFI 提供分页；但每一页仍重新向手机拉取完整媒体库。 |
| P1-10 XCFramework 不可分发 | 部分修复 | 构建脚本生成 universal/static；Package 与 CI 仍强依赖 Homebrew `/opt/homebrew/lib`。 |

---

# 3. P0 问题

## P0-1：同步台账只按设备 UUID 区分，多个 Profile 会共享并覆盖同一个台账

### 位置

- `crates/handshaker-application/src/sync.rs:20-35`
- `crates/handshaker-application/src/runtime.rs:1923-1930`
- `crates/handshaker-core/src/sync_store.rs:24-43`
- `crates/handshaker-core/src/sync_store.rs:67-78`
- `crates/handshaker-application/src/event.rs` 对多 Profile 路由已有明确支持

### 当前行为

`SyncProfileDto` 明确包含：

```rust
pub struct SyncProfileDto {
    pub id: String,
    pub session_id: SessionId,
    pub device_uuid: String,
    pub remote_root: String,
    pub local_root: String,
    pub enabled: bool,
}
```

但 `sync_store_for()` 只使用 `device_uuid`：

```rust
SyncStore::discover(
    &self.sync_config_dir()?,
    &profile.device_uuid,
)
```

Core 的 v2 ledger 也只存储：

```rust
struct SyncLedgerFileV2 {
    schema_version: u32,
    device_uuid: String,
    snapshot: SyncSnapshot,
}
```

因此以下两个合法 Profile 会访问同一个 JSON 文件：

```text
Profile A:
  phone = X
  remote_root = /DCIM/Camera
  local_root = /Users/me/Pictures/Phone

Profile B:
  phone = X
  remote_root = /Pictures/Screenshots
  local_root = /Volumes/Archive/Screenshots
```

### 风险路径

1. Profile A 完成同步，台账记录的 `local_path` 指向 A 的目录。
2. Profile B 加载同一台账。
3. B 的 diff、冲突检测和删除操作以 A 的记录为基础。
4. B 可能：
   - 认为某些文件已经同步而跳过写入 B 目录；
   - 对 A 目录中的文件执行冲突判断；
   - 在手机侧“删除”状态下删除 A 目录中的文件；
   - 与 A 并发执行时以最后一次 `rename` 覆盖整份台账。

Registry 又以 `profile.id` 作为并发互斥键，因此两个不同 Profile ID 可以同时写同一个 ledger，形成 last-writer-wins。

### 严重性

这是用户文件隔离问题，不只是状态显示错误。它可能让一个 Profile 操作另一个 Profile 的本地目录，因此列为 P0。

### 推荐修改：Ledger schema v3，身份必须包含 Profile scope

建议定义不可歧义的同步 scope：

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncLedgerIdentity {
    pub device_uuid: String,
    pub remote_root: String,
    pub local_root: String,
}

#[derive(Serialize, Deserialize)]
struct SyncLedgerFileV3 {
    schema_version: u32,
    identity: SyncLedgerIdentity,
    snapshot: SyncSnapshot,
}
```

文件键不要仅使用 caller-chosen `profile.id`，应使用规范化 scope：

```rust
fn ledger_key(identity: &SyncLedgerIdentity) -> String {
    let canonical = format!(
        "{}\0{}\0{}",
        identity.device_uuid,
        normalize_remote_root(&identity.remote_root),
        normalize_local_root(&identity.local_root),
    );
    sha256_hex(canonical.as_bytes())
}
```

Application 应同时做两件事：

1. `sync_store_for(profile)` 根据完整 scope 创建 store；
2. `sync_jobs` 之外再用同一个 `ledger_key` 做写入互斥，禁止两个不同 Profile 同时改一份 ledger。

示意：

```rust
struct RuntimeInner {
    sync_jobs: Mutex<HashMap<String, Arc<SyncJob>>>,
    sync_ledger_locks: Mutex<HashMap<SyncLedgerKey, Arc<tokio::sync::Mutex<()>>>>,
}
```

### v2 → v3 迁移

不能把旧的 device-only v2 ledger 自动绑定到任意新 Profile。安全策略：

- 首次迁移时要求调用方显式提供旧 Profile 的 remote/local roots；
- 或仅在存在一个明确的 legacy/default Profile 时迁移；
- 无法证明 scope 时拒绝迁移，并保留原文件备份；
- v3 load 必须逐字段验证 identity。

### 必须增加的测试

1. 同一 `device_uuid`、不同 `local_root` 得到不同 ledger path。
2. 同一设备不同 `remote_root` 不共享 snapshot。
3. Profile B 的删除计划绝不能访问 Profile A 的 `local_path`。
4. 两个 Profile 并发运行不会覆盖对方 ledger。
5. 将 A 的 v3 文件复制到 B 的 key 后，load 返回 identity mismatch。
6. v2 迁移无法证明 scope 时失败闭合。

---

## P0-2：同步文件副作用与台账提交不是事务，取消或崩溃后可能形成错误冲突与覆盖窗口

### 位置

- `crates/handshaker-core/src/sync.rs:146-219`
- `crates/handshaker-application/src/runtime.rs:2213-2222`
- `crates/handshaker-application/src/runtime.rs:2256-2278`
- `crates/handshaker-application/src/runtime.rs:2321-2351`
- `crates/handshaker-application/src/runtime.rs:2380-2419`
- `crates/handshaker-application/src/runtime.rs:2737-2791`

### 当前行为

Core `execute_plan()` 会逐项直接修改本地文件，并只在内存中更新 `SyncSnapshot`：

```rust
let (result, updated) = execute_plan(...).await?;
```

Application 等全部文件操作完成后才保存一次：

```rust
store.save(&updated)?;
```

`stop_sync()` 在等待超时后会：

```rust
task.abort();
let _ = task.await;
```

### 失败窗口

下载或删除已经对本地文件系统生效，但以下情况会让 ledger 仍保持旧状态：

- Runtime/进程崩溃；
- `stop_sync()` 在执行中 abort；
- 后续某个文件导致 task panic；
- 全部文件操作完成，但最终 ledger save 失败；
- watch batch 中部分文件已改动，随后 apply/save 失败。

### 具体后果

#### 新文件

1. 文件已下载并 rename 到正式位置；
2. ledger 尚未记录；
3. 用户在下次运行前编辑该文件；
4. 下次同步把它视为“远端新增、未跟踪”，冲突检查没有旧 SHA 可比较；
5. 用户修改可能被覆盖。

#### 已跟踪文件更新

1. 新版本已覆盖本地文件；
2. ledger 仍保存旧 `local_sha256`；
3. 下次运行会把刚刚由 HandShaker 写入的文件误判为用户修改冲突。

#### 删除

文件可能已删除但 ledger 未提交。虽然下一次通常可重试清理记录，但运行结果与持久状态仍不具备事务语义。

### 为什么“最后原子保存整份 ledger”还不够

原子 rename 只保证 ledger 文件本身不会半写；它不能把“本地文件修改”和“ledger 修改”变成一个原子事务。

### 推荐修改：操作日志/WAL + 每项 checkpoint

最稳妥的方案是增加同步 journal：

```rust
#[derive(Serialize, Deserialize)]
enum PendingSyncAction {
    Download {
        remote_path: String,
        final_path: PathBuf,
        staged_path: PathBuf,
        new_record: SyncFileRecord,
    },
    Delete {
        remote_path: String,
        original_path: PathBuf,
        staged_trash_path: PathBuf,
        old_record: SyncFileRecord,
    },
}
```

#### 下载事务顺序

1. 下载到唯一 staged temp；
2. fsync staged file；
3. 写入并 fsync journal；
4. 原子 rename staged → final；
5. 更新并原子保存 ledger；
6. 清除 journal。

#### 删除事务顺序

不要直接 unlink：

1. 写入 journal；
2. 将文件原子 rename 到同分区 staging trash；
3. 更新并保存 ledger；
4. 删除 staging trash；
5. 清除 journal。

启动时先执行 recovery：

```rust
store.recover_pending_action()?;
```

至少也应把 `execute_plan` 改为逐项 checkpoint，而不是全部执行后一次提交：

```rust
execute_plan_with_checkpoint(
    ...,
    |snapshot| store.save(snapshot),
).await
```

不过只有 checkpoint 仍存在“文件 rename 成功、ledger save 前崩溃”的小窗口；稳定同步产品建议直接做 journal。

### Cancellation 设计

- 在文件与文件之间检查 cooperative token；
- commit/journal 临界区进入后屏蔽 abort，完成或回滚后再响应取消；
- `stop_sync()` 首先 cooperative cancel；
- 超时 abort 只允许发生在可由 journal 恢复的阶段；
- 不允许在持有 ledger 写锁时被任意 abort。

### 必须增加的故障注入测试

对下载与删除分别在以下每个点模拟崩溃/取消并重启：

1. staged 下载完成前；
2. staged 完成、journal 前；
3. journal 后、rename 前；
4. rename 后、ledger save 前；
5. ledger save 后、journal 清理前；
6. 第 N 个文件成功、N+1 文件失败；
7. `stop_sync()` 在每个阶段触发；
8. ledger save 返回权限错误/磁盘满。

验收标准：重启恢复后，正式文件与 ledger 必须始终对应，且绝不覆盖无法证明来源的本地修改。

---

## P0-3：Transfer 与 Sync Watch 的任务发布仍未与 shutdown/reap 原子化

### 位置

- `crates/handshaker-application/src/runtime.rs:220-269`
- `crates/handshaker-application/src/runtime.rs:1481-1544`
- `crates/handshaker-application/src/runtime.rs:1548-1610`
- `crates/handshaker-application/src/runtime.rs:2458-2487`
- `crates/handshaker-application/src/runtime.rs:2506-2568`
- `crates/handshaker-application/src/runtime.rs:3130-3150`
- `crates/handshaker-application/src/transfer.rs:197-214`
- `crates/handshaker-application/src/transfer.rs:221-248`

### 3.1 Transfer 仍是三段式非原子发布

当前流程：

```rust
let entry = transfers.register(snapshot); // join = None
let handle = tokio::spawn(...);           // task 已可运行
*entry.join.lock() = Some(handle);         // 最后才发布
```

`start_download`/`start_upload` 在入口只执行一次 `ensure_open()`，之后没有与 shutdown 共用的 admission lock。

#### shutdown 竞态

可能发生：

1. `start_download` 通过 `ensure_open()`，取得 `session.client Arc`；
2. shutdown 设置 `shutting_down = true`；
3. shutdown 枚举当前 Transfer，此时新 Transfer 尚未 register；
4. shutdown 移除并关闭 Session；
5. start_download 随后 register/spawn，并返回成功；
6. 新任务持有 Client Arc，在 Runtime shutdown 之后继续执行或失败清理。

#### cancel/reap 竞态

`reap_locked()` 的条件：

```rust
join.as_ref().is_none_or(|handle| handle.is_finished())
```

这里 `None` 同时表示：

- 尚未发布 Handle；
- 从未创建任务；
- Handle 已被其他路径取走。

如果在 register 与 handle publication 之间调用 cancel，再触发 register/reap，条目已是业务终态且 `join == None`，会被错误认为 execution finished 并淘汰。

上一轮测试只覆盖“Handle 已经写入、任务仍运行”的场景，没有覆盖“终态发生在 publication 前”的场景。

### 3.2 Sync Watch 在长网络 await 后才发布任务

`start_sync_watch()`：

1. reserve `monitoring = true`；
2. subscribe；
3. `await client.sync_monitor(true)`；
4. 之后才 spawn 并写入 `watch_task`。

`sync_monitor(true)` 最长可阻塞到默认网络超时。shutdown 的 `take_published_task()` 只轮询 400 ms。

因此 shutdown 可能清理 registry/session 后，原 start 调用才从网络 await 返回并 spawn 一个已脱离 registry 的 watch task。

### 推荐修改：统一 ManagedTaskSlot + Runtime launch admission gate

不要再让 `Option<JoinHandle>`承担状态机。建议：

```rust
enum TaskPublication {
    Reserved,
    Published(tokio::task::JoinHandle<()>),
    Finished,
    Taken,
}

struct ManagedTaskSlot {
    state: tokio::sync::Mutex<TaskPublication>,
    changed: tokio::sync::Notify,
}
```

Runtime 增加统一 admission gate：

```rust
struct RuntimeInner {
    launch_gate: tokio::sync::RwLock<()>,
    shutting_down: AtomicBool,
    // ...
}
```

启动操作：

1. 持有 `launch_gate.read()`；
2. 再检查 `shutting_down`；
3. reserve registry entry；
4. spawn 一个等待 oneshot 的任务；
5. 在 registry 中写入 `Published(handle)`；
6. 再次检查 shutdown；
7. release oneshot；
8. 释放 read gate。

shutdown：

1. 获取 `launch_gate.write()`；
2. 设置 shutting_down；
3. 此后不可能有新的 Reserved/Published；
4. 枚举、cancel、take、join 所有任务；
5. 再关闭 Session。

Transfer 的 evict 条件必须是明确的 `Finished`，不能把 `None` 当完成：

```rust
matches!(*slot.lock().await, TaskPublication::Finished | TaskPublication::Taken)
```

Sync Watch 的 `sync_monitor(true)` 也必须属于被管理的 activation task，或至少在 await 返回后重新进入 launch gate、检查 Runtime 状态；若 shutdown 已开始，应 best-effort `sync_monitor(false)`，且不得 spawn watch。

### 必须增加的确定性测试

使用 Barrier/Notify 精确停在以下位置：

1. Transfer register 后、spawn 前触发 shutdown；
2. spawn 后、Handle publication 前触发 cancel + reap；
3. shutdown 枚举完成前后启动 Transfer；
4. `sync_monitor(true)` 卡住时触发 shutdown；
5. monitor(true) 在 shutdown 清理后才返回成功；
6. stop/shutdown 等待 publication 不依赖固定 400 ms；
7. 所有测试最后断言 registry 无 live task、Client Arc 释放、无网络任务继续运行。

---

# 4. P1 问题

## P1-1：协议 Reader 的每请求队列和 unmatched SID 总量无边界，可被异常手机端耗尽内存

### 位置

- `crates/handshaker-core/src/session.rs:24-32`
- `crates/handshaker-core/src/session.rs:420-435`
- `crates/handshaker-core/src/session.rs:559-628`
- `crates/handshaker-core/src/session.rs:672-714`

### 当前保护

已有良好的单项上限：

- 单 downstream chunk 最大 32,761 bytes；
- 单 push message 最大 4 MiB；
- 单 normal response 最大 64 MiB。

但缺少总量/队列上限：

```rust
type ChunkReceiver = mpsc::UnboundedReceiver<Result<Incoming>>;
struct PendingRequest {
    sender: mpsc::UnboundedSender<Result<Incoming>>,
}
```

Reader 对 pending 请求直接 `send` 到 unbounded channel。下载写磁盘较慢或消费者调度不及时的时候，手机端可以持续发 chunk，内存队列不受限。

未知 SID 的事件重组也使用：

```rust
let mut unmatched = HashMap::<u32, NormalAccumulator>::new();
```

虽然每个 accumulator 最大 4 MiB，但 SID 数量和所有 accumulator 的总字节数都没有上限。攻击者可以交错发送大量未完成 SID。

### 推荐修改

1. Pending channel 改为 bounded：

```rust
const PENDING_CHUNK_CAPACITY: usize = 64;
type ChunkReceiver = mpsc::Receiver<Result<Incoming>>;
```

Reader 使用 `sender.send(...).await`，让 TCP/USB 读取自然背压，而不是将压力转化为宿主内存。

2. unmatched 增加双上限：

```rust
const MAX_UNMATCHED_SIDS: usize = 64;
const MAX_UNMATCHED_BYTES: usize = 16 * 1024 * 1024;
```

每次 push 前计算 aggregate；超限应关闭连接并返回 ProtocolError，而不是继续丢事件后保持不可信会话。

3. 增加 SID TTL，长时间未完成的 accumulator 必须回收。

4. 对 pending request 数量也增加合理并发上限，避免调用方自己创建无限请求。

### 测试

- 65 个交错未知 SID 触发有界失败；
- 63 个 SID 的总字节数超过 aggregate cap；
- 慢下载 writer 时 Reader 队列不会超过固定容量；
- backpressure 不造成锁跨 await 死锁；
- fuzz 随机 SID/chunk/length，RSS 保持上界。

---

## P1-2：媒体“分页”每一页仍重新拉取整库，无法解决大相册峰值和快照一致性

### 位置

- `crates/handshaker-application/src/runtime.rs:1326-1361`
- `crates/handshaker-application/src/runtime.rs:1364-1421`

### 当前行为

`get_photo_library_page()` 先调用：

```rust
let full = self.get_photo_library(session_id).await?;
```

也就是每一页都：

1. 向手机请求完整媒体库；
2. Core 组装完整响应；
3. 反序列化完整对象；
4. Application 创建完整 DTO；
5. 最后才 `slice_page`。

因此分页只缩小 FFI 最终 JSON，不降低：

- 手机网络传输；
- Core `NORMAL_RESPONSE_LIMIT` 前的完整响应内存；
- DTO 转换峰值；
- 第二页、第三页的重复请求成本。

同时，媒体库在两页请求之间发生变化时，offset/media-id cursor 可能出现重复或跳过；当前 response 没有 snapshot token。

### 推荐修改

在 Session/Application 内增加 metadata snapshot cache：

```rust
struct MediaSnapshot<T> {
    id: MediaSnapshotId,
    generation: u64,
    created_at: Instant,
    items: Arc<[T]>,
}
```

API：

```text
first page request:  {session_id, limit}
response:            {snapshot_id, items, next_cursor}
next page request:   {session_id, snapshot_id, cursor, limit}
```

- 首次只拉取一次完整手机快照；
- 后续页从同一个 snapshot 切片；
- MediaChanged 事件生成新 generation，但旧 snapshot 保留短 TTL，保证正在翻页的客户端稳定；
- 大图库可将 metadata 存 SQLite/临时 mmap，而不是长期保留多个完整 Vec；
- 缩略图继续按 cache path 独立获取。

### 必须增加的测试

- page 2 不产生第二次 phone request；
- 翻页期间插入/删除媒体，旧 snapshot 页面仍无丢无重；
- snapshot 过期返回明确错误；
- 10 万条 metadata 的峰值和响应时间基准测试。

---

## P1-3：Swift JSON contract 检查接受未来破坏性版本，版本防线方向错误

### 位置

- `crates/handshaker-application/src/lib.rs:73-81`
- `platform/macos/Sources/HandShakerCore/Models/Diagnostics.swift:38-41`
- `platform/macos/Sources/HandShakerCore/Services/HandShakerRuntime.swift:165-187`

Rust 明确规定：破坏性 JSON 变化要递增 `JSON_CONTRACT_VERSION`。

Swift 当前：

```swift
guard contract.jsonContract >= RuntimeDiagnostics.minimumJSONContract else { ... }
```

这会让只理解 v1 的旧 Swift SDK 接受 Rust v2，而 v2 按定义可能已经重命名字段、改变 nesting 或 enum token。

### 推荐修改

若当前整数只代表 breaking generation，应要求严格相等：

```swift
public static let supportedJSONContract: UInt32 = 1

guard contract.jsonContract == Self.supportedJSONContract else {
    throw HandShakerError.unsupported(...)
}
```

更长期可改为：

```json
{
  "json_contract": {"major": 1, "minor": 3}
}
```

规则：同 major 且 runtime minor 位于 SDK 支持范围内才接受。

### 测试

- runtime 0：拒绝；
- runtime 1：接受；
- runtime 2：SDK v1 必须拒绝；
- 增加 required field/改变嵌套的 v2 fixture，证明不会进入普通解码流程。

---

## P1-4：EventStream 的同步 poll 仍可能占用 Swift Actor/cooperative executor

### 位置

- `platform/macos/Sources/HandShakerCore/Services/EventStream.swift:28-95`
- `platform/macos/Sources/HandShakerCore/Native/RuntimeHandle.swift:157-210`
- `platform/macos/Sources/HandShakerCore/Services/HandShakerRuntime.swift:107-143`

普通 Service FFI 已正确移入专用 `DispatchQueue`。但 EventStream 使用：

```swift
let poller = Task {
    switch try subscription.next(timeoutMs: 1000) { ... }
}
```

`eventStream()` 是 Actor-isolated 方法，`Task {}` 会继承当前 actor isolation/context；`subscription.next` 又是同步阻塞的 C 调用，最长阻塞 1 秒。注释所说“background Task never blocks actor”不可靠。

即使未固定在 actor executor，阻塞 cooperative executor worker 也不是合适的 FFI polling 模型。

### 推荐修改

为每个 subscription 使用真正的 native blocking queue：

```swift
let pollQueue = DispatchQueue(label: "handshaker.events.\(id)")

func pollAsync() async throws -> SubscriptionPoll {
    try await withCheckedThrowingContinuation { continuation in
        pollQueue.async {
            continuation.resume(with: Result {
                try subscription.next(timeoutMs: 1000)
            })
        }
    }
}
```

poller 可以是 `Task.detached`，但真正的阻塞调用仍放 DispatchQueue。destroy/next 继续由 SubscriptionHandle lock 协调。

### 测试

在完全无事件、native poll 正等待时：

- actor 上的 `diagnostics()` 或轻量状态方法应立即进入，不等待约 1 秒；
- 同时打开多个 EventStream，不应耗尽 cooperative pool；
- cancel stream 后最多一个 poll timeout 内回收 handle。

---

## P1-5：Swift `shutdown()` 吞掉所有 native 错误，调用方无法确认确定性关闭是否成功

### 位置

- `platform/macos/Sources/HandShakerCore/Services/HandShakerRuntime.swift:197-209`

当前：

```swift
public func shutdown() async {
    shutdownFlag.set()
    _ = try? await callNative {
        let result = hs_runtime_shutdown(runtime)
        hs_byte_buffer_free(result.value)
        hs_byte_buffer_free(result.error)
    }
}
```

问题：

- 不检查 `result.status`；
- 原生 `PublicError` 被丢弃；
- 方法不 `throws`；
- 调用方无法区分“已完整 join/close”与“shutdown 失败但被静默忽略”。

这与 Rust 侧强调的确定性 lifecycle 目标不一致。

### 推荐修改

```swift
public func shutdown() async throws {
    try await callNative {
        try self.handle.withRuntime { runtime in
            try hsCallVoid { hs_runtime_shutdown(runtime) }
        }
    }
    shutdownFlag.set()
}
```

如果必须保留无异常 deinit 清理，可提供两个层次：

```swift
public func shutdown() async throws
func shutdownBestEffort() async // internal/deinit only
```

幂等语义应保留：第二次成功返回，但第一次真实错误必须传给调用方。

### 测试

通过测试 FFI 注入 shutdown error，断言：

- public shutdown 抛出 typed error；
- buffer 全部释放；
- 重试行为有明确语义；
- deinit 仍执行 best-effort cleanup。

---

## P1-6：Swift Package 仍硬编码 Homebrew libusb，与“静态自包含 XCFramework”目标冲突

### 位置

- `platform/macos/Package.swift:13-20`
- `platform/macos/Package.swift:37-43`
- `scripts/build-ffi-macos.sh:2-9`
- `.github/workflows/macos-ci.yml:37-38`
- `scripts/run-ffi-smoke-tests.sh:8-24`

构建脚本声明 `LIBUSB_STATIC=1`，并称 XCFramework 无动态 libusb 依赖；但 Package 仍包含：

```swift
.linkedLibrary("usb-1.0"),
.unsafeFlags(["-L/opt/homebrew/lib"]),
```

CI 又预装 Homebrew libusb，Smoke test 也明确传 `-lusb-1.0`，这会掩盖静态归档没有真正自包含的问题。`/opt/homebrew` 还仅适用于 Apple Silicon Homebrew。

### 推荐修改

若 XCFramework 确实静态包含 libusb：

1. 删除 Package 的 `.linkedLibrary("usb-1.0")` 和 `/opt/homebrew` unsafe flag；
2. Smoke test 不再传 `-lusb-1.0`；
3. CI 增加：

```bash
nm -u dist/apple/libhandshaker_ffi.a | grep -q '_libusb_' && exit 1
otool -L dist/apple/libhandshaker_ffi.dylib
```

4. 增加“干净消费者项目”测试，不执行 `brew install libusb`，只依赖 XCFramework 完成 link/run。

如果实际仍需动态 libusb，则不要依赖用户 Homebrew，应把动态库作为受控 artifact 一并分发、签名和嵌入。

---

# 5. P2 问题

## P2-1：FFI request JSON 大多会静默忽略未知字段

除少数媒体请求外，大部分 `Ffi*Request` 没有：

```rust
#[serde(deny_unknown_fields)]
```

拼错字段会被当成默认值，例如 `overwirte` 可能静默变成 `overwrite = false`。在已经引入 JSON contract version 的情况下，应统一失败闭合。

### 建议

- 所有 FFI request struct 加 `deny_unknown_fields`；
- 对用于向后兼容的 envelope 采用显式 `extensions` 字段，而不是任意吞字段；
- 为每个导出至少增加一个 unknown-field rejection test。

---

## P2-2：订阅计数在 Tokio runtime 创建失败时可能泄漏一个配额

### 位置

- `crates/handshaker-ffi/src/lib.rs:650-695`

计数先 `fetch_add`，随后创建 current-thread runtime。若 `.build()` 失败，`ffi_try!` 直接返回，尚未创建 `HsSubscription`，也没有执行 `fetch_sub`。

### 建议

使用 RAII reservation：

```rust
struct SubscriptionReservation(Arc<AtomicUsize>);
impl Drop for SubscriptionReservation { /* fetch_sub */ }
```

成功 `Box::into_raw` 后 `reservation.commit()`。增加故障注入测试。

---

## P2-3：`sync_ledger_status(device_uuid)` 的模型与多 Profile 设计不匹配，并依赖当前工作目录

### 位置

- `crates/handshaker-application/src/runtime.rs:1933-1970`

它为了复用 profile validation 构造一个虚拟 Profile，并用：

```rust
std::env::current_dir().unwrap_or_default()
```

这会让纯本地 ledger status 受进程 CWD 可用性影响；在修复 P0-1 后，单独以 device UUID 也不足以选择 ledger。

### 建议

改为：

```rust
sync_ledger_status(scope: SyncLedgerIdentity)
list_sync_ledgers(device_uuid: Option<&str>)
```

validation 应拆成针对 device UUID 和 scope 的纯函数，不要伪造 Profile/CWD。

---

## P2-4：稳定版本注释与实现状态矛盾

`crates/handshaker-application/src/lib.rs:58-63` 仍说 preview、允许 breaking changes，紧接着 `65-71` 又声明已冻结 1.0.0。

`EventStream.swift:14-15` 仍描述 `.bufferingNewest(1)`，实际是 256。

同步 store 注释仍写 `<device_uuid>.json`/sanitize，而实现已改为 SHA-256。

### 建议

在下一次合并前执行一次 doc-code consistency sweep，并让关键常量/版本说明由测试生成，避免审计文档中的“已完成”状态反向成为错误事实来源。

---

## P2-5：CI 目前无法证明最终 Swift Artifact 的“无 Homebrew 依赖”

虽然 CI 已增加 universal 构建和 Swift tests，但它先安装 libusb，且 Package/Smoke test 都显式链接 Homebrew。这只能证明“安装 Homebrew 的 CI 能构建”，不能证明用户拿到的 XCFramework 自包含。

建议拆成两个 job：

1. Rust/FFI build job，可安装构建依赖；
2. clean-consumer job，只下载已生成 XCFramework，不安装 libusb，创建最小 Swift Package/App 完成 link 和启动诊断。

---

# 6. 建议修复顺序

## Gate A：同步数据安全

1. P0-1：Ledger v3，绑定完整 Profile scope；
2. Profile/ledger-key 级写互斥；
3. P0-2：journal/WAL 与重启恢复；
4. 故障注入、取消、磁盘满、权限失败测试。

完成 Gate A 前，不应向用户承诺照片同步具备数据安全保证。

## Gate B：任务生命周期

1. P0-3：统一 ManagedTaskSlot；
2. Runtime launch read/write gate；
3. Transfer、Sync run、Sync watch activation 全部走相同启动协议；
4. 去掉固定 400 ms publication polling；
5. barrier 测试证明 shutdown 后没有任务和 Client Arc 存活。

## Gate C：稳定 SDK 契约

1. JSON contract 改为 exact/range compatibility；
2. Event poll 移出 actor/cooperative executor；
3. `shutdown() async throws`；
4. FFI request `deny_unknown_fields`；
5. Rust fixtures → Swift tests 覆盖所有事件和主要 DTO。

## Gate D：规模与发布

1. 媒体 snapshot pagination；
2. Core Reader aggregate memory/backpressure；
3. 移除 Homebrew runtime/link 依赖；
4. clean-consumer CI；
5. 真机执行 10k/100k 媒体、长时间 watch、拔线、睡眠唤醒测试。

---

# 7. 具体 Definition of Done

## Core

- [ ] Pending chunk channel 有界并产生 socket backpressure。
- [ ] unmatched SID 数量、总字节、TTL 均有上限。
- [ ] 同步 journal 可从每个中断点恢复。
- [ ] 多 Profile ledger identity 不可碰撞、不交叉操作目录。
- [ ] 通过随机 chunk/SID fuzz 和内存上界测试。

## Application

- [ ] 所有后台任务通过一个统一 launch/ownership 状态机。
- [ ] shutdown 获取 admission write gate 后不存在新任务注册。
- [ ] Transfer `None` 不再表示 execution finished。
- [ ] Sync Watch activation 网络阶段也由 Registry 管理。
- [ ] 媒体分页基于固定 snapshot，不重复请求完整手机库。

## FFI

- [ ] JSON contract compatibility 规则明确并由双方测试。
- [ ] 所有 request struct 对未知字段失败闭合。
- [ ] Subscription reservation 在所有错误路径释放。
- [ ] ABI/JSON fixture/version snapshot 同步检查继续通过。

## Swift SDK

- [ ] Event polling 不运行在 actor/cooperative executor 上。
- [ ] `shutdown()` 抛出真实 native 错误。
- [ ] JSON contract v2 被 SDK v1 明确拒绝。
- [ ] Package 在无 Homebrew 环境可 link/run。
- [ ] clean consumer 项目通过 Swift Package 集成测试。

## 真机

- [ ] 同一手机两个 Profile 同时运行，无交叉文件访问。
- [ ] 同步中途 kill -9/强制退出，重启后 journal 正确恢复。
- [ ] 传输/Watch 启动瞬间拔线或 shutdown，无后台残留。
- [ ] 10 万媒体条目翻页稳定、无重复/遗漏、内存可控。
- [ ] 手机端异常高速分片时宿主内存保持在明确上界。

---

# 8. 本轮验证范围与限制

本轮完成：

- 解压并逐层检查 Core、Application、FFI、Swift SDK、脚本和 CI；
- 逐项核对上一轮审计文档的修复声明；
- 对两个 ZIP 版本做文件级差异确认；
- ZIP 完整性检查通过；
- 所有 shell 脚本通过 `bash -n`；
- locale JSON 可解析；
- GitHub Actions YAML 可解析；
- `swift package dump-package` 成功，Package manifest 语法有效。

当前执行环境没有 `cargo`/`rustc`，且为 Linux，不能独立运行 macOS XCFramework/Swift test。因此本报告没有声称：

- Rust Workspace 已重新编译；
- Clippy/test 已在本轮实际通过；
- XCFramework 已在干净 Mac 上链接；
- 真机行为已重新验证。

这些动态验证应作为关闭本轮 P0/P1 的必要条件，而不是仅依赖静态代码和既有修复注释。
