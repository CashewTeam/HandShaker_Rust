# M8 开发计划：内部分层整理、应用服务模型冻结与 `handshaker-ffi`

> 项目：`CashewTeam/HandShaker_Rust`  
> 计划基线：`handshaker_rust 0.6.1`  
> 里程碑代号：M8  
> 文档用途：供 Codex、Claude Code、OpenAI Codex Agent 及人工维护者共同执行  
> 状态：待实施  
> 目标分支建议：`refactor/m8-workspace-application-ffi`

---

## 0. 执行摘要

M8 的目标不是增加新的 SSP 协议能力，而是把当前已经较完整的单包 Rust 后端整理为可以长期支撑以下调用入口的稳定架构：

- `handshaker` CLI；
- macOS Swift/SwiftUI 前端；
- Linux GTK 前端；
- Windows/.NET 前端；
- 后续自动化工具、测试工具和其他语言绑定。

当前项目是单 Cargo package，library、CLI、协议实现、应用编排、同步逻辑、状态存储和平台传输均位于同一源码树中。公开 API 已经能够覆盖 ADB、Wi-Fi、USB AOA、文件、剪贴板、媒体、同步、事件和取消等能力，但公开边界仍然接近“底层客户端 API”，CLI 中也存在较多应用流程编排。

M8 应完成三件事：

1. **内部分层整理**  
   将单 package 改造成 Cargo Workspace，并建立严格的依赖方向。

2. **应用服务模型冻结**  
   创建面向 GUI、CLI 和跨语言绑定的 `handshaker-application` 服务层，冻结第一版公开业务模型、错误码、任务和事件语义。

3. **建立 `handshaker-ffi` 最小可用闭环**  
   创建独立 FFI crate，完成 Runtime、设备发现、连接、设备信息、目录浏览、事件订阅和显式资源释放。M8 不要求一次导出全部功能，但必须形成后续可扩展且不破坏 ABI 的基础。

M8 完成后，CLI 必须继续工作，已有协议和业务能力不得回归。Swift 前端可以开始使用 FFI 原型替代 CLI；GTK Rust 前端可以直接依赖 Application 层；.NET 绑定可以在稳定 C ABI 之上继续实现。

---

# 1. 背景与当前问题

## 1.1 当前工程形态

当前根 `Cargo.toml` 同时定义：

- library：`handshaker_rust`；
- binary：`handshaker`；
- `build.rs` 与 protobuf 生成；
- 所有运行时和平台依赖。

当前 `src/lib.rs` 直接导出大量类型和功能，包括：

- `HandShakerClient`；
- `ConnectionTarget`；
- `ClientOptions`；
- 取消令牌和请求选项；
- ADB、Wi-Fi、USB 设备模型；
- 文件、剪贴板、媒体、批量传输和同步模型；
- Client 事件和事件订阅；
- State、SyncStore 和同步算法函数。

当前 `src/client.rs` 同时承担：

- 设备枚举；
- 传输通道选择；
- 连接和握手；
- 设备信息初始化；
- 文件与剪贴板请求；
- 媒体请求；
- 批量传输；
- 信任记录操作；
- Session 生命周期；
- 部分协议响应解码和业务转换。

当前 `src/cli.rs` 同时承担：

- Clap 命令定义；
- 参数本地化；
- CLI 路径处理；
- 人机交互确认；
- 连接目标构造；
- 文件和批量任务编排；
- 同步流程；
- shell/REPL；
- watch；
- 部分本地文件扫描和应用级逻辑。

这套结构对于快速完成 CLI 很有效，但不适合直接扩展到多个 GUI 和语言绑定。

## 1.2 M8 要解决的核心问题

### 问题 A：GUI 不应依赖底层协议客户端

GUI 需要的是：

- 设备发现；
- 打开/关闭设备会话；
- 浏览目录；
- 启动传输任务；
- 取消任务；
- 查询任务快照；
- 订阅事件；
- 获取可展示的稳定错误。

GUI 不应该理解：

- SSP sid；
- protobuf；
- Session 帧；
- ADB forward 清理；
- Wi-Fi 握手内部状态；
- Rust 内部取消结构；
- CLI JSON envelope。

### 问题 B：CLI 中存在不可复用的应用编排

如果未来 Swift、GTK 和 .NET 各自重新实现：

- 连接选择；
- 路径解析；
- 任务管理；
- 进度聚合；
- 错误分类；
- 事件订阅；
- 资源关闭；

将形成多套不一致实现。

### 问题 C：当前 Rust 类型不能直接作为跨语言契约

Rust 的以下类型不能被视为稳定 ABI：

- `String`；
- `Vec<T>`；
- `PathBuf`；
- `SocketAddr`；
- trait object；
- `tokio` stream；
- Rust enum 默认布局；
- `Result<T, E>`；
- async future；
- 带泛型或生命周期的类型。

因此必须建立独立的跨语言 DTO 和句柄模型。

### 问题 D：错误和事件语义尚未正式冻结

CLI 可以展示字符串，但 GUI 需要稳定地判断：

- 是否需要安装 ADB；
- 设备是否断开；
- 操作是否可重试；
- 是否为用户取消；
- 是否为远端取消；
- 是否需要重新建立 Session；
- 是否是权限、协议或本地文件错误。

M8 必须冻结稳定错误码和公共事件种类。

---

# 2. M8 范围

## 2.1 M8 必须完成

- 将仓库改造成 Cargo Workspace；
- 拆分 `core`、`application`、`cli`、`ffi` crate；
- 保持 CLI 命令和 JSON/JSONL 行为兼容；
- 保持现有协议功能和测试通过；
- 定义 Application Service；
- 定义稳定的应用层 DTO；
- 定义稳定公共错误模型；
- 定义 Runtime、Session、Transfer、Subscription 句柄语义；
- 定义 Backend Event v1；
- 完成 `handshaker-ffi` 最小闭环；
- 提供 C Header；
- 提供 Swift 可调用验证样例或 smoke test；
- 提供 ABI 资源释放和 panic 隔离；
- 增加架构、FFI 和迁移文档；
- 增加 CI 验证。

