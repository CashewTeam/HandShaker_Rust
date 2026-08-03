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
