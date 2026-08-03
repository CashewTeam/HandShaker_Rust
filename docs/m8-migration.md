# M8 迁移记录(migration)

> 分支:`refactor/m8-workspace-application-ffi`,基线 `docs/m8-baseline.md`(0.6.1,154 测试)

## 1. 迁移步骤与提交

| 提交 | 内容 |
|---|---|
| `908fe1c` | M8 计划、基线记录、--help fixture |
| `2c3e85c` | workspace 根 + `handshaker-core`(library/build.rs/examples/localization 迁移,git rename 保历史) |
| `ca240e0` | `handshaker-cli`(binary 名 handshaker 保持,154 测试恢复) |
| `928e7fc` | application:Runtime + SessionRegistry + v1 DTO + PublicError |
| `e112df8` | DTO/错误码 serde 冻结 + `docs/application-api-v1.md` |
| `1f281d3` | TransferManager + EventHub |
| `a809388` | 文件服务方法(§5.5) |
| `956db3b` | CLI `device list` 走 Application(JSON 逐字节兼容) |
| `6561b92` | handshaker-ffi(C ABI、panic 边界、事件订阅) |
| `950a4e5` | Swift/C smoke test + 打包脚本 |

## 2. 兼容性结论

- `handshaker` binary 名与 `--version` 输出不变;`--help` 与基线 fixture 逐字节一致;
- CLI JSON/JSONL 字段无变化(`device list` 迁移经重建 payload 保持);
- 退出码、确认规则、Ctrl-C 行为不变;
- 依赖零升级(workspace.dependencies 沿用现有版本);
- 测试 154 → 186(`crates/handshaker-core/tests/localization.rs` 改为扫全 workspace src,
  CJK 约束对 core/cli/application/ffi 全部生效)。

## 3. 路径与资源变化

- `build.rs` proto 路径:`crates/handshaker-core/build.rs` → `../../proto/smartsync.proto`;
- `i18n.rs` include_str:`../../../locales/zh-CN.json`;
- `CARGO_BIN_EXE_handshaker`(CLI 集成测试)随 crate 迁移自动工作;
- 根 `Cargo.toml` 现为纯 workspace(无 package);`cargo build --workspace` 构建全部;
- 反编译基准 `original_smali_1.2.0/`(gitignored)未受影响。

## 4. 已知迁移遗留(非回归)

- CLI 其余命令(clipboard/media/sync/watch/batch/shell)仍直连 core,
  按文档 Phase 3 渐进迁移(下一阶段:clipboard/media → watch/sync/shell);
- `handshaker-test-support` crate 尚未拆分(当前 core 内部 `#[cfg(test)]`),预留;
- Application 的 Clipboard/Media/RemoteFile 事件为预留变体,未桥接;
- `fs.rm`/`fs.count` 暂留 core:rm 的 JSON 契约是 `RemoteFile` 数组(Application
  `DeleteResultDto` 返回路径字符串数组),count 携带 CLI 专用 exclusions 语义;
- CLI fs pull/push 的 human 实时进度降级为完成后汇总(JSONL 本就无每文件进度),
  `render_batch_progress` 已删除。

## 5. 0.7.1 迁移记录(提交 6bd6abf / 8bb89c5 / d7e516d / fd8b96b)

- **连接统一走 runtime(6bd6abf)**:`connect()` 构造 `DeviceDescriptor` 并经
  `HandShakerRuntime::connect` 建会话;无 serial 时经 `list_devices` 自动选唯一在线
  ADB 设备(0/多台 → DeviceSelection,同 i18n key,exit 3);`close_session` 消费式
  drop CLI 的 client 句柄使 `disconnect` 独占发 QUIT;`session_client` 过渡 API
  (冻结前移除)供未迁移命令复用同一连接;watch 保留独立 all-callbacks 连接。
  adb 缺失时错误文案从 `adb.spawn_failed` 收敛为 `adb.no_online_device`(exit 仍 3)。
- **fs 基础命令(8bb89c5)**:`ls/stat/exists/mkdir/mv` 走 runtime;CliFileEntry
  适配层保持与 core `RemoteFile` 相同的 JSON 字段集(单测锁定);mkdir 后 stat-back
  保持 RemoteFile 形状输出。