## 2.2 M8 建议完成

- UniFFI 的 Swift 绑定原型；
- Apple 静态库或 XCFramework 生成脚本；
- C ABI 的 .NET P/Invoke smoke test；
- GTK Rust 最小编译示例；
- ABI 版本查询；
- 自动生成 Header 的工具；
- API 兼容性快照测试。

## 2.3 M8 明确不做

- 不新增协议功能；
- 不重写 Session、传输和握手实现；
- 不修改已验证的线路格式；
- 不把所有 CLI 命令一次性导出到 FFI；
- 不在 M8 内完成正式 Swift GUI；
- 不在 M8 内完成 GTK GUI；
- 不在 M8 内完成完整 .NET SDK；
- 不承诺跨版本二进制 ABI 永久兼容；
- 不直接把现有 `domain.rs` 的所有 Rust 类型暴露给外部语言；
- 不允许 panic 穿过 FFI；
- 不允许外部调用者直接持有 Rust 引用或 Tokio 类型。

---

# 3. 目标架构

## 3.1 Workspace 结构

```text
HandShaker_Rust/
├── Cargo.toml
├── Cargo.lock
├── crates/
│   ├── handshaker-core/
│   │   ├── Cargo.toml
│   │   ├── build.rs
│   │   └── src/
│   ├── handshaker-application/
│   │   ├── Cargo.toml
│   │   └── src/
│   ├── handshaker-cli/
│   │   ├── Cargo.toml
│   │   └── src/
│   ├── handshaker-ffi/
│   │   ├── Cargo.toml
│   │   ├── include/
│   │   └── src/
│   └── handshaker-test-support/
│       ├── Cargo.toml
│       └── src/
├── proto/
├── locales/
├── bindings/
│   ├── swift/
│   ├── dotnet/
│   └── examples/
├── scripts/
├── docs/
└── tests/
```

根 Workspace：

```toml
[workspace]
resolver = "2"
members = [
    "crates/handshaker-core",
    "crates/handshaker-application",
    "crates/handshaker-cli",
    "crates/handshaker-ffi",
    "crates/handshaker-test-support",
]

[workspace.package]
edition = "2024"
license = "..."
repository = "https://github.com/CashewTeam/HandShaker_Rust"

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
```

实际版本应沿用现有 `Cargo.lock`，不要在拆分过程中无理由升级全部依赖。

## 3.2 依赖方向

```text
handshaker-core
        ↑
handshaker-application
        ↑
 ┌──────┼────────────────┐
 │      │                │
CLI    FFI        GTK Rust（未来）
```

允许：

- application → core；
- cli → application；
- ffi → application；
- test-support → core/application；
- core 内部模块互相依赖，但应维持当前清晰边界。

禁止：

- core → application；
- core → cli；
- core → ffi；
- application → cli；
- application → ffi；
- ffi → cli；
- GUI 概念进入 core；
- CLI 输出模型进入 application。

---

# 4. Crate 职责

## 4.1 `handshaker-core`

### 职责

保留所有经过协议验证的核心能力：

- SSP framing；
- protobuf；
- ADB/Wi-Fi/USB transport；
- 握手和信任；
- Session；
- 原始请求路由；
- 设备能力；
- 文件系统能力；
- 单文件和批量传输底层执行；
- 剪贴板；
- 媒体；
- 事件解码；
- 同步算法与持久化；
- 低层取消；
- Wire log；
- 现有安全限制。

### 初始迁移原则

M8 第一阶段不要同时做大规模文件重命名和逻辑重构。优先把现有 `src/` 基本原样移动到 `crates/handshaker-core/src/`，让测试先恢复通过。

### Core 公共 API

Core 可以继续保留 Rust 原生 API，但需要逐步缩窄。

短期兼容导出：

```rust
pub use client::{
    ClientOptions,
    ConnectionTarget,
    EventCallbacks,
    HandShakerClient,
    PingResult,
};
```

M8 内不要求立即删除现有公开 API，以避免破坏 CLI 和现有用户。但新增 GUI/FFI 不得直接以 `HandShakerClient` 作为最终公共入口。

## 4.2 `handshaker-application`

### 职责

提供与 UI 框架、CLI 和绑定无关的应用服务：

- Runtime 生命周期；
- 设备发现和设备目录；
- Session 生命周期；
- 任务注册、进度、取消和完成；
- Backend Event Hub；
- 公共错误转换；
- 连接选项规范化；
- 稳定应用 DTO；
- 对 Core API 的适配；
- 调用者无关的资源管理。

### 不负责

- Clap；
- stdout/stderr；
- Swift/UniFFI；
- C ABI；
- AppKit；
- GTK Widget；
- P/Invoke；
- human 文案渲染；
- CLI JSON envelope。

## 4.3 `handshaker-cli`

### 职责

- Clap 命令树；
- 本地化帮助；
- 参数解析；
- human/JSON/JSONL 输出；
- stdin/stdout/stderr；
- shell/REPL；
- 危险操作确认；
- 调用 Application Service；
- 兼容原有 CLI 行为。

### 迁移原则

CLI 不得在 M8 第一阶段重写全部命令。采用渐进式迁移：

1. 先让 CLI crate 编译并继续调用 Core 兼容 API；
2. 再优先迁移设备、连接和文件浏览到 Application；
3. 再迁移传输、事件和同步；
4. M8 完成时，主要命令路径应通过 Application；
5. 仅 CLI 特有的交互逻辑继续保留在 CLI。

## 4.4 `handshaker-ffi`

### 职责

