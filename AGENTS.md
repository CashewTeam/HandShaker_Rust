# HandShaker_Rust Agent 协作指南

本文件适用于仓库根目录及全部子目录。Agent 开始工作前必须先阅读本文件。

如果目标目录下存在更具体的 `AGENTS.md`，则遵循离目标文件最近的规则；子目录规则只能细化本文件，
不能放宽协议安全、隐私、兼容性和验证要求。

---

## 1. 项目定位与当前阶段

HandShaker_Rust 是兼容原版 Smartisan HandShaker 的跨平台 Rust 后端，包含：

- SSP（SmartSync Protocol）协议、传输和会话实现；
- `handshaker` 命令行客户端；
- UI 与绑定无关的 Application 服务层；
- 面向 Swift、.NET 等语言的稳定 C ABI；
- 协议逆向资料、真实设备抓包证据和验证工具。

当前 Workspace 版本为 `0.7.4`，使用 Rust 2024 edition，包含：

```text
crates/handshaker-core
crates/handshaker-application
crates/handshaker-cli
crates/handshaker-ffi
```

当前已经实现的主要能力包括：

- ADB、Wi-Fi 和 USB AOA 三种连接通道；
- 设备发现、连接、信任和设备信息；
- 文件查询、创建、移动、删除、上传和下载；
- 批量与递归传输、预演和受控并发；
- 剪贴板；
- 照片、视频、音频媒体库、缩略图和 EXIF；
- 主动推送、目录监控和事件解码；
- 单向照片同步及本地同步台账；
- CLI 一次性命令、shell、batch、watch 和 sync；
- Application Runtime、Session、Transfer、事件和公共错误模型；
- 手写 C ABI、C/Swift smoke 示例和 macOS/Linux 构建脚本。

当前开发重点不是继续无边界扩张协议功能，而是：

1. 收紧 Workspace 分层；
2. 完成 CLI 到 Application 的业务边界迁移；
3. 修复 Runtime、Session、Transfer 的生命周期和并发语义；
4. 完成 Core 事件到 Application/FFI 的桥接；
5. 补齐 Swift GUI 所需 FFI；
6. 建立正式的 Swift Package、XCFramework 和跨平台 CI；
7. 在不破坏 CLI 和协议兼容性的前提下，为 GTK、.NET 等平台保留同一 Application 契约。

除非任务明确要求，不要同时引入新的绑定框架、GUI 框架或第二套业务 API。
当前跨语言权威路径是：

```text
handshaker-application
        ↓
手写 handshaker-ffi C ABI
        ↓
Swift / .NET / 其他语言包装层
```

不要在没有架构决策的情况下同时引入 UniFFI、CXX、IPC 服务或另一套 JSON-RPC 接口。

---

## 2. 事实来源与验证等级

修改协议、连接、设备行为或兼容性逻辑前，按以下优先级确认事实：

1. `docs/14-capture-validation.md` 中的真实设备抓包和互通结果；
2. `proto/smartsync.proto` 与 APK 中的权威 proto2 schema；
3. `docs/13-verification-status.md` 中的验证等级和源码索引；
4. `docs/` 中对应协议、传输、命令和平台文档；
5. 本地反编译资料：
   - `Reference/Android_jadx/`
   - `Reference/android_smali/`
   - `Reference/original_smali_1.2.0/`
   - `Reference/macos/`
6. Core 中已有的 fake SSP、测试向量和状态机测试；
7. 明确标记的推断。

推断不能覆盖真实抓包、原版实现或已通过真机验证的结论。

接口或实现细节不确定时，例如：

- 类名或方法签名；
- protobuf 字段含义；
- 请求顺序；
- 握手分支；
- 返回格式；
- 错误码；
- 设备端状态变化；

必须先使用 `rg` 在反编译源码、协议文档和现有实现中交叉验证，并通读真实调用路径。
不要依据方法名猜测，不要把未使用的残留代码当成线上行为，也不要凭空补全协议。

若代码与文档冲突：

1. 先判断代码错误还是文档过时；
2. 找到抓包、schema、反编译或真机依据；
3. 在同一变更中修正代码和受影响文档；
4. 不要静默改变协议行为。

协议任务建议先阅读：

1. `docs/01-overview.md`
2. `docs/04-handshake-trust.md`
3. `docs/05-message-framing.md`
4. `docs/06-protobuf-schema.md`
5. `docs/07-command-reference.md`
6. `docs/13-verification-status.md`
7. `docs/14-capture-validation.md`

架构或绑定任务建议先阅读：

1. `docs/architecture.md`
2. `docs/application-api-v1.md`
3. `docs/ffi-v1.md`
4. `docs/m8-migration.md`
5. 根 `Cargo.toml`
6. 各 crate 的 `Cargo.toml` 与 `src/lib.rs`

---

## 3. Workspace 结构与依赖方向

### 3.1 `handshaker-core`

路径：

```text
crates/handshaker-core/
```

职责：

- SSP framing；
- protobuf 生成类型；
- ADB、Wi-Fi、USB AOA transport；
- 握手、信任和状态存储；
- Session、sid、心跳、读写任务和关闭；
- 设备、文件、剪贴板、媒体、传输和同步的底层实现；
- Core typed events；
- 取消机制；
- fake SSP 和协议级测试。

