# handshaker-ffi v1.5(C ABI 契约)
> ABI 版本:1.5.0(与 Rust crate 版本独立;major=签名破坏、minor=增函数/可选字段、patch=实现修复)
> 1.5 追加 update file info(`hs_update_file_info`)与媒体增量合并(`hs_media_merge_change`);
> 1.4 追加照片同步面(`hs_sync_plan/start/status/stop/start_watch/stop_watch`);
> 1.3 追加文件 stat/count/move/delete、剪贴板、信任、设备发现、目录监控、
> 批量传输、媒体库/缩略图/EXIF 与运行时诊断;1.2 追加 `hs_create_directory`
> 与 `hs_ping`;1.1 追加传输任务面(`hs_transfer_*`),1.0 符号不变。
> 单一事实来源:`crates/handshaker-ffi/src/lib.rs` 的 `ABI_VERSION_*` 常量与
> `crates/handshaker-ffi/include/handshaker_ffi.h` 顶部注释;`scripts/generate-ffi-header.sh`
> 校验两者与 ABI snapshot(`docs/ffi-abi-snapshot.md`)一致。
> crate:`crates/handshaker-ffi`,产物:`libhandshaker_ffi.{a,dylib,rlib}`(macOS)

## 1. 基础类型

```c
typedef struct { uint8_t* ptr; size_t len; size_t capacity; } HsByteBuffer;
typedef struct { int32_t status; HsByteBuffer value; HsByteBuffer error; } HsCallResult;
typedef struct HsRuntime HsRuntime;
typedef struct HsSubscription HsSubscription;
```

- `status == 0` 成功:`value` 为 UTF-8 JSON,`error` 空;
  失败:`error` 为 `PublicError` JSON(`{code,message,detail,retryable,operation}`),`value` 空;
- 空 buffer = `{ NULL, 0, 0 }`;`hs_byte_buffer_free`/`hs_call_result_free` 对其安全;
- **所有权:Rust 分配只能由 Rust 释放**(`hs_byte_buffer_free`);调用方不得改 `capacity`;
  double-free 是调用方错误(文档约定,不检测)。

## 2. 导出函数

| 函数 | 说明 |
|---|---|
| `hs_abi_version_major/minor/patch` | ABI 版本 1.5.0 |
| `hs_byte_buffer_free` / `hs_call_result_free` | 释放 |
| `hs_runtime_create(config_json, len, out_runtime)` | 创建(JSON: `adb_path_utf8/default_timeout_ms/heartbeat_interval_ms/state_dir_utf8/event_capacity`, 均可选) |
| `hs_runtime_shutdown(runtime)` | 幂等;NULL → 成功 |
| `hs_runtime_destroy(runtime)` | 保守清理;NULL 安全 |
| `hs_list_devices(runtime, request_json, len)` | 结果:DeviceDescriptor 数组 |
| `hs_connect(runtime, device_json, len)` | 结果:`{"session_id":N}` |
| `hs_disconnect(runtime, session_id)` | 结果:`{"disconnected":true}` |
| `hs_get_session(runtime, session_id)` | 结果:SessionSnapshot |
| `hs_list_files(runtime, session_id, request_json, len)` | 结果:FileEntryDto 数组 |
| `hs_create_directory(runtime, session_id, request_json, len)` | 结果:`{"created":true}`(ABI 1.2) |
| `hs_ping(runtime, session_id)` | 结果:`{"round_trip_ms":N}`(ABI 1.2) |
| `hs_transfer_start_download(runtime, session_id, request_json, len)` | 结果:`{"transfer_id":N}`(ABI 1.1) |
| `hs_transfer_start_upload(runtime, session_id, request_json, len)` | 结果:`{"transfer_id":N}`(ABI 1.1) |
| `hs_transfer_cancel(runtime, transfer_id)` | 结果:`{"cancelled":true}`(ABI 1.1) |
| `hs_transfer_get(runtime, transfer_id)` | 结果:TransferSnapshot(ABI 1.1) |
| `hs_transfer_list(runtime)` | 结果:TransferSnapshot 数组(ABI 1.1) |
| `hs_subscribe_events(runtime, out_subscription)` | 订阅 |
| `hs_subscription_next(subscription, timeout_ms)` | EventEnvelope JSON;超时 `{"timeout":true}`;关闭 `{"closed":true}`;Lagged → 错误 |
| `hs_subscription_destroy(subscription)` | NULL 安全 |