- 稳定 C ABI；
- 可选 UniFFI 高级绑定；
- FFI DTO；
- 不透明句柄；
- panic 隔离；
- 内存所有权；
- Buffer 编解码；
- 调用线程和 Runtime 管理；
- ABI 版本；
- C Header；
- Swift smoke test。

### 不负责

- 直接实现协议；
- 重复实现业务逻辑；
- 直接解析 protobuf；
- 直接读写 UI；
- 把 Core 类型裸暴露到 ABI。

## 4.5 `handshaker-test-support`

用于共享：

- 假 ADB；
- 假 SSP server；
- 临时状态目录；
- 测试设备数据；
- 事件 fixture；
- 传输 fixture；
- FFI smoke test fixture。

不得让生产 crate 依赖 test-support。

---

# 5. Application Service 模型

## 5.1 总体入口

建议第一版应用入口：

```rust
pub struct HandShakerRuntime {
    // 内部持有配置、任务管理器、事件总线和 session registry
}
```

创建：

```rust
impl HandShakerRuntime {
    pub async fn create(config: RuntimeConfig) -> AppResult<Self>;
    pub async fn shutdown(&self) -> AppResult<()>;
}
```

要求：

- `shutdown()` 幂等；
- Drop 只做保守清理，不依赖异步 Drop 完成所有工作；
- Runtime 关闭后新操作返回稳定错误；
- shutdown 应取消活动任务、关闭 Session、关闭事件订阅；
- Runtime 不能是全局单例；
- 一个进程允许创建多个 Runtime，但首版可明确限制共享资源。

## 5.2 Runtime 配置

```rust
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub adb_path: PathBuf,
    pub default_timeout: Duration,
    pub heartbeat_interval: Duration,
    pub state_dir: Option<PathBuf>,
    pub wire_log: Option<PathBuf>,
    pub event_capacity: usize,
}
```

FFI 公开 DTO 不直接使用 `PathBuf` 和 `Duration`，应转换为：

```rust
pub struct FfiRuntimeConfig {
    pub adb_path_utf8: Option<String>,
    pub default_timeout_ms: u64,
    pub heartbeat_interval_ms: u64,
    pub state_dir_utf8: Option<String>,
    pub event_capacity: u32,
}
```

## 5.3 设备目录

统一三类设备：

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceId(pub String);

#[derive(Debug, Clone)]
pub enum TransportKind {
    Adb,
    Wifi,
    UsbAccessory,
}

#[derive(Debug, Clone)]
pub struct DeviceDescriptor {
    pub id: DeviceId,
    pub display_name: Option<String>,
    pub model: Option<String>,
    pub transport: TransportKind,
    pub transport_address: String,
    pub available: bool,
}
```

设计要求：

- `DeviceId` 是应用层稳定标识，不直接等于某个底层临时地址；
- ADB 可以基于 serial；
- Wi-Fi 可以优先基于 device UUID，发现阶段无 UUID 时使用明确的临时 ID；
- USB 可基于 accessory location；
- 不允许 GUI 使用 enum 的序号值作为长期存储；
- FFI enum 必须给出固定数值。

接口：

```rust
pub async fn list_devices(
    &self,
    request: ListDevicesRequest,
) -> AppResult<Vec<DeviceDescriptor>>;
```

`ListDevicesRequest`：

```rust
pub struct ListDevicesRequest {
    pub include_adb: bool,
    pub include_wifi: bool,
    pub include_usb: bool,
    pub wifi_browse_timeout: Duration,
}
```

## 5.4 Session 模型

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(pub u64);
```

应用层不向调用者暴露 `HandShakerClient`：

```rust
pub async fn connect(
    &self,
    request: ConnectRequest,
) -> AppResult<SessionId>;

pub async fn disconnect(
    &self,
    session_id: SessionId,
) -> AppResult<()>;

pub async fn get_session_snapshot(
    &self,
    session_id: SessionId,
) -> AppResult<SessionSnapshot>;
```

`SessionSnapshot`：

```rust
pub struct SessionSnapshot {
    pub id: SessionId,
    pub device: DeviceDescriptor,
    pub device_info: DeviceInfoDto,
    pub state: SessionState,
    pub connected_at_ms: u64,
    pub last_activity_at_ms: Option<u64>,
}
```

Session 状态固定为：

```rust
pub enum SessionState {
    Connecting = 1,
    Ready = 2,
    Disconnecting = 3,
    Closed = 4,
    Failed = 5,
}
```

## 5.5 文件服务

M8 最小范围：

```rust
pub async fn list_files(
    &self,
    request: ListFilesRequest,
) -> AppResult<Vec<FileEntryDto>>;

pub async fn stat_file(
    &self,
    request: StatFileRequest,
) -> AppResult<FileEntryDto>;

pub async fn create_directory(
    &self,
    request: CreateDirectoryRequest,
) -> AppResult<()>;

pub async fn move_path(
    &self,
    request: MovePathRequest,
) -> AppResult<()>;

pub async fn delete_paths(
    &self,
    request: DeletePathsRequest,
) -> AppResult<DeleteResult>;
```

路径规则：

- Application 接收 UTF-8 远端路径；
- 相对路径解析必须集中处理；
- GUI 不应重复实现 root path 规则；
- 本地路径在 Rust API 使用 `PathBuf`；
- FFI 使用 UTF-8 路径字符串，遇到不可表示路径返回明确错误；
- M8 文档中标明 macOS/Windows/Linux 路径语义差异。

## 5.6 传输任务模型

长任务采用任务 ID，而不是一次 FFI 调用等待完成：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransferId(pub u64);
```

接口：

```rust
pub async fn start_download(
    &self,
    request: DownloadRequest,
) -> AppResult<TransferId>;

pub async fn start_upload(
    &self,
    request: UploadRequest,
) -> AppResult<TransferId>;

pub async fn cancel_transfer(
    &self,
    id: TransferId,
) -> AppResult<()>;