- **fs pull/push 批量上移(7e516d 之 d7e516d)**:`BatchTransferRequest{files, trees,
  overwrite}`;runtime `batch_download/batch_upload` 把 trees 交给 core
  `download_tree/upload_tree`(递归枚举与路径逃逸防护留在 core),files 交给
  `download_many/upload_many`(串行、并发 1、失败聚合不中止);CLI 只留参数解析/
  路径 resolve/misparsed 检查/确认/展示;dry-run 留在 CLI(pull 用 runtime
  `list_files` 枚举,push 复用 `collect_local_tree`);`BatchTransferResultDto` JSON
  与 legacy core 结果一致。
- **FFI 传输面(fd8b96b)**:ABI 1.1.0;`hs_transfer_start_download/start_upload/
  cancel/get/list`;header 追加;Swift smoke 覆盖错误路径与空列表。
- 测试 186 → 192(core 120 / bin 22 / cli 12 / app 22 / ffi 15 / localization 1);
  0.7.2 收尾后 **197**(core 120 / app 23 / bin 22 / cli 12 / ffi 19 / localization 1)。

## 6. 全量核对记录(HEAD 2965f64,工作树干净)

> **历史快照**:本节记录 2965f64 时点的核对结果,表格中过期条目(如
> Application API `"1.0.0"`、FFI 21 导出 ABI 1.1.0、`hs_create_directory`/
> `hs_ping` 未导出、`generate-ffi-header.sh` 未建)反映当时状态;
> 最新状态见 §8(M8.1 Phase A:ABI 1.2.0 / 23 导出 / Application preview)。
> 核对范围:CLI 命令迁移矩阵、Application 冻结条款、FFI 导出与未完成条目、
> Phase 7 脚本/产物。核对方式:逐命令/逐导出 grep 代码,与 §4/§5 记录比对。

### 6.1 CLI 迁移矩阵(逐命令)

| 命令 | 路径 | 状态 |
|---|---|---|
| `device list` | 独立 `device_list`(不连接,走 Application) | ✅ 已迁移(M8) |
| `device info/ping` | `session.client`(core) | ⏳ 未迁移 |
| `device discover` / `trust.*` | 连接前处理,直连 core | ⏳ 未迁移 |
| `fs ls/stat/exists/mkdir/mv` | `session.runtime.*` | ✅ 已迁移(8bb89c5) |
| `fs count` | `client.file_count`(CLI 专用 exclusions 语义) | ⏳ 暂留 core(§4) |
| `fs rm` | 编排 `client.stat` 检查 + `client.delete`(JSON 契约是 RemoteFile 数组) | ⏳ 暂留 core(§4) |
| `fs pull/push` | 编排在 CLI,批量交 `runtime.batch_download/upload` | ✅ 已迁移(d7e516d),编排残留 `client.stat`/`file_exists` 仅作目标存在性判断 |
| `clipboard.*` | `session.client` | ⏳ 未迁移 |
| `media.*` | `session.client` | ⏳ 未迁移 |
| `sync.*` | `session.client` | ⏳ 未迁移 |
| `shell` / `batch` / `watch` | 独立路径,core(REPL/长连接交互留在 CLI) | ⏳ 按设计保留 |

核对结论:§5 记录与代码一致;未迁移命令的列表与 §4 完全吻合,无文档超前/滞后差异。

### 6.2 Application 冻结条款核对(Phase 4)

| 条款 | 状态 | 证据 |
|---|---|---|
| `APPLICATION_API_VERSION` | ✅ | `crates/handshaker-application/src/lib.rs:35` = `"1.0.0"` |
| DTO serde 契约 | ✅ | `dto.rs` 全部 `Serialize/Deserialize`,snake_case |
| enum 判别值不复用 | ✅ | `TransportKind{1,2,3}`/`SessionState{1..5}` 固定 |
| 错误码分区 | ✅ | `PublicErrorCode` 1001–9001(见 `docs/application-api-v1.md`) |
| v1 JSON fixture | ✅ | `tests.rs:225`「v1 JSON contract fixtures (frozen)」 |
| `#[non_exhaustive]` | ✅ | enum + 追加容忍变体 |
| 批量用例上移 | ✅ | `batch_download/batch_upload` + `BatchTransferRequest/ItemDto/ResultDto/TreeTransferDto/TransferFailureDto` |
| 事件模型 | ✅ | `BackendEvent` + `EventEnvelope`(单调 sequence) |
| 路径解析 | ✅ | `resolve_remote_path` 相对路径钳制到 root(bc72aba 修复) |

