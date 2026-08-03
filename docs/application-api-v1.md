# handshaker-application API v1(冻结契约)

> 版本:`APPLICATION_API_VERSION = "1.0.0"`(与 Rust crate 版本独立)
> 里程碑:M8(M8.4 建立,M8.5/M8.6 冻结)
> crate:`crates/handshaker-application`,包名 `handshaker-application`

## 1. 目的与边界

Application 层是 GUI(Swift/GTK)、CLI 与跨语言绑定的**唯一业务契约来源**。
它依赖 `handshaker-core`,**不得**依赖 CLI(clap/stdout/JSON envelope)、
FFI、UniFFI 或任何 UI 框架。

- 不暴露 prost 类型、session 帧、传输内部、`HandShakerClient`。
- 调用方提供自己的 tokio runtime(所有接口 async)。
- 一个进程可创建多个 Runtime;Runtime 不是全局单例。

## 2. 冻结规则(v1)

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
| `DeviceId(pub String)` | 应用层稳定设备标识(ADB=serial, USB=location, WiFi=uuid 或临时 id) |
| `TransportKind` | `Adb=1 / Wifi=2 / UsbAccessory=3` |
| `DeviceDescriptor` | `id, display_name, model, transport, transport_address, available` |
| `SessionId(pub u64)` | 会话标识 |
| `SessionState` | `Connecting=1 / Ready=2 / Disconnecting=3 / Closed=4 / Failed=5` |
| `SessionSnapshot` | `id, device, device_info, state, connected_at_ms, last_activity_at_ms` |
| `DeviceInfoDto` | serial/phone_id/name/model/brand/manufacturer/smartisan_version/apk_version/apk_version_name/root_path |
| `FileEntryDto` | path/size/created_at_ms/modified_at_ms/is_directory/checksum/is_trash/media_id |
| `RuntimeConfig` | adb_path/default_timeout/heartbeat_interval/state_dir/wire_log/event_capacity |
| `ListDevicesRequest` | include_adb/include_wifi/include_usb/wifi_browse_timeout |
| `ConnectRequest` | device(来自 `list_devices` 或同形构造) |
| `ListFilesRequest` | session_id/path/depth |

## 4. Runtime 接口

```rust
impl HandShakerRuntime {
    pub async fn create(config: RuntimeConfig) -> AppResult<Self>;
    pub async fn shutdown(&self) -> AppResult<()>;   // 幂等
    pub async fn list_devices(&self, request: ListDevicesRequest) -> AppResult<Vec<DeviceDescriptor>>;
    pub async fn connect(&self, request: ConnectRequest) -> AppResult<SessionId>;
    pub async fn disconnect(&self, session_id: SessionId) -> AppResult<()>;
    pub async fn get_session_snapshot(&self, session_id: SessionId) -> AppResult<SessionSnapshot>;
    pub async fn list_files(&self, request: ListFilesRequest) -> AppResult<Vec<FileEntryDto>>;
}
```

语义:

- `shutdown()` 幂等:取消活动任务、关闭全部 Session、之后新操作返回
  `RuntimeClosed(1001)`;Drop 只做保守清理。
- 未知/已关闭 Session:`SessionNotFound(2103)`。
- 相对远端路径由 Application 集中解析(`resolve_remote_path`/`normalize_remote_path`),
  `..` 不越过根目录。
- `list_devices` 单传输失败不整批失败(如 ADB 未装时仍返回 WiFi/USB);
  全部失败时返回空列表或对应错误。

## 5. 错误模型

`PublicError { code: PublicErrorCode, message: String, detail: Option<String>,
retryable: bool, operation: Option<String> }`

- `code` 是唯一程序判断依据;`message` 仅展示;`detail` 不含密钥与 wire payload。
- 分区:1000–1099 Runtime / 1100–1199 参数状态 / 2000–2099 设备发现 /
  2100–2199 连接 / 2200–2299 信任握手 / 3000–3099 远端文件系统 /
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