pub async fn get_transfer(
    &self,
    id: TransferId,
) -> AppResult<TransferSnapshot>;

pub async fn list_transfers(
    &self,
) -> AppResult<Vec<TransferSnapshot>>;
```

状态：

```rust
pub enum TransferState {
    Queued = 1,
    Running = 2,
    Completed = 3,
    Failed = 4,
    Cancelled = 5,
}
```

快照：

```rust
pub struct TransferSnapshot {
    pub id: TransferId,
    pub session_id: SessionId,
    pub direction: TransferDirectionDto,
    pub source: String,
    pub destination: String,
    pub state: TransferState,
    pub transferred_bytes: u64,
    pub total_bytes: Option<u64>,
    pub started_at_ms: Option<u64>,
    pub finished_at_ms: Option<u64>,
    pub error: Option<PublicError>,
}
```

要求：

- 任务完成后保留有限历史；
- 历史容量可配置；
- 任务状态转换单向且可测试；
- `cancel_transfer()` 幂等；
- 本地取消和远端取消在事件及错误详情中可区分；
- 不伪造总大小和进度；
- 下载数据仍由 Rust 直接写文件，不把大文件字节穿过 FFI。

## 5.7 事件模型

统一事件：

```rust
pub enum BackendEvent {
    RuntimeStarted,
    RuntimeStopping,
    DeviceAdded(DeviceDescriptor),
    DeviceUpdated(DeviceDescriptor),
    DeviceRemoved(DeviceId),
    SessionStateChanged(SessionSnapshot),
    TransferUpdated(TransferSnapshot),
    ClipboardChanged(ClipboardSnapshot),
    MediaChanged(MediaChangeDto),
    RemoteFileChanged(RemoteFileChangeDto),
    Warning(PublicWarning),
}
```

每个事件包含 envelope：

```rust
pub struct EventEnvelope {
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub event: BackendEvent,
}
```

要求：

- sequence 在单 Runtime 内单调递增；
- 允许订阅者 Lagged；
- 订阅关闭必须可识别；
- 未知底层事件不能 panic；
- 不能把 protobuf 原始对象暴露给 Application；
- M8 可先完整实现 Session 和 Transfer 事件，其他事件使用映射或保守包装。

---

# 6. 公共错误模型冻结

## 6.1 PublicError

```rust
pub struct PublicError {
    pub code: PublicErrorCode,
    pub message: String,
    pub detail: Option<String>,
    pub retryable: bool,
    pub operation: Option<String>,
}
```

`message` 用于展示，不能用于程序判断。

## 6.2 错误码分区

```text
1000–1099 Runtime
1100–1199 参数与状态
2000–2099 设备发现
2100–2199 连接
2200–2299 信任与握手
3000–3099 远端文件系统
3100–3199 本地文件系统
4000–4099 上传
4100–4199 下载
4200–4299 任务与取消
5000–5099 协议
5100–5199 编解码
6000–6099 ADB
6100–6199 Wi-Fi
6200–6299 USB
7000–7099 媒体
7100–7199 剪贴板
7200–7299 同步
9000–9099 内部错误
```

第一版至少冻结：

```rust
pub enum PublicErrorCode {
    RuntimeClosed = 1001,
    InvalidArgument = 1101,
    InvalidState = 1102,
    NotFound = 1103,

    DeviceNotFound = 2001,
    DeviceUnavailable = 2002,

    ConnectFailed = 2101,
    ConnectionLost = 2102,
    SessionNotFound = 2103,
    SessionClosed = 2104,

    TrustRequired = 2201,
    TrustRejected = 2202,

    RemotePathNotFound = 3001,
    RemotePermissionDenied = 3002,
    RemotePathExists = 3003,

    LocalPathNotFound = 3101,
    LocalPermissionDenied = 3102,
    LocalPathExists = 3103,

    TransferNotFound = 4201,
    TransferCancelled = 4202,
    RemoteCancelled = 4203,

    ProtocolError = 5001,
    DecodeError = 5101,

    AdbUnavailable = 6001,
    AdbUnauthorized = 6002,
    AdbOffline = 6003,

    WifiDiscoveryFailed = 6101,
    UsbUnavailable = 6201,