## 3. 安全与并发语义

- 每个 extern 函数 `catch_unwind`:panic → `Internal` 错误,不跨 ABI unwind;
- NULL 输入/输出槽 → `InvalidArgument` 稳定错误,不崩溃;
- 短操作在 Runtime 的 tokio executor 上 `block_on`,阻塞调用线程;
  **调用方必须在后台线程调用**(Swift 主线程禁止);
- 长任务(传输)用 ID + `get_transfer`/事件轮询,不在 v1 引入跨语言回调;
- 事件订阅为队列拉取,固定缓冲,`Lagged` 显式上报;
- **JSON 契约独立版本(P1-7)**:ABI 版本只保证符号/签名;请求、响应、事件、
  DTO 的 JSON 形状由 `hs_runtime_diagnostics` 的 `json_contract` 字段版本化
  (当前 `= 1`,定义于 `handshaker_application::JSON_CONTRACT_VERSION`)。Swift
  在创建 Runtime 时校验 `json_contract >= 1`;JSON breaking change 必须递增
  该版本并同步 Swift 模型(`RuntimeDiagnostics.minimumJSONContract`),而不是
  只动 ABI 版本。
- **单租户信任模型**:ABI 是进程内契约,调用方与库同权限。Session/Transfer 的
  `u64` id 是顺序计数器(非不透明随机句柄),同进程调用方可枚举/猜测 id;库不
  做调用方鉴权。多租户隔离需要宿主进程自行分区(或引入随机 id,破坏 v1 稳定性)。

## 4. Swift 接入

- `include/module.modulemap` 提供模块 `HandShakerFFI`;
- 构建:`scripts/build-ffi-macos.sh`(stage 到 `dist/apple/`);
- 冒烟:`scripts/run-ffi-smoke-tests.sh`(C + Swift);
- Swift 层结构建议:`platform/macos/HandShakerCore/Native/`(RuntimeHandle RAII、
  NativeCall、NativeError)+ `Models/`(Codable DTO)+ `HandShakerClient.swift`(
  `protocol BackendClient: Sendable`);SwiftUI View 不直接触碰 C 类型。

## 5. v1.5 已导出 vs 未导出

- 已导出(共 52 个符号,Phase E + sync + update file info/media merge 后):
  - v1.0/1.1/1.2/1.3:Runtime 生命周期与诊断(`hs_runtime_diagnostics`,1.3)、
    设备列表与发现(`hs_list_devices`/`hs_discover_devices`,1.3)、
    连接/断开/快照、文件列表、`hs_create_directory`/`hs_ping`(1.2)、
    传输任务(start_download/upload/cancel/get/list,1.1;
    `hs_transfer_start_batch_download/upload`,1.3)、事件订阅;
  - v1.3:文件(`hs_stat_file`/`hs_count_files`/`hs_move_path`/
    `hs_delete_paths`)、剪贴板(`hs_clipboard_list/set/delete/clear`)、
    信任(`hs_trust_list/remove/reset`)、目录监控(`hs_monitor_folder`)、
    媒体(`hs_media_photo_library/video_library/audio_library/thumbnail/
    fetch_exif`)、诊断(`hs_runtime_diagnostics`);
  - v1.4(照片同步):`hs_sync_plan`/`hs_sync_start`(后台运行,立即返回
    profile_id)/`hs_sync_status`/`hs_sync_stop`/`hs_sync_start_watch`/
    `hs_sync_stop_watch`——进度用 status 轮询或事件订阅
    (`SyncWatchApplied`/`TransferUpdated`/`Warning`);编排由调用方完成
    (plan → start → poll/events → start_watch → stop_watch/stop),每个
    调用都是短调用;
  - v1.5:文件元数据更新(`hs_update_file_info`,UPDATE_FILE_INFO)与
    媒体增量合并(`hs_media_merge_change`——纯函数,kind photo|video|audio,
    库快照 + 手机推送 change → 合并后快照;不需要设备);
- 未导出(按需追加,minor):无(MVP 功能面已齐);
- 大文件字节**不**经过 JSON/FFI(Rust 直接写文件,任务 ID + 进度事件);
- 缩略图 bytes 经 FFI 写入 `<state_dir>/thumbnails/` 磁盘缓存并返回
  `cache_path`(不经过 JSON 数字数组;缓存文件按设备+路径 hash 命名,
  已存在则复用——全命中时跳过设备往返;无 TTL,幂等覆盖 + 原子写)。