### 6.3 FFI 完成度(21 个导出,ABI 1.1.0)

**Phase 6 必须项 — 全部完成**:runtime create/shutdown/destroy、list devices、
connect/disconnect/get session、list files、subscribe/next/destroy。

**Phase 6 建议项 — 部分完成**:

| 建议项 | 状态 |
|---|---|
| start download | ✅ `hs_transfer_start_download` |
| get transfer | ✅ `hs_transfer_get` |
| cancel transfer | ✅ `hs_transfer_cancel` |
| create directory | ❌ 未导出(application 已有 `create_directory`,仅缺 FFI 包装) |
| ping | ❌ 未导出 |

**其余未导出(ffi-v1.md §5 已列,按需 minor 追加)**:
`stat/move/delete`、批量传输(batch)、媒体、同步、watch。

### 6.4 Phase 7 脚本/产物缺口

| 项 | 状态 |
|---|---|
| `scripts/build-ffi-macos.sh` | ✅ 已有 |
| `scripts/run-ffi-smoke-tests.sh` | ✅ 已有 |
| `scripts/build-ffi-linux.sh` | ❌ 未建(计划要求 macOS + Linux CI) |
| `scripts/generate-ffi-header.sh` | ❌ 未建(当前 header 手工维护) |
| `dist/apple/`(lib .a/.dylib + header + modulemap) | ❌ 未产出(脚本未运行/未入库) |
| `HandShakerCore.xcframework` | ⏳ 建议项,未做 |

### 6.5 后续提交补录(§5 未覆盖)

- `20007b2` docs:0.7.1 release notes、migration record、FFI v1.1 docs;
- `bc72aba` fix:batch_download/upload 内做远端路径解析;FFI 信任模型文档化;
- `72d978a` / `2965f64` docs:修正 stale `BatchTransferItemDto`/`TreeTransferDto` 注释。

### 6.6 待办清单(按优先级,截至 §7 记录)

1. ✅ FFI `hs_create_directory` + `hs_ping`(ABI 1.2.0,8122ca1);
2. ✅ `fs.rm` 迁移(DeleteResultDto 携带 FileEntryDto,fdf7025);
3. ✅ `fs.count` 迁移(runtime.count_files,d127c76);
4. ✅ Phase 7 脚本:`generate-ffi-header.sh`、`build-ffi-linux.sh`、`dist/apple/`
   产物(4724070);
5. ✅ `clipboard` 迁移(5d06a1e)、`media` 迁移(57529d3);
6. ⏳ `shell`/`batch`/`watch`/`sync` 评估保留 core(§7 边界记录,后续事项
   见 §7.3:事件桥接、sync 上移、device info/ping CLI 侧迁移);
7. ⏳ `handshaker-test-support` 拆分:评估为**保持 core 内部**——FakeWifiSsp
   依赖 `protocol::proto::*`(prost 生成)与 `protocol::crypto::KEY_TABLE`,
   拆分需以 feature 公开协议内部,违反 AGENTS.md §8「Prost 类型、线路帧和
   密码学细节保持 crate 内部可见」;当前亦无外部消费者(application/ffi 测试
   走无设备路径)。如需共享,先修订该约束或经 feature 显式 opt-in。

### 7.4 test-support 拆分评估

- 现状:`crates/handshaker-core/src/test_support.rs`(1179 行,`#[cfg(test)]`)
  被 core 内部 client/sync/exif_parser 测试使用(8 处引用,27 个 `pub(crate)`
  项);application/cli/ffi 测试均走无设备错误路径,不依赖 fake;
- 障碍:fake 构造/解析 SSP 消息依赖 `protocol::proto`(prost 生成)与
  `protocol::crypto::KEY_TABLE`,均为 `pub(crate)`;拆分需以
  `#[cfg(feature = "test-support")]` 公开 protocol 模块,与 AGENTS.md §8
  冻结约束冲突;
- 结论:保持 core 内部(与 M8 计划 §4.5「预留」一致)。前置条件:后续若
  application/ffi 测试需要 fake 设备,先经用户决策公开协议内部(或
  feature opt-in),再行拆分。

## 7. CLI 交互层边界评估(HEAD 8122ca1 之后,逐项落库)

> 结论先行:`fs` 全量(ls/stat/exists/mkdir/mv/count/rm/pull/push)、`clipboard`
> 全量、`media` 全量(photo/video/audio/thumbnail)已迁移;`shell`/`batch`/
> `watch`/`sync` 评估为**保留 core**,理由与边界如下。