    Internal = 9001,
}
```

实际映射应根据现有 `Error`/`ErrorCode` 补齐，但一旦发布，不允许随意复用数值。

## 6.3 错误转换

实现：

```rust
impl From<handshaker_core::Error> for PublicError
```

转换必须：

- 不泄露密钥；
- 不泄露 wire payload；
- 不依赖本地化字符串分析；
- 对未知错误映射到 `Internal`；
- 保留可诊断 detail，但 detail 不能包含敏感内容；
- `Interrupted`/取消要映射为取消错误；
- CLI 退出码可继续使用现有逻辑，但应逐步基于 PublicErrorCode。

---

# 7. FFI v1 设计

## 7.1 双接口策略

M8 推荐：

- 稳定基础层：C ABI；
- Swift 便利层：UniFFI 原型或 Swift 包装；
- GTK Rust：直接依赖 application；
- .NET：未来使用 C ABI + `LibraryImport`/PInvoke。

UniFFI 不应成为 Application API 的定义源，Application 层才是源。

## 7.2 crate 类型

```toml
[lib]
crate-type = ["rlib", "staticlib", "cdylib"]
```

## 7.3 ABI 版本

```c
uint32_t hs_abi_version_major(void);
uint32_t hs_abi_version_minor(void);
uint32_t hs_abi_version_patch(void);
```

初始：

```text
1.0.0
```

规则：

- 增加函数：minor；
- 增加可选字段：minor；
- 改变现有函数签名：major；
- 改变 enum 数值：major；
- 修复内部实现：patch；
- Rust crate 版本与 ABI 版本独立记录。

## 7.4 不透明句柄

```c
typedef struct HsRuntime HsRuntime;
typedef struct HsSubscription HsSubscription;
```

Session 和 Transfer 首版可直接使用 `uint64_t` ID，不必每个对象都暴露指针句柄。

## 7.5 Buffer 所有权

```c
typedef struct {
    uint8_t* ptr;
    size_t len;
    size_t capacity;
} HsByteBuffer;
```

必须提供：

```c
void hs_byte_buffer_free(HsByteBuffer buffer);
```

规则：

- Rust 分配的内存只能由 Rust free；
- 空 buffer 使用 `{NULL, 0, 0}`；
- 外部语言不得修改 capacity；
- free 空 buffer 安全；
- 文档明确调用者所有权；
- 不返回借用指针。

## 7.6 Result 设计

建议第一版采用统一 JSON/UTF-8 Buffer 作为复杂参数和结果：

```c
typedef struct {
    int32_t status;
    HsByteBuffer value;
    HsByteBuffer error;
} HsCallResult;
```

规则：

- `status == 0` 表示成功；
- 成功时 `value` 为 UTF-8 JSON，`error` 为空；
- 失败时 `error` 为 PublicError JSON，`value` 为空；
- 必须提供 `hs_call_result_free`；
- 简单版本查询不使用 JSON；
- 大文件数据不使用该 Buffer；
- 以后可逐步为高频路径增加结构化 ABI，不破坏已有 JSON API。

采用 JSON 的理由：

- 首版跨 Swift/.NET 验证成本低；
- 便于 Agent 自动生成绑定；
- 便于记录契约快照；
- 当前数据量相对可控；
- 文件内容本身不经过 JSON；
- 将 ABI 风险与业务模型冻结分离。

## 7.7 Runtime API

```c
HsCallResult hs_runtime_create(
    const uint8_t* config_json,
    size_t config_len,
    HsRuntime** out_runtime
);

HsCallResult hs_runtime_shutdown(HsRuntime* runtime);

void hs_runtime_destroy(HsRuntime* runtime);
```

要求：

- `out_runtime` 只在成功时写入；
- `destroy(NULL)` 安全；
- shutdown 可重复；
- destroy 会执行保守取消；
- 所有 extern 函数使用 `catch_unwind`；
- panic 映射为 `Internal`；
- 不允许 unwind 穿过 ABI。

## 7.8 M8 必须导出的业务函数

```c
HsCallResult hs_list_devices(
    HsRuntime* runtime,
    const uint8_t* request_json,
    size_t request_len
);

HsCallResult hs_connect(
    HsRuntime* runtime,
    const uint8_t* request_json,
    size_t request_len
);

HsCallResult hs_disconnect(
    HsRuntime* runtime,
    uint64_t session_id
);

HsCallResult hs_get_session(
    HsRuntime* runtime,
    uint64_t session_id
);

HsCallResult hs_list_files(
    HsRuntime* runtime,
    uint64_t session_id,
    const uint8_t* request_json,
    size_t request_len
);
```

建议同时完成：

```c
HsCallResult hs_ping(...);
HsCallResult hs_create_directory(...);
HsCallResult hs_start_download(...);
HsCallResult hs_cancel_transfer(...);
HsCallResult hs_get_transfer(...);
```

## 7.9 异步调用策略

不能在 Swift 主线程直接执行长时间同步 FFI。

M8 推荐两层：

### C ABI 基础层

同步函数内部通过 Runtime 执行 async，并阻塞当前调用线程直到完成。要求调用方在后台线程调用。

适用于：

- list devices；
- connect；
- list files；
- stat；
- 短操作。

### 长任务

返回 ID：

- start download/upload 只等待任务注册；
- 进度通过事件订阅或 get_transfer；
- cancel 使用独立函数。

后续可增加真正 callback/future bridge，但不作为 M8 的必要条件。

## 7.10 Event Subscription

建议首版使用队列拉取，避免回调生命周期复杂度：

```c
HsCallResult hs_subscribe_events(
    HsRuntime* runtime,
    HsSubscription** out_subscription
);

HsCallResult hs_subscription_next(
    HsSubscription* subscription,
    uint32_t timeout_ms
);

