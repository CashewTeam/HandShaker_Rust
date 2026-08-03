# M8 测试报告

> 基线:0.6.1(154 测试);M8 完成:0.7.0(186 测试)

## 1. 回归(全部通过)

| 目标 | 数量 | 说明 |
|---|---|---|
| core(lib) | 120 | framing/handshake/transport/event/cancel/sync/media/transfer 等原样保留 |
| bin | 21 | 二进制单元测试 |
| cli | 12 | 含 `device_list_reads_only_adb_devices_long`(JSON/human 行不变、仅 `adb devices -l`)与 batch/shell 回归 |
| application | 20 | Runtime 生命周期、会话/传输注册表、错误映射、路径规则、serde 契约、事件 lag/shutdown |
| ffi | 12 | buffer 所有权、result JSON、panic→Internal、NULL/invalid 输入、runtime 生命周期、ABI 版本 |
| localization | 1 | 全 workspace src 无 CJK 硬编码 |
| **合计** | **186** | |

执行方式(沙箱):`cargo build --tests` + 直接运行 `target/debug/deps/` 各测试二进制。
`cargo fmt -- --check` FMT_OK;`git diff --check` DIFF_OK;release 构建成功(0 警告)。

## 2. 新增测试覆盖

- **Application**:`runtime_create_and_shutdown_is_idempotent`、
  `operations_after_shutdown_return_runtime_closed`、`unknown_session_errors_are_stable`、
  `core_errors_map_to_stable_codes`、`resolve_remote_path_rules`、
  `device_descriptor_json_contract_is_stable`、`public_error_json_contract_is_stable`、
  `unknown_enum_values_are_rejected_not_guessed`、`transfer_state_transitions_are_one_way`、
  `cancel_transfer_is_idempotent_...`、`event_hub_sequences_and_lags`、
  `subscribe_after_shutdown_still_gets_runtime_stopping`、`transfer_snapshot_json_contract_is_stable`;
- **FFI**:`buffer_round_trip_preserves_bytes`、`empty_buffer_is_null_and_free_is_safe`、
  `free_reclaims_allocation`、`ok/err_result_has_*_json`、`catch_converts_panic_to_internal`、
  `input_str_rejects_null_and_invalid_utf8`、`runtime_lifecycle_create_list_shutdown_destroy`、
  `null_handles_return_errors_not_crashes`、`invalid_json_config_is_rejected`、
  `abi_version_is_1_0_0`;
- **C smoke**:`scripts/ffi_smoke.c`(clang 编译链接运行,`ffi smoke ok`);
- **Swift smoke**:`scripts/ffi_smoke.swift`(swiftc 编译运行,`swift ffi smoke ok`)。

## 3. 测试中发现并修复的问题

1. tokio `broadcast` Lagged 语义:Lagged 后 resume 位置为最旧保留槽(非最新)——测试断言修正;
2. std `Mutex` 非重入:`cancel` 持锁调用 `transition` 死锁 → 先释放注册表锁再转换;
3. FFI 测试 `into_vec` 后 `free_result` double-free(allocator 崩溃)→ 测试按所有权修正,
   生产 `hs_byte_buffer_free` 契约文档化;
4. FFI 函数对 NULL 句柄直接解引用(UB)→ 新增 `runtime_ref` 检查返回 `InvalidArgument`;
5. edition 2024 `#[no_mangle]` → `#[unsafe(no_mangle)]`;`unsafe_op_in_unsafe_fn` 显式 allow
   (FFI 入口集中审阅)。

## 4. 未运行/未覆盖

- 沙箱拦截 `cargo test`/`cargo clippy` 直接运行;CI 上仍应跑全量;
- 真机 FFI 冒烟(ADB 连接 → list files)未做:需用户授权与设备在场;
- C/Swift smoke 只在 macOS ARM64 本机验证;Linux/Windows 未跑;
- Address Sanitizer / Thread Sanitizer / Miri 未在本轮运行(文档 §11.7 建议项)。