Core 可以暴露稳定的 Rust 领域 API，但不得暴露给 Application/FFI 消费者：

- Prost 生成类型；
- 原始线路帧；
- 密码学常量；
- 内部 pending request；
- transport cleanup 细节；
- sid 路由内部状态。

### 3.2 `handshaker-application`

路径：

```text
crates/handshaker-application/
```

职责：

- `HandShakerRuntime`；
- Runtime、Session 和 Transfer 生命周期；
- 稳定 DTO；
- 公共错误码；
- 路径解析；
- 设备、文件、剪贴板、媒体和传输应用服务；
- EventHub；
- 面向 CLI、GUI 和绑定的统一业务语义。

Application 必须：

- 只依赖 Core；
- 不依赖 Clap、stdout、CLI JSON envelope、Swift、FFI 或 GUI 框架；
- 不泄露 `HandShakerClient`、Prost、Session frame 或 transport 内部；
- 将跨 UI 可复用的业务编排放在这里，而不是复制到 CLI、Swift 或 FFI。

### 3.3 `handshaker-cli`

路径：

```text
crates/handshaker-cli/
```

职责：

- Clap 命令树；
- human、JSON 和 JSONL 输出；
- 本地化；
- TTY、确认、stdin/stdout/stderr；
- shell、batch 和命令行专属编排；
- 调用 Application。

CLI 不应成为 GUI 业务契约。

允许保留在 CLI 的内容：

- 参数解析；
- 终端确认；
- REPL history；
- shell/batch 循环；
- CLI 输出兼容适配；
- 本地终端展示逻辑。

不应留在 CLI 的内容：

- 可被 Swift、GTK 或 .NET 复用的设备、文件、传输、信任、同步业务规则；
- Session 生命周期；
- Core error 到 UI error 的解释；
- GUI 也需要的冲突检测和任务状态机。

### 3.4 `handshaker-ffi`

路径：

```text
crates/handshaker-ffi/
```

职责：

- 稳定 C ABI；
- 不透明 Runtime/Subscription 句柄；
- `HsByteBuffer`、`HsCallResult`；
- JSON 请求和结果；
- panic 隔离；
- C Header 和 module map；
- C/Swift smoke；
- ABI 版本。

FFI 只能依赖 Application，不得直接依赖 Core，也不得重新实现业务逻辑。

### 3.5 其他目录

```text
docs/                    协议、架构、迁移和验证文档
proto/                   权威 SSP proto2 schema
locales/                 用户可见语言资源
scripts/                 构建、Header 校验和 smoke 脚本
tools/capture/           协议抓包和复现工具
dist/                    构建暂存产物，不是源代码事实来源
```

`dist/` 中的文件不得手工修改。应由脚本从源码、Header 和构建结果生成。
除非发布策略明确要求，不要把本地临时产物、签名产物或机器相关二进制提交到仓库。

### 3.6 禁止的依赖方向

```text
core        -> application / cli / ffi
application -> cli / ffi
ffi         -> core / cli
cli         -> ffi
```

允许的主要方向：

```text
core
  ↑
application
  ↑
├── cli
└── ffi
```

Rust GTK 前端可直接依赖 Application；Swift、.NET 和其他语言优先通过 FFI。

---

## 4. 当前迁移状态与边界

当前 CLI 的业务命令已经通过 Application，包括：

- `device list/info/ping`；
- 普通命令连接和 Session Registry；
- `fs ls/stat/exists/mkdir/mv/count/rm`；
- `fs pull/push` 的批量实际执行；
- clipboard 全部命令；
- media photo/video/audio/thumbnail；
- trust、watch 与 sync plan/run/watch/status。

以下仍为明确保留的 CLI 边界：

- `device discover` 的 Wi-Fi 广播发现仍直接调用 Core；
- shell/batch/TTY/确认/输出循环；
- Core error 与本地化资源到既有 CLI 退出码和文案的兼容适配。

`shell` 和 `batch` 的终端循环保留在 CLI 是设计决定；循环执行的业务命令
必须继续调用 Application。`session_client()` 过渡入口已经删除，不得重新
引入。修改 `device discover` 时应优先把剩余发现能力迁入 Application，不能
为新命令增加 Core 业务直连。

---

## 5. 不可破坏的协议不变量

### 5.1 握手与兼容身份

- ADB 使用已抓包验证的 USB-style 裸公钥交换；
- USB AOA 连接复用 ADB 裸握手，但 transport 建立流程不同；
- Wi-Fi 使用 REQUEST_01/REQUEST_02 和持久化信任；
- 不得把 ADB/USB 裸握手与 Wi-Fi 信任握手合并成同一未经验证流程；
- RSA 为 1024 位，签名为 SHA-256 with RSA；
- ADB 连接使用临时密钥；
- Wi-Fi derived key 持久化时必须使用安全状态目录和权限；
- `enckey` 使用协议固定 AES-256-CBC 表，修改常量必须有抓包向量和真机证据。

手机端检查的主机兼容身份固定为：

```text
host_app_version = 2.5.6
host_app_version_code = 408
```

它不是：

- Cargo Workspace 版本；
- Application API 版本；
- FFI ABI 版本；
- CLI JSON schema 版本。