void hs_subscription_destroy(HsSubscription* subscription);
```

语义：

- next 返回一个 EventEnvelope JSON；
- timeout 返回明确的 timeout 状态或空结果；
- Runtime shutdown 后返回 Closed；
- 订阅有固定缓冲；
- Lagged 作为事件或错误明确上报；
- destroy 可从任意非回调线程调用；
- M8 不需要跨语言回调；
- Swift 层可以在 Task 中循环调用 next 并转换为 AsyncStream。

---

# 8. Swift 接入目标

## 8.1 Swift 层结构（现已落地为 `platform/macos/HandShakerCore`，2026-08）

```text
platform/macos/HandShakerCore/
├── Generated/
├── Native/
│   ├── RuntimeHandle.swift
│   ├── NativeCall.swift
│   └── NativeError.swift
├── Models/
└── HandShakerClient.swift
```

Swift GUI 仍然保留：

```swift
protocol BackendClient: Sendable
```

正式实现：

```text
FFIBackendClient → platform/macos/HandShakerCore Swift wrapper → C ABI/UniFFI
```

不允许：

```text
SwiftUI View → 生成的 UniFFI 类型
SwiftUI View → C 函数
ViewModel → UnsafeMutableRawPointer
```

## 8.2 M8 Swift smoke test

至少验证：

1. 加载库；
2. 查询 ABI 版本；
3. 创建 Runtime；
4. 调用 list devices；
5. 无设备时正常返回空数组或 ADB 状态；
6. 错误能解码为 PublicError；
7. 关闭并销毁 Runtime；
8. Address Sanitizer 下无明显泄漏；
9. 多次 create/destroy 不崩溃。

真机测试作为附加验收：

- ADB 设备列表；
- connect；
- device info；
- list root files；
- disconnect。

---

# 9. GTK 与 .NET 预留

## 9.1 GTK Rust

未来新增：

```text
crates/handshaker-gtk/
```

直接依赖：

```toml
handshaker-application = { path = "../handshaker-application" }
```

Application 层不得依赖 Swift 特性。

需要在 M8 检查：

- API 不要求必须从 Apple 主线程调用；
- DTO 不包含 Swift 专属概念；
- Runtime 可以由 GTK 后台 Tokio runtime 驱动；
- Event Subscription 可转换到 GLib channel；
- 路径和错误模型在 Linux 可工作。

## 9.2 .NET

未来结构：

```text
bindings/dotnet/
├── HandShaker.Native/
├── HandShaker.Client/
└── HandShaker.SmokeTests/
```

M8 的 C ABI 必须满足：

- 所有函数 `extern "C"`；
- Windows 使用明确导出宏；
- 固定宽度整数；
- 不暴露 `usize` 到长期协议字段，Buffer 长度除外；
- bool 使用 `uint8_t` 或整数；
- enum 使用固定 `int32_t`；
- 字符串是 UTF-8 bytes + length；
- 调用约定文档化；
- DLL/.so/.dylib 名称稳定。

---

# 10. 代码迁移计划

## Phase 0：建立基线和护栏

### 任务

- 记录当前 commit SHA；
- 执行 `cargo fmt --check`；
- 执行 `cargo clippy --all-targets --all-features`；
- 执行 `cargo test --all-targets`；
- 记录 CLI `--help`；
- 记录 JSON/JSONL fixture；
- 记录公开 Rust API；
- 记录二进制名称与版本；
- 创建 M8 专用分支。

### 产物

```text
docs/m8-baseline.md
tests/fixtures/m8/
```

### 验收

- 任何迁移阶段都能与 baseline 比较；
- 已知失败必须记录，不允许在后续误称为新回归。

## Phase 1：建立 Workspace，不改逻辑

### 任务

- 创建根 Workspace；
- 创建 `handshaker-core`；
- 移动现有 library 文件；
- 移动 `build.rs`；
- 保持 proto 路径有效；
- 保持 locales 路径有效；
- 修复 include 路径；
- 恢复所有 core 测试；
- 创建 CLI crate 并移动 main/cli/output；
- 保持 binary 名称 `handshaker`；
- 保持版本输出和 Cargo install 方式。

### 风险

- `build.rs` 相对路径变化；
- `include_str!` locale 路径；
- 集成测试使用 `CARGO_BIN_EXE_handshaker`；
- 文档测试 crate 名变化；
- fixture 路径；
- macOS CI working directory；
- target 路径。

### 验收

- CLI 命令无变化；
- Core 测试通过；
- CLI 测试通过；
- package 构建通过；
- 不进行业务重构。

## Phase 2：建立 Application crate 与兼容适配

### 任务

- 创建 RuntimeConfig；
- 创建 HandShakerRuntime；
- 创建 registries：
  - SessionRegistry；
  - TransferRegistry；
  - EventHub；
- 创建 Application DTO；
- 创建 PublicError；
- 实现 Core → Application 转换；
- 实现 list devices；
- 实现 connect/disconnect；
- 实现 session snapshot；
- 实现 list files；
- 增加单元测试。

### 验收

- Application 不依赖 CLI；
- Application 不公开 Core Session；
- Application 不公开 protobuf；
- 所有句柄查找错误稳定；
- Runtime shutdown 可测试；
- 并发访问不发生数据竞争。

## Phase 3：迁移 CLI 到 Application

### 优先顺序

1. `device list/discover/info/ping`；
2. `fs ls/stat/exists/mkdir/mv/rm`；
3. `fs pull/push`；
4. batch；
5. clipboard；
6. media；
7. watch；
8. sync；
9. shell。

### 迁移规则

- 每迁移一个命令，保持输出 fixture；
- CLI 负责路径输入和展示；
- Application 负责业务用例；
- 不把 `OutputFormat` 放进 Application；
- 不把 `--yes` 放进 Application；
- Application 只接收已经明确授权的请求；
- 危险确认仍在 CLI；
- shell 当前目录属于 CLI 状态，但远端路径解析 helper 可放 Application。

### 验收

- CLI 主要命令不再直接构造 `HandShakerClient`；
- CLI JSON 字段无非预期变化；
- exit code 无非预期变化；
- Ctrl+C 行为保持；
- watch 能正确关闭注册；
- shell 正常关闭 Session。

## Phase 4：冻结 Application v1

### 任务

- 审计 DTO；
- 审计 enum 数值；
- 审计 error code；
- 审计任务状态机；
- 审计事件语义；
- 增加 serde fixture；
- 增加 schema 文档；
- 标记 `#[non_exhaustive]` 的 Rust API；
- 明确字段新增兼容规则；
- 添加 `APPLICATION_API_VERSION`。

### 冻结标准

冻结不表示永远不变，而表示：

- v1 字段名称不随意重命名；
- enum 数值不复用；
- error code 不复用；
- ID 生命周期明确；
- 时间统一使用 Unix milliseconds；
- 字节统一使用 `u64`；
- UTF-8 规则明确；
- Optional 字段语义明确；
- 未知 enum 和新字段的兼容策略明确。

## Phase 5：建立 FFI 基础设施

### 任务

- 创建 `handshaker-ffi`；
- 配置 staticlib/cdylib；
- 添加导出宏；
- 添加 panic boundary；
- 添加 UTF-8 输入校验；
- 添加 Buffer；
- 添加 Result；
- 添加 ABI version；
- 创建 Header；
- 创建 C smoke test；
- 增加内存和 NULL 测试。

### 验收

- 所有 extern 函数 catch panic；
- 所有返回 Buffer 可释放；
- NULL 行为文档化；
- malformed JSON 不崩溃；
- invalid UTF-8 不崩溃；
- 重复 destroy 不访问释放内存；
- CI 至少在 macOS 和 Linux 构建。

