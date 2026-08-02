# 19 M3：目录监控与设备/剪贴板主动推送（设计与实现记录）

> 状态基线：2026-08，Cargo package `handshaker_rust 0.2.0`。
> 本文记录 M3 的实现范围、协议依据、事件 schema 契约、公开 API 与验证结果；
> 行为细节以源码与自动化测试为准。

## 1. 目标与范围

M3 在 M1 事件总线（强类型 `ClientEvent` 解码与广播）之上补齐发送侧与实时消费：

- library：`monitor_folder(path, register)` —— 发送 `MONITOR_FOLDER_REQUEST(23)`
  注册/注销目录监控，并处理手机端确认 `MONITOR_FOLDER_RESPONSE_HEADER(24)`；
- CLI：`watch` 命令 —— 连接设备后可选注册监控（`--path` 可重复），订阅全部主动推送，
  实时输出到 stdout（human 或 jsonl），Ctrl-C 优雅注销后退出；
- 事件 schema 契约：`ClientEvent` 的 JSON `kind` tag 与 `watch` jsonl envelope 固定
  （0.2.0 兼容承诺）。

不在本里程碑范围：`SYNC_MONITOR_REQUEST(39)`/`PHOTO_SYNC_REQUEST(37)` 的发送侧
（其事件解码 M1 已就绪，`watch` 会输出它们）；`UPDATE_FILE_INFO(40/41)`；
`FILE_CHANGE(38)` 仅作为推送事件被 `watch` 输出，不主动请求。

## 2. 协议依据

> ⚠️ 验证等级：**APK 反编译推断**（docs/08 §8.10），无真实抓包向量；真机行为见 §6 验收。

- 请求：`SSPMonitorFolderRequest { type=23, file, register }`；
  `register=true` 开始监控，`false` 停止。
- 确认：`SSPMonitorFolderResponseHeader { type=24, succeed, error_message }`；
  走普通 sid 请求-响应路径。
- 事件：`SSPMonitorFolderResponse { type=25, repeated event:SSPFileEvent }`，
  `SSPFileEvent = { file:SSPFile, event:SSPFileEventType }`；
  Android 端为 FileObserver（mask 0xFC8）事件，映射见 docs/08 表
  （CREATE→1、DELETE→2、CLOSE_WRITE→3、MOVED_FROM→4、MOVED_TO→5、
  DELETE_SELF→6、MOVE_SELF→7、DIR_CHANGED→8）。
- 剪贴板推送：`SSPClipboardChange { type=30, repeated clipboard }`（手机剪贴板变化时）。
- 校验规则（`d/c.java:552-582`）：仅普通目录路径；SD 未拔出、非 `/system`、有写权限。

## 3. 实现

### 3.1 library API

```rust
pub async fn monitor_folder(&self, path: &str, register: bool) -> Result<()>
pub async fn monitor_folder_with_options(
    &self, path: &str, register: bool, options: RequestOptions,
) -> Result<()>
```

- 构造 `SSPMonitorFolderRequest` 经 `session.request_with_options` 发送，
  解码 `SSPMonitorFolderResponseHeader`；`succeed=false` 时按手机端
  `error_message` 映射 `Error::RemoteIo`（退出码 6）。

### 3.2 CLI `watch`

```
handshaker [--wifi IP:PORT | --serial SN] watch [--path DIR]... [--output human|jsonl]
```

- 每个 `--path` 先 `monitor_folder(path, true)` 注册，任一路径失败则回滚已注册项并报错；
- 订阅 `EventFilter::all()`，流式输出：
  - human：每行一个事件的紧凑 JSON（`serde_json::to_string(&ClientEvent)`）；
  - jsonl：完整 envelope（见 §4）；
- 事件流 `Lagged` → stderr 提示并继续；`Closed` → `Error::Transport`（不自动重连）；
- Ctrl-C：逐个 `monitor_folder(path, false)` 注销 → 返回 `Error::Interrupted`（退出码 130）；
  `main.rs` 对 `watch` 不包顶层 ctrl_c select（由 watch 自管清理）。