没有新的真机证据时，绝对不要修改。

### 5.2 帧与请求状态

- 上行帧：

```text
[sid:u32 BE][flag:u8][len:u32 BE][payload]
```

- 上行 payload 上限为 4 MiB；
- 下行帧：

```text
[sid:u32 BE][chunkLen:u16 BE][chunk]
```

- 下行单块上限为 32761 字节；
- 普通响应使用 8 字节大端总长度重组；
- 下载数据面为无普通长度包络的裸流，以响应头声明长度为准；
- 响应类型以当前请求状态和预期消息为主，不得依赖 protobuf field 1；
- 线上响应可能省略默认字段；
- pending request 必须在发送前注册；
- 完成、取消、超时或关闭后及时移除；
- sid 不能碰撞，主动推送 sid 不能误路由到普通请求；
- 可疑的同 sid 普通消息不得被写入下载文件；
- Ready Session 在长传输期间仍必须维持心跳；
- QUIT、EOF、超时、取消、帧错误和 transport 断开必须进入统一、幂等关闭流程。

### 5.3 取消语义

下载是裸流。取消下载可能需要关闭连接才能保证停止接收，不得伪造“取消后 Session 仍然可用”。

修改取消逻辑时必须确认：

- Transfer 最终状态；
- 临时文件和目标文件；
- Session 是否仍然可用；
- ADB forward、USB interface 或 socket 是否释放；
- Application Session 事件是否同步；
- CLI 和 FFI 对取消结果的描述是否真实。

---

## 6. Runtime、Session 与并发规则

### 6.1 不得持锁跨网络 `await`

禁止：

```rust
let sessions = self.sessions.lock().await;
let session = sessions.get(&id)?;
session.client.list_dir(...).await;
```

必须先在短临界区内复制需要的 `Arc` 或快照，然后释放 Registry 锁：

```rust
let client = {
    let sessions = self.sessions.lock().await;
    sessions.get(&id)?.client.clone()
};

client.list_dir(...).await
```

以下锁不得跨越网络、文件系统或长时间等待：

- Session Registry；
- Transfer Registry；
- Event receiver；
- 全局状态锁；
- 任务 join 锁。

### 6.2 Session 状态必须真实

状态变化应遵循明确路径：

```text
Connecting
  -> Ready
  -> Disconnecting
  -> Closed
```

失败路径：

```text
Connecting / Ready
  -> Failed
  -> Closed
```

规则：

- 不得在物理连接仍运行时提前报告 `Closed`；
- Connection loss 必须更新 Session 状态；
- `last_activity_at_ms` 只有在真实活动发生后才能更新；
- 关闭后新操作返回稳定的 Session/Runtime 错误；
- 一个 Session 上的任务和事件转发任务必须有明确所有者。

### 6.3 `disconnect()` 与 `shutdown()` 必须确定性清理

不要用固定 `sleep` 作为任务已结束的证明。

正确关闭应包括：

1. 拒绝新操作；
2. 标记 Disconnecting/RuntimeStopping；
3. 取消所属 Transfer 和后台任务；
4. 在有界 deadline 内等待任务；
5. 停止事件转发；
6. 显式关闭 Core client；
7. 释放 transport cleanup；
8. 发布最终状态；
9. 从 Registry 删除；
10. 返回真实清理结果。

`Drop` 或 FFI destroy 只能做保守兜底，不能替代正常 shutdown。

### 6.4 多 Runtime 与多 Session

`HandShakerRuntime` 不是全局单例。

实现不得依赖：

- 进程唯一 Runtime；
- 全局当前设备；
- 全局当前 Session；
- 可猜测的固定 SessionId；
- Runtime 之间共享的可变状态，除非明确设计为持久化配置。

SessionId 和 TransferId 只能在所属 Runtime 内解释。

---

## 7. Application API 与 DTO 契约

### 7.1 Application 是 GUI 的唯一业务入口

Swift、GTK 和 .NET 不得：

- 直接调用 Core；
- 解析 SSP；
- 维护 sid；
- 清理 adb forward；
- 复制远端路径解析；
- 根据本地化错误文本判断错误类型；
- 重建 Session/Transfer 状态机。

这些逻辑应在 Core 或 Application 中完成。

### 7.2 不泄露内部类型

Application 公开 API 不得出现：

- Prost 类型；
- protocol frame；
- `Session`；
- `TransportConnector`；
- `HandShakerClient`；
- 密码学常量；
- pending request；
- CLI 输出类型。

公开数据使用 Application DTO。

### 7.3 版本与兼容性

当前代码中的：

```rust
APPLICATION_API_VERSION = "1.0.0"
```

与 Cargo Workspace 版本独立。v1 契约已经冻结；破坏公开 Rust/JSON 契约
必须执行 major 递增规则。

任何公开契约变化必须判断：

- 仅实现修复：通常不改变 Application API 版本；
- 增加新方法或可选字段：评估 minor；
- 删除方法、重命名字段、改变必填字段、改变 enum JSON token 或函数签名：breaking change；
- Rust 源码级变化即使 JSON 不变，也要在迁移说明中记录；
- 未完成迁移的临时接口不得被当成永久 v1 契约。

公开 DTO 和错误应有 serde fixture，至少覆盖：

