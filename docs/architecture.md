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

## 3. 应用服务模型(preview v1)

- `HandShakerRuntime`(非单例,可多实例):create/shutdown(幂等,单次执行,
  确定性关闭:取消传输 → 有界 join → 并行关闭 Session → 关闭 EventHub)、
  list_devices/discover_devices(分通道 warnings)、connect/disconnect/
  get_session_snapshot、list_files、stat_file/create_directory/move_path/
  delete_paths、start_download/start_upload/cancel_transfer/get_transfer/
  list_transfers、monitor_folder、subscribe_events;
- Phase D 业务服务:`list_trust_records/remove_trust_record/
  reset_wifi_trust`(state_dir 真实生效)、`plan_download/plan_upload/
  execute_file_plan`(六类 FileConflictKind,可取消,transport 级失败标记
  连接丢失)、SyncService(`plan_sync/start_sync/get_sync_status/stop_sync/
  start_sync_watch/stop_sync_watch/last_sync_result/sync_ledger_status`,
  ledger 位于 `<state_dir>/sync/<device_uuid>.json`,watch 批次经
  `BackendEvent::SyncWatchApplied` 发布);
- `SessionId(u64)`/`TransferId(u64)` 单调分配;长任务后台执行 + 事件轮询;
- 配置真实生效:`state_dir` 控制信任记录/host UUID/sync ledger 位置(缺省
  Core 默认目录;CLI 提供 `--state-dir`),`wire_log` 真实开启线路日志
  (P2-4:默认关闭/header-only,payload 需显式 opt-in,64 MiB 轮转);
  Registry 锁不跨网络 await(短临界区 clone client);
- 事件:`EventEnvelope { sequence, timestamp_ms, event }`,broadcast,
  Lagged 显式上报;Runtime shutdown 后订阅流以 `Closed` 结束;
  v1 完整实现 Runtime/Session/Transfer 事件;Core typed events 桥接
  (M8.1 Phase C/C1):`DeviceUpdated`/`ClipboardChanged`/`MediaChanged`/
  `RemoteFileChanged` 携带 DTO payload(`RemoteFileChangeDto` 含
  files/statuses 完整元数据),未知事件安全 `Warning`;请求或
  传输发现连接丢失时发布 `ConnectionLost` 并将 Session 置 `Failed`(C5);
- 错误:`PublicError { code, message, detail, retryable, operation }`,
  分区码 1001–9001,`code` 是唯一程序判断依据。

## 4. FFI 契约(v1)

见 `docs/ffi-v1.md`。要点:ABI 1.5.0 独立版本(Rust 常量/Header/文档/snapshot
由 `scripts/generate-ffi-header.sh` 校验一致);Rust 分配 Rust 释放;
所有函数 catch panic;NULL 句柄稳定报错;短操作同步阻塞调用线程
(调用方在后台线程);事件队列拉取,无跨语言回调。

## 5. 平台与已知限制

- 协议能力与 M7 一致(ADB/WiFi/USB、文件、剪贴板、媒体、同步、watch);
- USB accessory 会话单次性、Linux udev、Windows 评估均未变(见 docs/23);
- CLI 业务命令已全部迁移到 Application(M8.1 Phase D 收口:device
  info/ping、trust、pull/push 预检、watch、sync.* 均走
  `HandShakerRuntime`;`session_client()` 过渡入口已删除,`AppSession`
  不再持有 Core client)。仅剩 `device discover`(Wi-Fi mDNS)直连 core,
  `fs rm/count` 输出适配保留在 CLI(见 `docs/m8-migration.md` §4/§7);
- FFI 已导出 52 个符号(ABI 1.5.0,`docs/ffi-v1.md`/`docs/ffi-abi-snapshot.md`
  同步):设备/会话/文件/传输/事件/剪贴板/信任/监控/批量传输/媒体
  (library 分页 + 磁盘缓存缩略图 + EXIF)/sync/update_file_info/
  media_merge/diagnostics;`json_contract=1` 版本化 JSON 契约;
  destroy 并发契约写入 Header;Apple 产物由
  `scripts/build-ffi-macos.sh` 生成 **arm64+x86_64 universal +
  静态 libusb**(无动态依赖,可公证/上架)。
