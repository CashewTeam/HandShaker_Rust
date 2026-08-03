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

- CLI 其余命令(fs/clipboard/media/sync/watch/batch/shell)仍直连 core,
  按文档 Phase 3 渐进迁移(下一阶段:fs 命令族 → 传输/事件/同步);
- `handshaker-test-support` crate 尚未拆分(当前 core 内部 `#[cfg(test)]`),预留;
- Application 的 Clipboard/Media/RemoteFile 事件为预留变体,未桥接;
- FFI 未导出传输任务与文件变更方法(见 `docs/ffi-v1.md §5`)。