- 字段名；
- enum snake_case token；
- Optional/null；
- u64 时间和字节；
- 未知 enum 的处理；
- PublicError；
- EventEnvelope；
- TransferSnapshot。

不得复用已发布 enum 判别值或错误码。

### 7.4 路径和本地文件

远端路径规则由 Application 统一负责：

- 相对路径基于设备 root；
- `.` 和空路径归一；
- `..` 不得逃逸 root；
- CLI、Swift 和 FFI 不重复实现。

本地路径使用平台原生 Path/PathBuf。
跨 FFI 使用 UTF-8 字符串时，无法表示的路径必须返回明确错误，不得静默损坏。

### 7.5 配置字段必须真实生效

如果 API 接受：

- `state_dir`；
- `wire_log`；
- timeout；
- heartbeat；
- event capacity；
- transfer history 配置；

则实现必须真实使用。

禁止：

- 解析后丢弃；
- 用默认值覆盖调用方指定值；
- 对不支持字段静默成功；
- 文档声称支持但代码忽略。

状态目录必须可由 macOS App Container、测试 tempdir 和多 Runtime 场景明确控制。

---

## 8. 事件与 Transfer 规则

### 8.1 Core 事件必须在 Application 映射

Core typed events 不能直接穿过 FFI。

Application 负责映射为稳定的 `BackendEvent`，包括需要时的：

- device info changed；
- clipboard changed；
- media changed；
- remote file changed；
- Session state changed；
- Transfer updated；
- warning；
- connection lost。

未知 Core event：

- 不得 panic；
- 不得泄露 payload；
- 可以映射为安全 Warning/Unknown；
- 必须保持 EventHub 可继续工作。

### 8.2 EventEnvelope

事件必须包含：

```text
sequence
timestamp_ms
event
```

规则：

- sequence 在单 Runtime 内单调；
- Lagged 必须显式上报；
- Closed 必须可识别；
- 终态事件不得被节流丢弃；
- 事件 schema 变化必须有 fixture 和版本决策。

### 8.3 Transfer 状态

状态只允许单向变化：

```text
Queued
  -> Running
  -> Completed | Failed | Cancelled
```

终态不得被后续任务结果覆盖。

TransferSnapshot 应真实维护：

- transferred bytes；
- total bytes；
- started/finished time；
- PublicError；
- SessionId；
- source/destination；
- direction。

进度事件应节流，避免每个数据块都跨 FFI：

- 可按时间或字节阈值；
- 建议不超过约 10–20 次/秒；
- 完成、失败和取消无条件立即发布。

取消时必须：

- 触发 CancellationToken；
- 设置最终状态和 finished time；
- 发布终态事件；
- 处理取消导致的 Session 失效；
- 清理临时文件；
- 保证后台任务不能回写为 Completed。

Transfer history 必须有容量或 TTL 策略，不能无限增长。

---

## 9. FFI 与 C ABI 规则

### 9.1 当前 ABI

Rust 源码当前声明：

```text
ABI 1.5.0
```

FFI ABI 与 Cargo Workspace、Application API 和 CLI schema 独立。

任何发现的 Rust 常量、Header、文档、Swift 检查不一致都属于阻塞性 bug，应优先修复。

版本规则：

- major：删除/重命名 symbol、改变 C 签名、结构布局或所有权；
- minor：增加 symbol、增加兼容的可选请求字段；
- patch：不改变 ABI 的实现修复。

### 9.2 所有权

- Rust 分配的内存只能由 Rust 释放；
- `HsCallResult` 使用 `hs_call_result_free`；
- 单独 buffer 使用 `hs_byte_buffer_free`；
- 空 buffer 为 `{NULL, 0, 0}`；
- destroy/free 对 NULL 安全；
- double-free、use-after-free 是调用方错误；
- Swift wrapper 不得暴露可复制的裸 handle。

### 9.3 Panic 与错误

每个 extern 函数必须：

- 使用 panic boundary；
- 不允许 unwind 穿过 C ABI；
- NULL、invalid UTF-8、invalid JSON 返回稳定 PublicError；
- 不通过 stderr 或进程退出报告普通错误；
- 不返回借用 Rust 临时内存的指针。

### 9.4 线程模型

当前短调用会在 FFI Runtime 的 Tokio executor 上同步 `block_on`。

规则：

- Swift 主线程禁止直接调用；
- Swift wrapper 应使用 actor、后台 Task 或专用队列；
- 同一 Runtime 的 destroy 不能与普通调用并发；
- Subscription polling 使用独立后台任务；
- 长任务使用 ID + events/polling，不把整个传输阻塞在一个 FFI 调用中。

### 9.5 Header 是生成或校验产物

修改导出函数时必须同步：

- Rust extern；
- `handshaker_ffi.h`；
- ABI 常量；
- `docs/ffi-v1.md`；
- module map（若需要）；
- C smoke；
- Swift smoke；
- symbol/signature 校验。

仅检查函数名存在不够。应尽量让 CI 编译 Header 和调用方，以发现签名不一致。

### 9.6 JSON 与二进制

复杂 DTO 可使用 UTF-8 JSON。

大二进制不得序列化成巨大 JSON 数字数组，尤其是：

- 缩略图；
- 图片；
- 音频封面；
- 文件正文。