### 7.1 已迁移命令(截至本记录)

| 命令 | 迁移方式 |
|---|---|
| `fs ls/stat/exists/mkdir/mv` | runtime 文件服务(8bb89c5) |
| `fs pull/push` | runtime batch_download/batch_upload(d7e516d;编排残留 `client.stat`/`file_exists` 仅作存在性判断) |
| `fs rm` | runtime.stat_file + runtime.delete_paths,DeleteResultDto 携带 FileEntryDto(fdf7025) |
| `fs count` | runtime.count_files(协议 exclusions 透传,d127c76) |
| `clipboard get/set/delete/clear` | runtime.list/set/delete/clear_clipboards(5d06a1e) |
| `media photo/video/audio/thumbnail` | runtime media 服务,DTO 镜像 core 字段(57529d3) |

### 7.2 保留 core 的命令与理由

- **`shell`(REPL)**:TTY 交互层(stdin/提示符/history/嵌套 shell 拒绝)。
  Application 是 UI/binding 无关服务层,不承载终端交互;保留 core 是设计
  决定,不是迁移欠账。
- **`batch`(stdin 单连接批量)**:CLI 编排层,逐行命令复用 REPL 解析与确认
  规则,底层命令已随各自迁移;无独立服务面。
- **`watch`(长连接事件监听)**:使用 `connect_with_all_callbacks`(core 连接
  层注册 photo/audio/video/device 回调);runtime.connect 无回调参数,
  Application 事件总线对 device 事件的桥接未做。CLI watch 走独立连接路径,
  不受影响。
- **`sync plan/run/watch/status`**:跨会话状态工作流(独立台账 `SyncStore` +
  core 状态机 `plan_diff`/`execute_plan`/`apply_file_change`),深度耦合 CLI
  目录/确认/输出;保留 core,后续可整体上移为 application sync 域服务。

### 7.3 后续边界事项

1. **事件桥接**:`BackendEvent::ClipboardChanged/MediaChanged/RemoteFileChanged`
   预留变体仍未桥接;需要 FFI/GUI 接收设备推送时,在 runtime.connect 增加
   回调注册(ConnectRequest 扩展属冻结契约变更,需 minor 决策);
2. **sync 上移**:如 GUI 需要同步能力,将 `SyncStore`+状态机包装为
   application 服务,CLI 仅做展示;
3. `device info/ping` 仍走 `session.client`(core);`ping` 的 FFI 面已补齐
   (hs_ping,ABI 1.2),CLI 侧可随后续 device 迁移一起处理。

## 8. M8.1 Phase A 契约止血记录(审计 c71cf94 之后)

> 依据 `docs/HandShaker_Rust_c71cf94_Swift_Delivery_Audit_and_Plan.md` Phase A;
> 审计指出 FFI ABI 版本三处不一致(Header/docs 仍写 1.1.0,create directory
> 标为未导出)与 Application v1「名义冻结、实际未冻结」问题。

### 8.1 FFI ABI 单一事实来源(ABI 1.2.0)

- `handshaker_ffi.h` 顶部注释 1.1.0 → 1.2.0,与 `ABI_VERSION_*` 常量一致;
- `docs/ffi-v1.md` 升为 v1.2:函数矩阵补 `hs_create_directory`/`hs_ping`,
  已导出/未导出清单修正(create 已导出;未导出改为 stat/move/delete、
  batch、媒体、剪贴板、信任、同步);
- 新增 `scripts/check-ffi-abi.py`:校验 Rust 导出与 Header 原型的符号、
  参数类别、返回类别一致,ABI 常量与 Header 注释一致,snapshot 同步;
- 新增 `docs/ffi-abi-snapshot.md`(23 个导出,由脚本生成,`--update` 刷新);
- `scripts/generate-ffi-header.sh` 接入上述检查(默认校验,`--update` 更新
  snapshot),保留 dist/apple/ staging;
- Swift smoke 增加 `hs_abi_version_minor()==2`/`patch()==0` 断言
  (C smoke 原已检查 major/minor);
- CI(macos-arm64.yml)在 release 构建后运行 ABI 检查与
  `scripts/run-ffi-smoke-tests.sh`(C + Swift)。

### 8.2 Application API 改为 preview