## 4. 事件 schema 契约（0.2.0）

- `ClientEvent` 序列化为 `{"kind": "<tag>", "data": {...}}`，`kind` tag 固定 snake_case：

| EventKind | kind tag |
|---|---|
| DeviceInfoChanged | `device_info_changed` |
| ClipboardChanged | `clipboard_changed` |
| MediaLibraryChanged | `media_library_changed` |
| DirectoryChanged | `directory_changed` |
| FileChanged | `file_changed` |
| PhotoSyncChanged | `photo_sync_changed` |
| SyncMonitorChanged | `sync_monitor_changed` |
| RequestCancelled | `request_cancelled` |
| Unknown | `unknown` |

- `watch` jsonl 每行信封（`schema_version` 保持 `1`，结构与其他命令一致）：

```json
{"schema_version":1,"ok":true,"command":"watch","device":{"serial":"...","name":"..."},
 "event":"watch","data":{"kind":"directory_changed","data":[...]},"warnings":[]}
```

- **兼容承诺**：`kind` tag、`schema_version`、信封键（`command`/`device`/`event`/`data`/
  `ok`/`warnings`）为 0.2.0 稳定契约；变更需大版本。新增事件类型只追加 tag，不改既有 tag。

## 5. 公开 API 变更（相对 0.1.5）

- 新增：`HandShakerClient::monitor_folder`、`monitor_folder_with_options`；
  CLI `watch` 子命令（含 `--path`）。
- 无破坏性变更：既有 API、JSON envelope、错误 code 与退出码语义不变。
- 版本从 `0.1.5` 升至 `0.2.0`（功能里程碑，按 plan.md §9 由维护者决定）。

## 6. 测试与验收

- 单元/集成：`wifi_monitor_folder_registers_and_unregisters`、
  `wifi_monitor_folder_rejection_is_reported`（RemoteIo/退出码 6）；
  `watch_accepts_repeatable_paths`、`watch_envelope_carries_event_payload`、
  `watch_help_is_localized`；`all_event_kinds_serialize_to_stable_snake_case_tags`、
  `directory_and_file_events_round_trip_with_stable_kinds`。
- 真机验收：见 §6.1（验收后填写）。

### 6.1 真机验收（2026-08，Smartisan OD103，Android 7.1.1）

> 环境：Mac 与手机（192.168.2.47）同一局域网；隔离 HOME（全新 host_uuid）；手机端
> 首次连接弹信任对话框，点击"信任"（TRUST_ALWAYS）后注册监控。

| 验收项 | 结果 |
|---|---|
| watch 注册 | ✅ `watch --path /storage/emulated/0/hs_m3_test` 注册成功，stderr 提示"目录监控已注册" |
| 文件事件推送 | ✅ adb 创建/写入/移动文件 → watch 依次输出 `directory_changed` 事件：`create`(delta.txt) → `close_write`(delta.txt) → `create`(sub/) → `moved_from`(delta.txt)，事件类型与 docs/08 FileObserver 映射一致 |
| 剪贴板推送 | ✅ 手机端复制文本 → watch 输出 `clipboard_changed` 事件（data 含新条目 timestamp_ms 与旧历史） |
| jsonl 信封 | ✅ 每行 `{schema_version:1, ok:true, command:"watch", device:{serial,name}, event:"watch", data:{kind,...}, warnings:[]}` |
| 连接重连 | ✅ 信任后重连免弹窗（ping 14ms） |
| Ctrl-C 注销 | ⚠️ 自动化环境屏蔽 SIGINT（后台 job 忽略），未能在本环境触发；注销分支（`monitor_folder(path,false)` + `Error::Interrupted`）经代码 review，建议真实终端验证（exit 130） |
| 清理 | ✅ 测试目录已删、无残留 handshaker 进程 |

**结论**：目录监控与剪贴板推送在真机验证通过，事件类型映射与文档一致；这是
MONITOR_FOLDER 协议首次真机互通证据（此前仅 APK 反编译推断）。