优先设计：

1. Rust 直接写缓存文件并返回路径/标识；
2. 独立 byte buffer API；
3. 有明确数量和大小上限的 buffer table。

文件上传下载正文继续由 Rust 直接读写文件，不经过 JSON。

---

## 10. Swift、GTK 与 .NET 接入规则

> 仓库定位：本仓库是 Rust 后端，不交付 GUI 应用工程；Swift/GTK/.NET 包装
> 层不属于本仓库（Swift SDK 位于 `platform/macos/HandShakerCore`，作为
> 后端能力证明与 CI 验证对象，GUI 应用工程由外部项目承载）。
> 平台策略：现阶段适配目标为现代 macOS；其他平台暂不承诺适配，但架构
> 保持未来多平台兼容（transport/平台实现隔离在 Core，Application 为 UI
> 无关契约，FFI 为稳定 C ABI）。

### 10.1 Swift

推荐 Swift 层结构（已落地于 `platform/macos/HandShakerCore`）：

```text
platform/macos/HandShakerCore/
├── Native/
│   ├── RuntimeHandle
│   ├── NativeCall
│   └── NativeError
├── Models/
├── Services/
│   ├── DeviceService
│   ├── FileService
│   ├── TransferCenter
│   ├── ClipboardService
│   └── MediaService
└── EventStream
```

规则：

- SwiftUI View 不直接调用 C 函数；
- 不向 View 暴露 `OpaquePointer` 或 C struct；
- Runtime 使用 RAII，并由 actor/锁保护；
- Session 和 Transfer 持有 Runtime 生命周期；
- Event subscription 包装为 `AsyncThrowingStream`；
- PublicError 解码为稳定 Swift error；
- ABI 版本在初始化时校验；
- FFI 调用不在 MainActor；
- shutdown 只能执行一次，deinit 只做兜底。

macOS security-scoped URL：

- 用户选择的上传/下载路径在 Transfer 完成前必须保持授权；
- `startAccessingSecurityScopedResource()` 不能在开始 FFI 调用后立即结束；
- 完成、失败或取消后再释放；
- 书签恢复失败必须向用户报告。

正式 Apple 交付优先使用静态 XCFramework + Swift Package，不依赖开发机裸 `.dylib` 路径。

### 10.2 GTK

Rust GTK 前端可直接依赖 `handshaker-application`，不得绕过 Application 调 Core。

平台 UI 逻辑留在 GTK 工程，不进入 Core/Application。

### 10.3 .NET

.NET 通过 C ABI/PInvoke：

- 使用 `SafeHandle`；
- 明确 UTF-8；
- C ABI struct 使用固定布局；
- Rust buffer 由 Rust free；
- 异步 UI 调用放后台线程；
- 不在托管层复制 Session 状态机。

### 10.4 跨平台公共原则

平台差异应位于：

- UI 包装层；
- Application 配置；
- Core transport 的平台实现。

不要在协议代码中散布 Swift、AppKit、WinUI、GTK 或 .NET 条件分支。

---

## 11. CLI 与输出兼容性

- 可执行文件名固定为 `handshaker`；
- CLI crate 为 `handshaker-cli`；
- 一次性命令、shell 和 batch 尽量共用同一 Clap 命令和业务执行逻辑；
- 默认 human 输出为中文；
- JSON 字段、命令、事件和错误 code 固定使用英文；
- JSON envelope `schema_version` 当前为 `1`；
- JSON schema 变化必须更新 fixture、文档和迁移说明；
- `json` 只输出一个最终对象；
- `jsonl` 可输出事件和进度；
- 日志、提示不得混入 JSON stdout；
- 删除、覆盖、清空等危险操作遵循统一确认；
- 非 TTY 或 JSON 模式除操作开关外仍需 `--yes`；
- 不得为了 Application 迁移随意改变 CLI 旧输出；
- 确有不兼容变化时必须记录并更新版本/测试。

退出码保持稳定：

```text
2   参数错误
3   配置或设备选择
4   连接或握手
5   协议
6   手机端错误
7   本地 I/O
8   缺少确认
130 用户中断
```

Application 的 PublicError 不得简单全部折叠成同一个 CLI Transport error；
修改映射时检查旧退出码和 JSON error envelope。

---

## 12. 错误、诊断与发现

### 12.1 使用结构化错误

程序判断必须使用：

- Core ErrorCode；
- Application PublicErrorCode；
- enum 或明确字段。

不得解析：

- 中文文案；
- `Display` 字符串；
- 第三方错误文本；

来决定程序逻辑。

### 12.2 不吞错

禁止：

```rust
let _ = error;
Err(_) => {}
```

除非：

- 该错误确实是 best-effort；
- 调用者仍能通过 warnings、events 或 diagnostics 观察；
- 文档明确说明 partial success。

设备发现需要区分：

- 确实无设备；
- ADB 不可用；
- ADB unauthorized/offline；
- Wi-Fi mDNS 失败；
- USB 枚举或权限失败。

如果接口允许部分成功，应返回 devices + warnings，而不是把所有失败变成空数组。

### 12.3 错误映射

不要把所有：

- RemoteIo 映射成 NotFound；
- LocalIo 映射成 NotFound；
- Handshake 映射成 TrustRejected；
- Cancelled 映射成 TransferCancelled。

