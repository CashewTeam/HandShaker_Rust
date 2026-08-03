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
- 测试 186 → 192(core 120 / bin 22 / cli 12 / app 22 / ffi 15 / localization 1)。

## 6. 全量核对记录(HEAD 2965f64,工作树干净)

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