## Phase 6：业务 FFI 最小闭环

### 必须完成

- runtime create/shutdown/destroy；
- list devices；
- connect；
- get session；
- disconnect；
- list files；
- subscribe；
- next event；
- subscription destroy。

### 建议完成

- ping；
- create directory；
- start download；
- get transfer；
- cancel transfer。

### 验收

- fake backend 测试；
- 无设备环境测试；
- 真机 smoke test 文档；
- Swift 示例成功；
- 多次调用无泄漏；
- Runtime 关闭后调用返回 RuntimeClosed。

## Phase 7：文档与发布脚本

### 文档

```text
docs/architecture.md
docs/application-api-v1.md
docs/ffi-v1.md
docs/m8-migration.md
docs/m8-test-report.md
```

### 脚本

```text
scripts/build-ffi-macos.sh
scripts/build-ffi-linux.sh
scripts/generate-ffi-header.sh
scripts/run-ffi-smoke-tests.sh
```

### Apple 建议产物

```text
dist/apple/
├── libhandshaker_ffi.a
├── handshaker_ffi.h
├── module.modulemap
└── platform/macos/Artifacts/HandShakerFFI.xcframework（建议）
```

---

# 11. 测试计划

## 11.1 Core 回归测试

所有已有：

- framing；
- handshake；
- transport；
- event decode；
- cancellation；
- CLI fixture；
- fake ADB；
- fake SSP；
- sync；
- media；
- transfer；

必须继续通过。

## 11.2 Application 单元测试

覆盖：

- Runtime create/shutdown；
- Session registry；
- Transfer registry；
- ID 唯一性；
- invalid ID；
- Core error → PublicError；
- Device descriptor 映射；
- Session 状态转换；
- Transfer 状态转换；
- Event sequence；
- subscriber lag；
- shutdown 取消；
- 并发 list/get；
- 重复 disconnect/cancel。

## 11.3 Application 集成测试

使用 fake transport：

- list devices；
- connect；
- device info；
- list files；
- connection loss；
- transfer progress；
- user cancel；
- remote cancel；
- event order；
- runtime shutdown。

## 11.4 FFI 单元测试

从 Rust 直接调用 extern：

- NULL runtime；
- NULL input + 非零长度；
- invalid UTF-8；
- invalid JSON；
- oversized JSON；
- success result；
- error result；
- free empty buffer；
- free success/error；
- ABI version；
- panic conversion。

## 11.5 C smoke test

使用纯 C 编译：

- include Header；
- 链接动态/静态库；
- ABI version；
- create Runtime；
- list devices；
- decode JSON；
- shutdown；
- destroy。

## 11.6 Swift smoke test

- module import；
- Runtime RAII；
- 后台调用；
- JSON Codable；
- PublicError；
- Event AsyncStream 包装；
- shutdown；
- deinit。

## 11.7 内存与并发

建议：

- Address Sanitizer；
- Thread Sanitizer（可运行范围内）；
- Miri 检查纯 Rust unsafe helper；
- `cargo test`；
- `cargo clippy`；
- FFI 压力循环 1000 次 create/destroy；
- 多订阅者；
- shutdown 与 next_event 并发；
- cancel 与 completion 竞争。

---

# 12. CI 计划

建议新增矩阵：

```text
ubuntu-latest
macos-14
windows-latest（至少 build FFI）
```

Jobs：

1. format；
2. clippy；
3. workspace tests；
4. CLI compatibility tests；
5. FFI build；
6. C smoke test；
7. Swift smoke test（macOS）；
8. artifact upload；
9. optional ABI snapshot。