应优先使用 Core 结构化错误码和 operation 上下文区分：

- not found；
- permission denied；
- already exists；
- no space；
- unauthorized；
- offline；
- trust required；
- trust rejected；
- user cancelled；
- remote cancelled；
- connection lost。

错误 `detail` 不得包含密钥、payload 或用户文件正文。

---

## 13. ADB、Wi-Fi、USB 与设备安全

### 13.1 ADB

- 服务组件固定使用已验证的
  `com.smartisanos.smartfolder.aoa/.service.ConnectionManagerService`；
- 使用 `adb forward tcp:0 tcp:10086` 获取动态端口；
- 只清理本进程明确创建的 forward；
- 无法唯一识别时返回错误，不猜测或误删；
- 默认不执行 `force-stop`；
- 不尝试未经验证的备用包名、服务或端口；
- 未指定 serial 时，只在恰好一台在线设备时自动选择；
- `device list` 不应启动服务或建立 forward。

### 13.2 Wi-Fi

- mDNS SRV 端口是动态值，不能长期缓存；
- 首次信任与重连使用不同状态；
- derived key 不得出现在日志、FFI JSON 或错误 detail；
- Device discovery endpoint 不是稳定设备身份；
- 连接后优先使用手机 UUID 建立稳定 identity；
- trust state 目录必须由 Runtime 配置真实控制。

### 13.3 USB AOA

- AOA identification 和 vendor/product 规则必须以抓包、原版实现和真机结果为准；
- libusb 的 open/control/claim 是阻塞操作时，应放到 `spawn_blocking`；
- 只释放当前连接持有的 interface/handle；
- 不把 USB retry 扩展为无限重试；
- Linux udev、Windows driver 和 macOS 权限属于平台交付事项，不能假设已自动解决。

### 13.4 真机安全

只有在用户已授权并指定测试目标时才操作真机。

测试必须：

- 使用唯一测试目录；
- 不读取或删除无关用户文件；
- 不上传敏感测试数据；
- 测试后删除临时文件；
- 清理 adb forward；
- 关闭 Session；
- 明确报告使用了哪个 transport；
- 未进行真机测试时明确说明。

---

## 14. 文案、日志与敏感数据

### 14.1 国际化

- CLI 用户可见文本放在 `locales/<language>.json`；
- Rust 源码只引用稳定英文 key；
- 当前中文资源为 `zh-CN`；
- 新语言保持 key 和占位符一致；
- 不在生产 Rust 源码中硬编码中文用户文案；
- JSON key、错误 code、命令和协议常量不本地化；
- 测试会扫描 CJK 文案，修改后必须运行 localization 测试。

Application 和 FFI 的稳定错误 `message` 可以是安全的默认文本，但程序逻辑只看 code。

### 14.2 日志

普通日志只能记录必要元数据，例如：

- sid；
- flag；
-长度；
- transport；
-状态；
- operation；
-安全的设备标识摘要。

不得记录：

- protobuf payload；
-剪贴板正文；
-文件正文；
-derived key；
-私钥；
-完整抓包；
-用户隐私路径，除非用户明确开启诊断并知情。

### 14.3 状态文件

- 配置目录在 Unix 上保持 `0700`；
- 敏感文件保持 `0600`；
- host UUID 必须稳定；
- sync ledger 原子写入；
-损坏状态文件不能静默重建导致数据丢失；
- `wire_log` 必须显式开启；
- 文档和 UI 必须提示 wire log 可能包含敏感内容。

---

## 15. protobuf、依赖和生成物

- 使用 `prost-build` 和 vendored `protoc`；
- schema 来源为 `proto/smartsync.proto`；
- 生成 Rust 文件放在 `OUT_DIR`；
- 不提交或手改生成 protobuf Rust 文件；
- 不为测试拆出 `handshaker-test-support` 而公开 protocol/crypto 内部，除非用户批准架构变更；
- 当前 fake SSP 保持 Core 内部；
- 新依赖前先确认标准库和现有依赖不足；
- 不为未来可能使用的功能提前加入大型框架；
- 避免同时存在多个功能重叠的 async/runtime/serialization/binding 框架；
- 升级依赖应单独评估协议、MSRV、平台和产物影响。

---

## 16. 版本策略

项目存在四类独立版本：

### 16.1 Cargo Workspace 版本

根 `Cargo.toml` 的：

```toml
[workspace.package]
version = "..."
```

用于 Core、Application、CLI 和 FFI crate 发布版本。

规则：

- bug 修复和小功能通常递增 patch；
- 文档、注释、格式和不改变行为的测试调整不递增；
- 上一轮版本已经为同一未完成工作递增时，不重复递增；
-较大、不兼容或 milestone 版本由用户决定；
-版本变化同步更新 `Cargo.lock` 和 README 中相关说明。

### 16.2 Application API 版本

`APPLICATION_API_VERSION` 表示公开 Application 契约。

- 与 Cargo 版本独立；
- 破坏公开 Rust/JSON 契约时必须评估 major；
- 添加兼容能力时评估 minor；
- 临时迁移接口不应被永久冻结。

### 16.3 FFI ABI 版本

