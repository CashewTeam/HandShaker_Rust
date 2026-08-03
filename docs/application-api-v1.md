# handshaker-application API v1(preview 契约)

> 版本:`APPLICATION_API_VERSION = "1.0.0-preview.1"`(与 Rust crate 版本独立)
> 里程碑:M8(M8.4 建立,M8.5/M8.6 草案冻结);M8.1 Phase A 改为 preview
> crate:`crates/handshaker-application`,包名 `handshaker-application`
>
> **当前状态:preview,不是稳定契约。** 冻结前允许破坏性源码级修改(如
> `session_client()` 过渡入口的移除、事件/传输语义修正);下游消费者
> (Swift/GTK/.NET)不得把 preview 版本当作稳定契约固化。

## 1. 目的与边界

Application 层是 GUI(Swift/GTK)、CLI 与跨语言绑定的**唯一业务契约来源**。
它依赖 `handshaker-core`,**不得**依赖 CLI(clap/stdout/JSON envelope)、
FFI、UniFFI 或任何 UI 框架。

- 不暴露 prost 类型、session 帧、传输内部、`HandShakerClient`。
- 调用方提供自己的 tokio runtime(所有接口 async)。
- 一个进程可创建多个 Runtime;Runtime 不是全局单例。

## 2. 冻结规则(v1,preview 期间作为目标契约)

以下规则在 preview 阶段即为硬性要求,冻结后继续生效;preview 阶段允许的
例外只有:为完成收口而删除临时过渡接口(如 `session_client()`)、修正
事件/传输语义与文档,此类变更必须在此文档与 `docs/m8-migration.md` 记录。

正式冻结(preview 后缀移除)的条件:

- 移除 `session_client()` 过渡入口并完成 CLI 必要迁移;
- 事件桥接、传输进度/取消/终态语义确定;
- 公开 DTO/error/event fixture 完整;
- 文档与代码同步(本文件、`docs/architecture.md`、README)。

冻结规则正文:
- 字段名称不随意重命名;
- enum 判别值不复用(`TransportKind` 1/2/3、`SessionState` 1..=5、错误码分区);
- ID 生命周期明确(`SessionId(u64)` 由 Runtime 单调分配,1 起);
- 时间统一 Unix milliseconds(`u64`);
- 字节统一 `u64`;
- 字符串 UTF-8;远端路径为 UTF-8,不可表示时明确报错;
- `Option` 字段语义:缺省 = 未知/不可用,不猜测默认;
- 未知 enum 值在解码时**拒绝**(serde),不静默映射;
- 枚举与错误码标 `#[non_exhaustive]`:Rust 调用方须处理未来变体;
- JSON 契约:`TransportKind`/`SessionState`/`PublicErrorCode` 以
  snake_case 字符串序列化(`"adb"`/`"ready"`/`"session_not_found"`),
  数值判别值仅内部固定,不作 JSON 值。

## 3. 公开类型(冻结)

| 类型 | 说明 |
|---|---|
| `DeviceId(pub String)` | 应用层设备标识:发现条目的 `id`(ADB=serial, USB=location, WiFi=endpoint)与连接后 reconcile 的 `stable_id`(`phone:<uuid>`);UI 优先 `stable_id ?? id` 作为身份 |
| `TransportKind` | `Adb=1 / Wifi=2 / UsbAccessory=3` |
| `DeviceDescriptor` | `id, stable_id(可选), display_name, model, transport, transport_address, available`(Phase D/D2:Wi-Fi `id` 仅为动态 endpoint 标识,不可长期使用) |
| `SessionId(pub u64)` | 会话标识 |
| `SessionState` | `Connecting=1 / Ready=2 / Disconnecting=3 / Closed=4 / Failed=5` |
| `SessionSnapshot` | `id, device, device_info, state, connected_at_ms, last_activity_at_ms` |
| `DeviceInfoDto` | serial/phone_id/name/model/brand/manufacturer/smartisan_version/apk_version/apk_version_name/root_path/external_storage_path/disk_size/used_disk_size/battery_percentage/phone_locked(后 5 字段可选,Phase D/D2 补全) |
| `FileEntryDto` | path/size/created_at_ms/modified_at_ms/is_directory/checksum/is_trash/media_id |
| `RuntimeConfig` | adb_path/default_timeout/heartbeat_interval/state_dir/wire_log/event_capacity/transfer_history_capacity/transfer_history_ttl |
| `ListDevicesRequest` | include_adb/include_wifi/include_usb/wifi_browse_timeout |
| `ConnectRequest` | device(来自 `list_devices` 或同形构造) |
| `ListFilesRequest` | session_id/path/depth |
| `DeviceDiscoveryResult` | devices + warnings(Phase D/D1;单传输失败不吞错) |
| `DeviceDiscoveryWarning` | transport + error(稳定 JSON:`{"transport":"adb","error":{...}}`) |
| `TrustRecordDto` | device_id(`phone:<uuid>`)/device_name/updated_at_ms(Phase D/D3;不含 derived key) |
| `RemoveTrustRequest/Result`、`ResetWifiTrustRequest` | 信任服务请求/响应(Phase D/D3) |
| `FileOperationPlan` | direction/session_id/items/conflicts/file_count/directory_count/total_bytes/requires_recursive/executable(Phase D/D4) |
| `FilePlanItem/FilePlanConflict/FileConflictKind` | 计划条目与冲突(`destination_exists`/`destination_type_mismatch`/`recursive_required`/`duplicate_destination`/`source_missing`/`local_permission_denied`) |
| `PlanDownloadRequest/PlanUploadRequest/ExecuteFilePlanRequest` | 预检与执行请求(Phase D/D4) |

