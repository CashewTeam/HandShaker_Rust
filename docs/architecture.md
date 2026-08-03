# 架构(M8)

> 版本 0.7.0 · 分支 `refactor/m8-workspace-application-ffi`

## 1. Workspace 分层

```text
handshaker-core
        ↑
handshaker-application
        ↑
 ┌──────┼────────────────┐
CLI    FFI        GTK Rust(未来)
```

| crate | 职责 | 依赖 |
|---|---|---|
| `handshaker-core` | SSP framing、protobuf、ADB/WiFi/USB 传输、握手信任、Session、设备/文件/媒体/批量/同步/剪贴板、事件解码、State/SyncStore、低层取消 | 无(内部) |
| `handshaker-application` | Runtime 生命周期、设备发现目录、Session/Transfer 注册表、事件 Hub、公共错误、稳定 DTO、路径解析 | core |
| `handshaker-cli` | Clap 命令树、本地化、human/JSON/JSONL 输出、REPL/shell/batch、确认、调用 Application | application → core |
| `handshaker-ffi` | 稳定 C ABI、不透明句柄、Buffer/Result、panic 隔离、事件拉取、ABI 版本、C/Swift smoke | application |
| `handshaker-test-support` | (预留)假 ADB/SSP、fixture 共享 | 不参与生产 |

禁止方向:core → application/cli/ffi;application → cli/ffi;ffi → cli。

## 2. 数据流

```text
SwiftUI / GTK / .NET
  ↓
handshaker-ffi(C ABI) / handshaker-application(Rust)
  ↓
handshaker-application(业务语义、注册表、错误、事件)
  ↓
handshaker-core(协议、传输、会话)
```

- CLI 保留为自动化/调试/无 GUI 入口,输出模型(JSON envelope、退出码)留在 CLI;
- GUI 只通过 Application/FFI 消费,不接触 Prost、sid、帧、forward 清理。

## 3. 应用服务模型(冻结 v1)

- `HandShakerRuntime`(非单例,可多实例):create/shutdown(幂等)、
  list_devices、connect/disconnect/get_session_snapshot、list_files、
  stat_file/create_directory/move_path/delete_paths、start_download/
  start_upload/cancel_transfer/get_transfer/list_transfers、subscribe_events;
- `SessionId(u64)`/`TransferId(u64)` 单调分配;长任务后台执行 + 事件轮询;
- 事件:`EventEnvelope { sequence, timestamp_ms, event }`,broadcast,
  Lagged 显式上报;v1 完整实现 Runtime/Session/Transfer 事件,
  Clipboard/Media/RemoteFile 预留;
- 错误:`PublicError { code, message, detail, retryable, operation }`,
  分区码 1001–9001,`code` 是唯一程序判断依据。

## 4. FFI 契约(v1)

见 `docs/ffi-v1.md`。要点:ABI 1.0.0 独立版本;Rust 分配 Rust 释放;
所有函数 catch panic;NULL 句柄稳定报错;短操作同步阻塞调用线程
(调用方在后台线程);事件队列拉取,无跨语言回调。

## 5. 平台与已知限制

- 协议能力与 M7 一致(ADB/WiFi/USB、文件、剪贴板、媒体、同步、watch);
- USB accessory 会话单次性、Linux udev、Windows 评估均未变(见 docs/23);
- CLI fs/clipboard/media/sync/watch 命令仍直连 core(渐进迁移中,
  `device list` 已走 Application);FFI 导出为最小闭环(设备/会话/文件/事件),
  传输任务 API 已就绪未导出。