- 与 Cargo 和 Application 独立；
- C symbol/signature/struct/ownership 破坏为 major；
- 增 symbol 为 minor；
- 实现修复为 patch；
- Rust、Header、文档和调用方检查必须一致。

### 16.4 CLI schema

CLI JSON envelope `schema_version = 1` 是另一套兼容契约。

不要将以上任何版本与：

```text
host_app_version = 2.5.6
host_app_version_code = 408
```

混淆。

---

## 17. 实施工作流

每个 Agent 任务按以下顺序执行。

### 17.1 开始前

1. 阅读本文件；
2. 阅读任务涉及的文档；
3. 执行：

```sh
git status --short
git branch --show-current
```

4. 保留用户已有修改；
5. 使用 `rg` 搜索：
   - 现有实现；
   - 公共 API；
   - 测试；
   - 文档；
   - 反编译依据；
6. 识别影响的契约：
   - protocol；
   - CLI；
   - Application；
   - FFI；
   - Swift/跨平台；
   - 版本。

不要在不理解现有调用链时直接重写。

### 17.2 修改中

- 做最小且完整的根因修复；
- 不顺手重构无关模块；
- 不用备用流程掩盖根因；
- 不吞错；
- 不破坏无关兼容性；
- 复制或移动文件时保留 Git 历史；
- 优先使用小而可审查的提交；
- 新公开 API 同时加入文档和 fixture；
- 新后台任务同时设计关闭、取消和 join；
- 新配置字段同时实现真实行为；
- 新 FFI symbol 同时补 Header 和 smoke。

### 17.3 修改后

1. 运行对应测试；
2. 运行格式和 Clippy；
3. 检查：

```sh
git diff --check
git diff --stat
git diff
```

4. 确认没有：
   - 临时文件；
   - 构建目录；
   - 密钥；
   - 抓包正文；
   - 用户数据；
   - 机器绝对路径；
   - 残留 forward；
5. 更新文档；
6. 按版本策略处理版本；
7. 给出实际验证结果。

---

## 18. 分层测试要求

### 18.1 通用代码检查

代码变更提交前至少执行：

```sh
cargo fmt -- --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo build --release
git diff --check
```

### 18.2 Core

协议、Session、transport、传输或同步变更必须运行相关定向测试，并确保覆盖：

- 32761 下行块；
- 4 MiB 上行 payload；
- 8 字节长度跨帧；
- 下载裸流；
- field 1 缺失；
- sid 递增、碰撞和推送；
- timeout、EOF、取消和重复关闭；
- fake adb forward 精确清理；
- Wi-Fi trust；
- USB buffer overflow retention；
- batch partial failure；
- sync ledger 原子性和冲突。

### 18.3 Application

Application 变更应覆盖：

- DTO JSON fixture；
- enum token；
- PublicError mapping；
- path normalization；
- Runtime create/shutdown；
- Session 状态；
- 多 Session 并发；
- 不持锁跨 await；
- Transfer 单向状态；
- progress/total；
- cancel/final event；
- EventHub sequence、Lagged、Closed；
- operation after shutdown；
- state_dir 和配置生效。

### 18.4 CLI

CLI 变更应覆盖：

- Clap 命令树；
-中文帮助；
-human/JSON/JSONL；
-schema version；
-退出码；
-非 TTY 确认；
-shell/batch；
-旧 JSON fixture；
- Application 错误到 CLI error 的映射。

### 18.5 FFI

FFI 变更应执行：

```sh
scripts/generate-ffi-header.sh
scripts/run-ffi-smoke-tests.sh
```

并覆盖：

- Header 可编译；
- C link；
- Swift import/link；
- ABI version；
- Runtime create/shutdown/destroy；
- NULL；
- invalid UTF-8/JSON；
- panic boundary；
- success 与 error buffer；
- double ownership 不发生；
- event timeout/closed/lagged；
- Transfer start/get/list/cancel；
- 新增 symbol 的成功和失败路径。

只测试“missing session”不足以证明功能可交付。
能使用 fake device 时，应增加真实成功路径。

### 18.6 Apple 产物

涉及 Apple 交付时还应验证：

```sh
scripts/build-ffi-macos.sh
```

以及：

- arm64；
- 需要时 x86_64；
- static link；
- XCFramework；
- Swift Package tests；
- rpath 不依赖开发机绝对路径；
- symbol 和 Header 一致；
- App Sandbox 路径/授权；
- 签名和 Hardened Runtime 策略。

### 18.7 Linux/Windows

声称跨平台可用前必须有对应构建或测试。

Linux：

```sh
scripts/build-ffi-linux.sh
```

并检查 libusb/udev、`.so`、C smoke。

Windows：

- Core/Workspace build；
- `cdylib`/DLL；
- C Header；
- C# PInvoke smoke；
- USB driver/权限说明。

macOS CI 通过不能被描述成 Linux 或 Windows 已验证。

### 18.8 文档变更

仅文档变更至少执行：

```sh
git diff --check
```

若文档包含命令、路径、版本、API 或 symbol，应静态核对真实代码。

---

## 19. 真机验收

涉及 transport、握手、Session、文件、传输、剪贴板、媒体、watch 或 sync 的高风险变更，
在自动化测试后应建议或执行受控真机验收。

基础验收：