## 4. Runtime 接口(preview,未冻结)

当前为最小冻结面;M8.1 收口后将更新为完整列表。

```rust
impl HandShakerRuntime {
    pub async fn create(config: RuntimeConfig) -> AppResult<Self>;
    pub async fn shutdown(&self) -> AppResult<()>;   // 幂等
    // 发现（Phase D/D1）:带 per-transport 诊断;list_devices 为 preview 兼容包装
    pub async fn discover_devices(&self, request: ListDevicesRequest) -> AppResult<DeviceDiscoveryResult>;
    pub async fn list_devices(&self, request: ListDevicesRequest) -> AppResult<Vec<DeviceDescriptor>>;
    pub async fn connect(&self, request: ConnectRequest) -> AppResult<SessionId>;
    pub async fn disconnect(&self, session_id: SessionId) -> AppResult<()>;
    pub async fn get_session_snapshot(&self, session_id: SessionId) -> AppResult<SessionSnapshot>;
    pub async fn ping(&self, session_id: SessionId) -> AppResult<PingResultDto>;
    pub async fn list_files(&self, request: ListFilesRequest) -> AppResult<Vec<FileEntryDto>>;
    // 信任（Phase D/D3）:始终使用 Runtime 的 state_dir
    pub async fn list_trust_records(&self) -> AppResult<Vec<TrustRecordDto>>;
    pub async fn remove_trust_record(&self, request: RemoveTrustRequest) -> AppResult<RemoveTrustResult>;
    pub async fn reset_wifi_trust(&self, request: ResetWifiTrustRequest) -> AppResult<()>;
    // 文件预检与执行（Phase D/D4）
    pub async fn plan_download(&self, request: PlanDownloadRequest) -> AppResult<FileOperationPlan>;
    pub async fn plan_upload(&self, request: PlanUploadRequest) -> AppResult<FileOperationPlan>;
    pub async fn execute_file_plan(&self, request: ExecuteFilePlanRequest) -> AppResult<TransferId>;
}
```

语义:

- `shutdown()` 幂等且只执行一次:取消全部传输任务并**有界 join**(不依赖
  固定 sleep),并行走确定性 Session 关闭路径,关闭 EventHub(订阅者收
  `Closed`);之后新操作返回 `RuntimeClosed(1001)`;Drop 只做保守兜底。
- `disconnect(session_id)` 确定性关闭:原子进入 `Disconnecting`(终态幂等)
  → 取消该 Session 的传输任务并等待结束(有界 deadline)→ 显式关闭 Core
  client(发送 QUIT,仅当调用方是最后持有者)→ 无条件发布
  `SessionStateChanged(Closed)`;清理异常(超时/QUIT 发送失败/仍有持有者)
  通过 `Warning` 事件可观察,调用仍返回成功。
- `state_dir` 真实生效:连接、信任(Phase D/D3)记录与 host UUID 写入
  `state_dir`(缺省为 Core 默认配置目录);`wire_log` 真实开启线路日志
  (默认关闭)。
- 未知/已关闭 Session:`SessionNotFound(2103)`/`SessionClosed(2104)`。
- 相对远端路径由 Application 集中解析(`resolve_remote_path`/`normalize_remote_path`),
  `..` 不越过根目录。
- `discover_devices`(Phase D/D1):单传输失败不整批失败——失败以
  `DeviceDiscoveryWarning { transport, error }` 呈现,其余传输的设备保留;
  整体错误(Runtime 已关闭、请求非法)仍返回 `Err`。`list_devices` 为
  preview 兼容包装,仅返回 devices。
- 稳定身份(Phase D/D2):Wi-Fi 发现的 `DeviceDescriptor.id` 是动态
  endpoint 标识;连接成功后 `stable_id`(`phone:<uuid>`)由
  `phone_id` reconcile,`DeviceUpdated` 事件携带 `stable_id`;
  UI 身份判断使用 `stable_id ?? id`。
- 文件计划(Phase D/D4):`plan_download`/`plan_upload` 统一决定源类型、
  recursive 要求、目标解析与 overwrite/类型/重复冲突;
  `FileOperationPlan.executable` 只由不可覆盖冲突和调用方 `overwrite`
  决定;`execute_file_plan` 校验后以后台传输执行并返回统一 `TransferId`
  (进度/终态经 `TransferUpdated` 事件,取消经 `cancel_transfer`)。

## 5. 错误模型

`PublicError { code: PublicErrorCode, message: String, detail: Option<String>,
retryable: bool, operation: Option<String> }`

- `code` 是唯一程序判断依据;`message` 仅展示;`detail` 不含密钥与 wire payload。
- 分区:1000–1099 Runtime / 1100–1199 参数状态 / 2000–2099 设备发现 /
  2100–2199 连接 / 2200–2299 信任握手 / 3000–3099 远端文件系统
  (3004 `remote_io` 为聚合批量失败,Phase D/D4 新增)/
  3100–3199 本地文件系统 / 4200–4299 传输任务 / 5000–5199 协议编解码 /
  6000–6299 传输后端 / 7000–7299 媒体剪贴板 / 9000–9099 内部。
- 核心错误映射 `from_core_error`:未知 → `Internal(9001)`;取消 → 传输取消;
  不解析本地化文案。

## 6. 兼容策略

- 增加函数/可选字段:次要(application API minor);
- 改变函数签名/删除字段/复用错误码:破坏性(major),必须升级
  `APPLICATION_API_VERSION`;
- Rust `#[non_exhaustive]` 保证外部 crate 不受新变体影响;
- 新传输/新会话状态/新错误码只能**追加**,不复用数值。
