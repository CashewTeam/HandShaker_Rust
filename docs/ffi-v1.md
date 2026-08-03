# handshaker-ffi v1(C ABI 契约)

> ABI 版本:1.0.0(与 Rust crate 版本独立;major=签名破坏、minor=增函数/可选字段、patch=实现修复)
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
| `hs_abi_version_major/minor/patch` | ABI 版本 1.0.0 |
| `hs_byte_buffer_free` / `hs_call_result_free` | 释放 |
| `hs_runtime_create(config_json, len, out_runtime)` | 创建(JSON: `adb_path_utf8/default_timeout_ms/heartbeat_interval_ms/state_dir_utf8/event_capacity`, 均可选) |
| `hs_runtime_shutdown(runtime)` | 幂等;NULL → 成功 |
| `hs_runtime_destroy(runtime)` | 保守清理;NULL 安全 |
| `hs_list_devices(runtime, request_json, len)` | 结果:DeviceDescriptor 数组 |
| `hs_connect(runtime, device_json, len)` | 结果:`{"session_id":N}` |
| `hs_disconnect(runtime, session_id)` | 结果:`{"disconnected":true}` |
| `hs_get_session(runtime, session_id)` | 结果:SessionSnapshot |
| `hs_list_files(runtime, session_id, request_json, len)` | 结果:FileEntryDto 数组 |
| `hs_subscribe_events(runtime, out_subscription)` | 订阅 |
| `hs_subscription_next(subscription, timeout_ms)` | EventEnvelope JSON;超时 `{"timeout":true}`;关闭 `{"closed":true}`;Lagged → 错误 |
| `hs_subscription_destroy(subscription)` | NULL 安全 |

## 3. 安全与并发语义

- 每个 extern 函数 `catch_unwind`:panic → `Internal` 错误,不跨 ABI unwind;
- NULL 输入/输出槽 → `InvalidArgument` 稳定错误,不崩溃;
- 短操作在 Runtime 的 tokio executor 上 `block_on`,阻塞调用线程;
  **调用方必须在后台线程调用**(Swift 主线程禁止);
- 长任务(传输)用 ID + `get_transfer`/事件轮询,不在 v1 引入跨语言回调;
- 事件订阅为队列拉取,固定缓冲,`Lagged` 显式上报。

## 4. Swift 接入

- `include/module.modulemap` 提供模块 `HandShakerFFI`;
- 构建:`scripts/build-ffi-macos.sh`(stage 到 `dist/apple/`);
- 冒烟:`scripts/run-ffi-smoke-tests.sh`(C + Swift);
- Swift 层结构建议:`HandShakerCore/Native/`(RuntimeHandle RAII、
  NativeCall、NativeError)+ `Models/`(Codable DTO)+ `HandShakerClient.swift`(
  `protocol BackendClient: Sendable`);SwiftUI View 不直接触碰 C 类型。

## 5. v1 已导出 vs 未导出

- 已导出:Runtime 生命周期、设备列表、连接/断开/快照、文件列表、事件订阅;
- 未导出(v1 后按需追加,minor):传输任务(start_download/upload/cancel/get)、
  stat/create/move/delete、媒体、剪贴板、同步;
- 大文件字节**不**经过 JSON/FFI(Rust 直接写文件,任务 ID + 进度事件)。