1. 指定设备和 transport；
2. 连接并读取完整设备信息；
3. ping；
4. 列根目录；
5. 在唯一测试目录创建目录；
6. 上传文件；
7. 下载并校验 MD5；
8. 重命名和删除；
9. 剪贴板读写；
10. 发送 QUIT；
11. 确认无残留 forward/handle。

按功能追加：

- Wi-Fi 首次信任和重连；
- trust remove/reset；
- USB AOA 单连接批量；
-媒体库和缩略图；
-目录监控；
-照片同步；
-传输取消；
-设备中途断开；
-App 退出时存在活动 Transfer。

未经用户授权不要扩大测试范围。

最终报告必须明确区分：

```text
自动化测试通过
C/Swift smoke 通过
真机 ADB 通过
真机 Wi-Fi 通过
真机 USB 通过
尚未验证
```

不要把静态检查描述成真机互通。

---

## 20. 文档同步规则

修改架构或公共接口时检查并更新：

```text
README.md
docs/architecture.md
docs/application-api-v1.md
docs/ffi-v1.md
docs/m8-migration.md
```

修改协议时更新对应协议文档和验证状态。

规则：

- 文档不得领先代码宣称功能已完成；
- 文档不得保留已失效的单 crate 路径；
- 版本、symbol、函数和字段必须与源码一致；
- README 的当前状态必须区分 Core、Application、FFI 和 GUI 交付程度；
- migration 文档保留历史，但最新结论必须明确；
- `dist/` 产物不能作为 API 文档唯一来源。

---

## 21. 当前阶段的高优先级约束

在 Swift 正式交付与真机发布验收前，优先处理以下事项，不要继续扩大债务：

1. 保持 FFI Rust 常量、Header、文档、ABI snapshot 与 Swift 检查一致；
2. 完成媒体分页、同步崩溃恢复和断连/取消的真机故障注入验收；
3. 完成静态 XCFramework/Swift Package 的签名、公证和干净消费者验证；
4. 评估并实现 discovery watcher 后再发布 `RuntimeStarted`/`DeviceAdded`/
   `DeviceRemoved`，不得让文档领先运行时；
5. 为 `sync_ledger_status` 的 v1 Core 类型泄漏设计 Application DTO 替代入口，
   在兼容版本周期内迁移，不能继续新增类似泄漏；
6. 迁移 CLI 最后的 `device discover` Core 业务直连；
7. 继续维持有界队列、任务所有权、确定性 shutdown 和 JSON/事件 fixture。

若任务与上述事项无关，不要顺手大规模重构；但不得引入会让这些问题更难解决的新依赖和新 API。

---

## 22. 禁止模式

以下模式原则上禁止：

- 根据方法名猜协议；
- 用本地化字符串做逻辑判断；
- `Err(_) => {}` 静默吞错；
- 接受配置后不使用；
- 用固定 sleep 代替同步；
- 持全局锁跨网络 await；
- 在 FFI 中重新实现业务；
- SwiftUI View 直接调用 C；
- FFI 直接依赖 Core；
- Application 暴露 Prost 或 `HandShakerClient`；
- 将文件正文或大缩略图放入 JSON 数字数组；
- 无界增长 Session/Transfer/Event 历史；
- Drop 代替正常 shutdown；
- 取消后伪造 Session 可用；
- 未测试就声称跨平台；
- 未真机验收就声称协议互通；
- 提交生成 protobuf 文件；
- 提交真实密钥、抓包、用户文件或 wire log；
- 为未来可能的功能提前加入大型框架；
- 在同一任务中引入第二套绑定或第二套业务 API；
- 覆盖用户已有无关修改；
- 修改手机兼容身份为 Cargo 版本。

---

## 23. Agent 交付报告格式

完成任务后，报告至少包含：

### 变更

- 修改了哪些文件；
- 修复或实现了什么；
- 所属层级：Core/Application/CLI/FFI/Swift/文档；
- 是否改变公开契约。

### 兼容性

- Cargo 版本；
- Application API；
- FFI ABI；
- CLI JSON schema；
- host compatible identity；
- 是否有 breaking change。

### 验证

列出实际执行的命令和结果，不要只写“测试通过”。

### 真机

明确：

- 使用了哪种 transport；
- 测试了哪些操作；
- 是否清理测试文件和 forward；
- 或说明尚未真机验证。

### 未完成或风险

- 仍未覆盖的路径；
- 平台限制；
- 需要用户决定的版本或架构事项；
- 不确定性及证据不足处。

---

## 24. 完成标准

一个任务只有在以下条件满足时才算完成：

- 请求功能完整实现，不是只添加接口或占位；
- 协议行为有证据；
- 分层方向正确；
- Application 未泄露 Core 内部；
- FFI 未复制业务逻辑；
- 生命周期、取消和清理有确定语义；
- 新配置真实生效；
-错误可观察且不吞错；
-公共 DTO、事件和错误有 fixture；
-CLI 输出兼容，或不兼容变化已明确记录；
-Header、ABI 和文档同步；
-敏感数据未泄露；
-必要自动化检查通过；
-需要时完成 C/Swift smoke；
-需要时完成受控真机验收；
-版本策略正确；
-最终 diff 无无关修改；
-最终报告诚实列出已验证和未验证事项。