- `APPLICATION_API_VERSION`: `"1.0.0"` → `"1.0.0-preview.1"`;
- `docs/application-api-v1.md`:标题改为「preview 契约」,冻结规则标注为
  preview 期间的目标契约,列出正式冻结条件(移除 `session_client()`、
  事件/传输语义确定、fixture 完整、文档同步);
- 冻结前允许破坏性源码级修改并记录于此,不升 major。

### 8.3 尚未处理(后续 Phase)

- Phase B:Session Registry 锁跨 await、确定性 disconnect/shutdown、state_dir;
- Phase C/D/E:事件桥接、传输进度、FFI 功能扩展;
- 本记录仅覆盖 Phase A 契约与文档止血,不改变任何 Rust 行为代码
  (除 `APPLICATION_API_VERSION` 常量字符串)。
## 9. M8.1 Phase B 记录(Runtime 与并发模型修复)

> 依据审计文档 Phase B(B1–B5);全部为 Application/Core 行为代码变更。

### 9.1 配置真实生效(B4 state_dir / B5 wire_log)

- Core:`StateStore::from_dir(&Path)` 公开(状态文件位于 `config_dir/state.json`,
  `discover()` 保留);`HandShakerClient::connect_with_state` 公开为稳定入口;
- Application `connect()`:按 `RuntimeConfig.state_dir` 构造 StateStore
  (`Some` → `from_dir`,`None` → `discover()`,错误映射 `Configuration → InvalidState`);
- FFI `config_from_json`:`wire_log_utf8` 真正映射到 `RuntimeConfig.wire_log`
  (原解析后丢弃,固定 `None`);ABI 不变;
- 测试:`state_dir_controls_state_store_location`(connect 失败但 state.json
  落在指定 tempdir)、FFI `runtime_config_state_dir_and_wire_log_are_applied`、
  Core `from_dir_roots_state_file_in_config_dir`。

### 9.2 Registry 并发模型(B1)

- `ActiveSession` 改为 `Arc<ActiveSession>`(state 用 `AtomicU8` 存
  `SessionState` 判别值,`closing: CancellationToken`);Registry 存
  `HashMap<SessionId, Arc<ActiveSession>>`;
- 全部网络方法统一"短临界区 clone client Arc → 释放 Registry 锁 → await":
  `list_files/count_files/stat_file/create_directory/move_path/delete_paths`
  (clipboard/media 原本已合规);`get_session_snapshot`/`session_client` 保持短锁;
- 私有 `session_client_arc` 为内部标准路径,公共 `session_client`(过渡 API)
  委托之;
- 测试:`concurrent_requests_do_not_hold_the_registry_lock_across_await`。

### 9.3 确定性 Session 关闭(B2)

- `TransferRegistry::cancel_for_session(session_id)`:取消该 Session 全部
  transfer 并收集 join handle(按 snapshot.session_id 过滤,不维护易漂移的
  集合);
- `Runtime::disconnect` 重写,共享私有 `close_session`:
  原子进入 `Disconnecting`(Closed/Failed 终态幂等)→ `closing.cancel()` →
  取消 transfers → 有界等待 join(`SESSION_CLOSE_DEADLINE = 5s` 常量,
  不新增配置字段)→ `Arc::try_unwrap` 成功则显式 `close()`(发 QUIT)→
  无条件发布 `SessionStateChanged(Closed)` → 清理异常(close 失败/仍有
  持有者)发布 `Warning` 事件并返回 Ok(partial success 可观察,不吞错);
- CLI `close_session`(drop client + disconnect)保持不变,与新路径兼容;
- 测试:`cancel_for_session_only_cancels_that_sessions_transfers`、
  `cancel_is_idempotent`。

### 9.4 确定性 shutdown 与 EventHub close(B3)

- `EventHub`:sender 改 `Mutex<Option<Sender>>`;`close()` drop sender 后
  全部 receiver 收 `Closed`(此前 sender 随 `Arc<RuntimeInner>` 永存,
  订阅者永远收不到 Closed);关闭后 `publish` 静默、`subscribe` 返回
  立即 Closed 的 receiver;
- `Runtime::shutdown` 重写:`compare_exchange` 保证只执行一次(并发/重复
  调用返回 Ok)→ `RuntimeStopping` → 取消全部 transfers → 并行执行
  `close_session`(join 证明任务结束)→ `EventHub::close()`;删除固定 50ms
  sleep;