命令：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
cargo build -p handshaker-ffi --release
```

---

# 13. Agent 执行规则

## 13.1 每次修改前

Agent 必须：

1. 阅读本计划；
2. 阅读 `plan.md`；
3. 阅读当前目标文件；
4. 检查已有测试；
5. 说明本次只执行哪个 Phase；
6. 不同时跨越多个高风险 Phase。

## 13.2 每次提交要求

每个 commit 必须：

- 单一目的；
- 可编译；
- 测试通过或记录已知失败；
- 不混入无关格式化；
- 不升级无关依赖；
- 不删除已有测试；
- 不声称未运行的测试已通过。

建议 commit：

```text
refactor(workspace): move existing library into handshaker-core
refactor(cli): move binary into handshaker-cli
feat(application): add runtime and session registry
feat(application): freeze public error model
refactor(cli): route device commands through application service
feat(ffi): add ABI version and owned byte buffers
feat(ffi): expose runtime and device listing
feat(ffi): add session and file listing APIs
test(ffi): add C and Swift smoke tests
docs(m8): document application and FFI v1 contracts
```

## 13.3 禁止行为

Agent 不得：

- 一次提交移动全部文件并重写逻辑；
- 为了“整洁”改变协议字节；
- 删除复杂测试；
- 修改现有 CLI JSON 字段而不说明；
- 用 `unwrap()` 处理 FFI 输入；
- 让 panic 穿过 ABI；
- 用全局 mutable static 存 Runtime；
- 把裸 Tokio handle 返回给外部；
- 在 C API 返回 Rust `String`/`Vec`；
- 让 Swift 释放 Rust allocator 内存；
- 用错误字符串决定错误类别；
- 为未来假设大量未验证 API；
- 直接把所有 Core 类型 derive UniFFI；
- 把 UI 状态写入 Application；
- 把 CLI 的 `--yes` 放进 Application；
- 声称“ABI 稳定”但没有版本和测试。

---

# 14. 风险与缓解

## 风险 1：拆 Workspace 导致大量路径和测试失败

缓解：

- Phase 1 只移动；
- 保留目录兼容软策略；
- 先恢复 build.rs、proto、locale；
- 单独提交。

## 风险 2：Application 层成为无意义包装

缓解：

- 由 Application 管理 Runtime、Session、Transfer、Event；
- 不只是给 Core 方法改名；
- CLI 和 FFI 都实际依赖它。

## 风险 3：过早冻结错误模型

缓解：

- 使用分区错误码；
- 保留 detail；
- 未知映射 Internal；
- enum 标记 non-exhaustive；
- v1 只冻结 GUI 必需部分。

## 风险 4：UniFFI 限制未来 .NET

缓解：

- C ABI 为基础；
- UniFFI 只是 Swift 便利层；
- Application 是唯一业务契约来源。

## 风险 5：FFI async 与生命周期复杂

缓解：

- 短操作后台线程同步等待；
- 长操作任务 ID；
- 事件队列拉取；
- 不在 M8 引入跨语言回调。

## 风险 6：FFI JSON 性能

缓解：

- M8 数据规模可接受；
- 文件内容不走 JSON；
- 后续热点可增加结构化函数；
- 不破坏 v1 JSON API。

## 风险 7：CLI 行为回归

缓解：

- fixture；
- snapshot；
- 分命令迁移；
- CLI 输出模型仍在 CLI；
- 保持 binary 名称。

---

# 15. Definition of Done

M8 只有满足全部核心条件才可标记完成：

## 工程

- [ ] 根项目是 Cargo Workspace；
- [ ] core/application/cli/ffi crate 分离；
- [ ] 依赖方向符合文档；
- [ ] `handshaker` binary 名称保持；
- [ ] 当前版本与构建方式已更新文档。

## 回归

- [ ] 现有 Core 测试通过；
- [ ] CLI 测试通过；
- [ ] CLI 主要命令可运行；
- [ ] JSON/JSONL 无未声明破坏；
- [ ] fake ADB/SSP 测试保持。

## Application

- [ ] Runtime；
- [ ] DeviceDescriptor；
- [ ] SessionId 与 SessionSnapshot；
- [ ] TransferId 与 TransferSnapshot；
- [ ] EventEnvelope；
- [ ] PublicErrorCode；
- [ ] list devices；
- [ ] connect/disconnect；
- [ ] list files；
- [ ] shutdown；
- [ ] 单元与集成测试。

## FFI

- [ ] ABI version；
- [ ] Runtime create/shutdown/destroy；
- [ ] Buffer free；
- [ ] panic boundary；
- [ ] list devices；
- [ ] connect/get session/disconnect；
- [ ] list files；
- [ ] event subscription；
- [ ] C Header；
- [ ] C smoke test；
- [ ] Swift smoke test；
- [ ] 所有资源所有权有文档。

## 文档

- [ ] 架构；
- [ ] Application v1；
- [ ] FFI v1；
- [ ] Swift 接入；
- [ ] GTK/.NET 预留；
- [ ] M8 测试报告；
- [ ] M8 已知限制。

---

# 16. M8 完成后的预期状态

```text
                   handshaker-cli
                         ↑
handshaker-core ← handshaker-application → handshaker-ffi
                         ↑                    ↑
                  GTK Rust（未来）       Swift / .NET
```

Swift 前端不再依赖 CLI 解析业务结果，而是：

```text
SwiftUI
  ↓
Swift BackendClient
  ↓
FFIBackendClient
  ↓
handshaker-ffi
  ↓
handshaker-application
  ↓
handshaker-core
```

GTK Rust：

```text
GTK UI
  ↓
handshaker-application
  ↓
handshaker-core
```

.NET：

```text
Avalonia / WinUI / WPF
  ↓
HandShaker.Client
  ↓
P/Invoke
  ↓
handshaker-ffi C ABI
  ↓
handshaker-application
```

CLI 长期保留为：

- 自动化入口；
- 调试入口；
- 无 GUI 环境入口；
- FFI 故障对照；
- 集成测试工具。

---

# 17. 建议的 M8 子任务拆分

建议拆为以下 Issue 或 Agent Task：

1. **M8.1：建立 Workspace 基线和兼容性 fixture**
2. **M8.2：迁移现有 library 到 `handshaker-core`**
3. **M8.3：迁移 CLI 到 `handshaker-cli`**
4. **M8.4：建立 `handshaker-application` Runtime 和 registries**
5. **M8.5：冻结 Device/Session/File DTO**
6. **M8.6：冻结 PublicErrorCode v1**
7. **M8.7：建立 TransferManager 和事件总线**
8. **M8.8：迁移 CLI 设备与文件命令**
9. **M8.9：迁移 CLI 传输、媒体、同步与 watch**
10. **M8.10：建立 `handshaker-ffi` Buffer、Result 和 panic boundary**
11. **M8.11：导出 Runtime、设备和 Session API**
12. **M8.12：导出文件浏览和事件订阅**
13. **M8.13：增加 Swift/C smoke test**
14. **M8.14：增加 CI、打包脚本和最终审计**

每个子任务独立验收，不建议由单个 Agent 一次性执行整个 M8。

---

# 18. Agent 完成报告模板

每个子任务完成后，Agent 必须报告：

1. 本次执行的 M8 子任务；
2. 修改文件；
3. 新增公共 API；
4. 是否改变现有 CLI 行为；
5. 是否改变 JSON/JSONL；
6. 是否改变 Rust public API；
7. 运行的构建和测试命令；
8. 实际结果；
9. 未运行的测试；
10. 已知风险；
11. 下一任务前必须处理的问题。

禁止只回复“已完成”。

---

# 19. 最终原则

M8 的判断标准不是“文件拆得越多越好”，而是：

- Core 仍然可靠；
- Application 真正承载跨前端复用的业务语义；
- CLI 不回归；
- FFI 有明确边界；
- 资源生命周期可验证；
- 错误和事件可被 GUI 稳定消费；
- Swift、GTK 和 .NET 不需要各自重复实现后端业务流程；
- 后续新增功能只需先进入 Core/Application，再由各绑定薄层导出。