- FFI `hs_subscription_next` 在 shutdown 后返回 `{"closed":true}`;
- 测试:`subscription_receives_closed_after_shutdown`、
  `concurrent_shutdown_runs_once_and_closes_events`(app)、
  `subscription_observes_closed_after_runtime_shutdown`(ffi)。

### 9.5 遗留与后续

- 下载取消导致 Session 失效的 `Failed` 状态发布、transfer 进度事件、
  transfer history 容量边界属 Phase C,未在本阶段处理;
- `session_client()` 过渡 API 仍在(移除属冻结前收口,见 §8.2 条件);
- trust list/remove/reset 的 state_dir 注入属 Phase D TrustService,
  CLI 仍走 `StateStore::discover()`。
## 10. M8.1 Phase C 记录(事件与传输模型,传输侧)

> 依据审计文档 Phase C(C2/C3/C4);C1(Core 事件桥接)与 C5(通用连接丢失
> 事件)未完成,见 §10.3。

### 10.1 传输进度事件(C2)

- progress 回调携带 `TransferProgress.total`,`set_progress` 同时写入
  `transferred_bytes` 与 `total_bytes`(此前 total 被丢弃,进度条无法确定);
- 事件节流:每条目 `ProgressThrottle`(100ms / 256KiB 阈值),
  `TransferUpdated` 事件流有界(~10-20/s),终态无条件发布;
- `TransferRegistry` 持有 Runtime 的 `EventHub` 克隆(同一事件序列)。

### 10.2 取消语义(C3)与有界历史(C4)

- `cancel()` 立即置 `Cancelled` + `finished_at_ms` 并发布 `TransferUpdated`
  事件(不等待后台任务);`transition` 对 Cancelled 同样记录 finished_at;
  终态单向不可覆盖(既有 guard);
- `from_core_error` 按 `CancellationOrigin` 区分:
  `Local → TransferCancelled(4202)` / `Remote → RemoteCancelled(4203)`;
- 任务闭包检测 `transfer_closed_the_session`(`Cancelled.connection_closed`
  或 `Transport` 错误)后 `mark_session_failed`:Session `Ready → Failed`
  并发布 `SessionStateChanged`,GUI 不再使用已失效会话;
- `RuntimeConfig` 新增 `transfer_history_capacity`(默认 64,淘汰最老
  finished,live 任务永不淘汰)与 `transfer_history_ttl`;
  `TransferRegistry::new(event_hub, capacity, ttl)`;FFI 配置新增
  `transfer_history_capacity`/`transfer_history_ttl_ms`(ABI 不变);
- CLI/tests 构造点补齐新字段。

### 10.3 遗留

- C1(Core typed event 桥接:clipboard/media/remote-file/device 主动推送)
  与 C5(请求级连接丢失统一检测)仍为后续阶段;
- batch 传输仍无逐文件进度事件(CLI 批量面设计如此,任务面 start_* 有)。

### 10.4 真机发现并修复:macOS provenance 权限

- 现象:`state.json` 上 `fs::set_permissions` 返回 EPERM,所有连接失败;
- 根因:macOS 14+ 的 `com.apple.provenance` xattr 使 `fchmodat`(Rust std
  `fs::set_permissions` 所用)即使权限相同也返回 EPERM(`/bin/chmod` 的
  `chmod(2)` 不受限;python os.chmod 同样失败);
- 修复:`ensure_permissions` 先比对当前 mode,已符合(0600/0700)时跳过
  chmod;权限不符时仍显式设置并如实报错;新文件由 `OpenOptions.mode`
  直接以 0600 创建;
- 测试:`ensure_permissions_skips_when_mode_already_matches`。

### 10.5 真机验收(ADB,Smartisan U2 Pro / OD103,2026-08-03)

- 基础:device info/ping(3ms)/fs ls 正常;唯一测试目录
  `HandShakerTest_PhaseC_<ts>` 创建、3MB+24B 上传、下载 MD5 一致
  (`0b63945931d22d284d080356ff945dce`)、mv/rm、剪贴板 set/get;
- 传输进度:30MB 下载 `total_bytes=31457280`,节流事件 108 个
  (< 256KiB 阈值上限),终态 Completed 无条件发布;
- 传输中取消:立即 `Cancelled` + `finished_at_ms`,随后
  `SessionStateChanged(Failed)`(下载取消关闭 Core 会话);
- 确定性关闭:Failed 会话 disconnect 幂等清理,无残留 adb forward;
- 测试目录与本地临时文件全部清理。
