# HandShaker_Rust 后端现状与完整开发计划

> 状态基线：2026-08-02，Cargo package `handshaker_rust 0.1.2`。
>
> 本文只把已经存在于 Rust 代码中的能力标记为“已实现”。协议文档、proto schema 或抓包已经确认，
> 但尚未形成正式 Rust API/CLI 的能力，仍标记为“未实现”。

## 1. 项目目标

HandShaker_Rust 的目标是提供一个兼容原版 Smartisan HandShaker 的跨平台 Rust 后端，并以
`handshaker` CLI 作为首个完整调用入口。后端最终应覆盖：

- ADB、WiFi 和 USB AOA 三种连接通道；
- SSP 握手、信任、封帧、签名、心跳、请求路由、主动推送和关闭；
- 设备、文件、传输、剪贴板、媒体库、目录监控和照片同步；
- 可供 CLI、未来 GUI 和自动化工具复用的稳定 Rust library；
- 中文 human 输出以及稳定的 JSON/JSONL 自动化接口。

当前优先级仍是先把无 daemon 的本地 library + CLI 做完整，再考虑 GUI。本文不包含 GUI 视觉设计。

## 2. 状态定义

| 标记 | 含义 |
|---|---|
| ✅ 已实现 | 正式 Rust 代码中已有可调用路径，并有相应测试或运行验证入口 |
| 🟡 部分实现 | 已有底层结构或内部逻辑，但尚未形成完整、稳定的业务能力 |
| ⬜ 未实现 | 协议可能已经逆向或抓包确认，但正式 Rust 后端尚未实现 |
| 🔬 待验证 | 实现前还需要补充真机或字节级验证 |

## 3. 当前已经实现的后端能力

### 3.1 工程与公开 library

- ✅ 单 Cargo package 同时提供 `handshaker_rust` library 和 `handshaker` binary。
- ✅ `src/lib.rs` 导出连接配置、客户端、稳定错误类型和领域模型。
- ✅ CLI 和未来 GUI 不需要直接依赖 Prost 生成结构。
- ✅ `proto/smartsync.proto` 使用 proto2，通过 `prost-build` 生成到 `OUT_DIR`。
- ✅ 使用 vendored `protoc`，开发机不需要另外安装 protobuf 编译器。
- ✅ Tokio 异步运行时、异步 TCP、异步 adb 子进程和独立读写任务已经接入。
- ✅ 用户配置状态文件支持稳定 host UUID、schema version 1、Unix `0600` 文件权限和 `0700`
  目录权限。
- ✅ 状态结构已预留 WiFi 信任记录字段，但 ADB 当前不依赖这些记录。
- ✅ 所有用户可见中文文案集中在 `locales/zh-CN.json`，Rust 源码只使用稳定消息 key。

### 3.2 ADB 传输连接

- ✅ 读取并解析 `adb devices -l`。
- ✅ 指定 `--serial` 时只连接目标在线设备。
- ✅ 未指定设备时，仅在恰好一台在线设备的情况下自动选择。
- ✅ 启动已验证的手机服务组件，并传入 `ADB_PORT=10086`。
- ✅ 使用 `adb forward tcp:0 tcp:10086` 获取动态本地端口。
- ✅ 等待本地转发端口就绪并建立 Tokio TCP 连接。
- ✅ 正常退出和异常路径只清理当前连接创建的 adb forward。
- ✅ 动态端口输出异常时，通过前后 forward 差集进行保守清理；无法唯一识别时拒绝猜测。
- ✅ adb 子进程、输出收集、端口连接和清理均受超时约束。
- ✅ `device list` 只读取设备列表，不启动服务、不创建 forward。

### 3.3 ADB 握手与密码学

- ✅ ADB 使用真实抓包验证的 USB-style 裸公钥交换。
- ✅ 每次连接临时生成 RSA-1024 密钥。
- ✅ `enckey = MD5(DER) + AES-256-CBC(PKCS7(base64(DER)))` 已实现。
- ✅ 使用协议固定 AES key/IV 表。
- ✅ 上行业务请求使用 SHA-256 with RSA 签名，签名长度为 128 字节。
- ✅ 能识别手机返回的 `failed`、`locked`、`needauth` 和加密 `ok`。
- ✅ `GET_DEVICE_INFO` 向手机报告兼容身份 `2.5.6 / 408`，与 Cargo 版本独立。

### 3.4 SSP 封帧与 Session

- ✅ 上行帧：`sid:u32 BE + flag:u8 + len:u32 BE + payload`。
- ✅ 上行 payload 4 MiB 限制。
- ✅ 下行帧：`sid:u32 BE + chunkLen:u16 BE + chunk`。
- ✅ 下行单块 32761 字节限制。
- ✅ 普通响应按 8 字节大端总长度跨帧重组。
- ✅ 下载按响应头声明长度接收无 8 字节包络的裸数据流。
- ✅ 下载流与可疑普通消息包络无法安全判别时主动报协议错误，不写入可疑数据。
- ✅ 请求发送前注册 sid，响应按 sid 路由，完成后移除 pending 请求。
- ✅ 上行通过独立串行写任务发送，保证单帧原子写入。
- ✅ 连接 Ready 后周期性发送心跳，上传和下载期间同样维持心跳。
- ✅ 请求级超时、EOF、帧错误、写任务失败和主动关闭进入统一关闭路径。
- ✅ QUIT 请求和 adb forward 清理。
- 🟡 未匹配 sid 的完整普通消息能够被组装并进入内部事件队列，但目前只记录日志，没有公开为强类型
  事件流。
- 🟡 请求对象被丢弃时可以发送 flag 2 取消，但手机不会中断已经开始的下载裸流；当前没有完整的
  公共取消令牌/API。

### 3.5 设备能力

- ✅ `device list`：列出 ADB 设备。
- ✅ `device info` / `HandShakerClient::device_info()`：读取手机名称、型号、品牌、系统、APK、根目录、
  存储、电量和锁屏状态等信息。
- ✅ `device ping` / `HandShakerClient::ping()`：发送 SSP 心跳并报告往返延迟。
- ✅ 连接建立后自动获取设备信息和设备 `root_path`。

### 3.6 文件系统能力

- ✅ 列目录：`list_dir(path, depth)` / `fs ls`。
- ✅ 文件计数：`file_count(path, depth, exclusions)` / `fs count`。
- ✅ 存在性检查：`file_exists(path)` / `fs exists`。
- ✅ 路径信息：`stat(path)` / `fs stat`；当前通过根目录信息或父目录列表查找实现。
- ✅ 创建目录：`create_dir(path)` / `fs mkdir`。
- ✅ 重命名或移动：`rename(source, target)` / `fs mv`。
- ✅ 删除一个或多个远端路径：`delete(paths, options)` / `fs rm`。
- ✅ 删除目录要求显式 `--recursive`；该参数当前授权删除目录，但不在电脑端递归遍历并逐项删除。
- ✅ 支持协议 `is_trash` 删除选项。
- ✅ 逐文件检查删除响应中的 `succeed` 和 `error_code`，不会把部分失败报告为成功。
- ✅ 一次性命令的相对远端路径以设备 `root_path` 为基准。
- ✅ shell 中相对远端路径以当前远端目录为基准。

### 3.7 单文件传输

- ✅ 单文件下载：`download(remote, local, options)` / `fs pull`。
- ✅ 下载前处理目标覆盖策略，写入唯一临时文件，完成并校验后再原子移动到目标。
- ✅ 下载按手机响应头中的 MD5 校验内容；失败时保留原目标不变并清理临时文件。
- ✅ 单文件上传：`upload(local, remote, options)` / `fs push`。
- ✅ 上传前验证本地目标是普通文件，计算 MD5 和长度。
- ✅ 按 4 MiB 上行限制分块发送 flag 3 数据。
- ✅ 处理上传 ready、完成、失败和 canceled 响应。
- ✅ human 进度和 JSONL 进度事件。
- ✅ 覆盖默认拒绝；需要 `--overwrite`，非 TTY 或 JSON 环境还需要 `--yes`。

### 3.8 剪贴板

- ✅ 获取剪贴板列表并解压 gzip 内容：`clipboard_list()` / `clipboard get`。
- ✅ 写入文本并 gzip 压缩：`clipboard_set(text)` / `clipboard set`。
- ✅ 支持命令参数或 `--stdin` 输入。
- ✅ 按时间戳删除条目：`clipboard_delete(timestamp)` / `clipboard delete`。
- ✅ 清空剪贴板：`clipboard_clear()` / `clipboard clear`。
- ✅ 删除和清空遵循统一确认策略。

### 3.9 CLI、REPL、输出和安全

- ✅ 分组命令：`device`、`fs`、`clipboard`、`shell`。
- ✅ 全局参数：`--serial`、`--output`、`--timeout`、`--yes`、`-v/--verbose`、`--wire-log`。
- ✅ REPL 复用相同 Clap 命令树和业务执行逻辑。
- ✅ shell 内建 `pwd`、`cd`、`lpwd`、`lcd`、`help`、`exit`。
- ✅ Ctrl-D 正常关闭；Ctrl-C 取消当前请求，下载期间通过关闭连接确保电脑端停止接收。
- ✅ human、JSON、JSONL 三种输出格式。
- ✅ JSON 成功/错误 envelope、英文稳定字段、英文错误 code 和固定退出码。
- ✅ 危险操作在非交互环境中要求 `--yes`。
- ✅ `--wire-log` 显式启用、启动时警告，并以 `0600` 权限记录完整线路数据。
- ✅ 普通日志不记录 payload。

### 3.10 当前自动化验证

- ✅ 帧大小端、4 MiB 上行限制和 32761 字节下行边界测试。
- ✅ 普通响应 8 字节长度跨帧、超长拒绝和下载裸流判别测试。
- ✅ RSA 签名、错误签名、AES/enckey 和握手往返测试。
- ✅ 写队列超时、调用方取消和 sid 请求路由测试。
- ✅ 假 adb 的设备解析、超时、动态端口和精确清理测试。
- ✅ CLI JSON envelope、中文帮助和 `device list` 无副作用测试。
- ✅ Rust 源码 CJK 文案扫描，防止用户文本重新硬编码。

## 4. 已写入协议文档但尚未实现的功能

### 4.1 SSP 请求类型覆盖表

| SSPRequestType | 协议功能 | Rust 后端状态 |
|---:|---|---|
| 1 | HEART_BEAT | ✅ Session 心跳和 `device ping` |
| 2 | GET_DEVICE_INFO | ✅ 连接初始化和 `device info` |
| 3 | GET_THUMBNAIL | ⬜ 未实现 |
| 4 | GET_PHOTO_LIB | ⬜ 未实现 |
| 5 | GET_VIDEO_LIB | ⬜ 未实现 |
| 6 | GET_AUDIO_LIB | ⬜ 未实现 |
| 7 | GET_DIR_FILES | ✅ 已实现 |
| 8 | GET_FILE_COUNT | ✅ 已实现 |
| 9 | GET_FILE_EXIST | ✅ 已实现 |
| 10 | GET_CREATE_FOLDER | ✅ 已实现 |
| 11 | GET_RENAME_FILE | ✅ 已实现 |
| 12–14 | DOWNLOAD 请求、响应头、数据面 | ✅ 单文件全量下载；⬜ range/resume 未实现 |
| 15–18 | UPLOAD 请求头、ready、数据面、完成 | ✅ 单文件上传 |
| 19 | DELETE_FILE | ✅ 已实现 |
| 20 | PHOTO_LIB_CHANGE | ⬜ 未实现强类型推送 |
| 21 | AUDIO_LIB_CHANGE | ⬜ 未实现强类型推送 |
| 22 | VIDEO_LIB_CHANGE | ⬜ 未实现强类型推送 |
| 23–25 | MONITOR_FOLDER 请求、确认、回调 | ⬜ 未实现 |
| 26 | GET_CLIPBOARD | ✅ 已实现 |
| 27 | POST_CLIPBOARD | ✅ 已实现 |
| 28 | CLEAR_CLIPBOARD | ✅ 已实现 |
| 29 | DELETE_CLIPBOARD | ✅ 已实现 |
| 30 | CLIPBOARD_CHANGE | ⬜ 未实现强类型推送 |
| 31–34 | WiFi REQUEST_01/02 握手与信任 | ⬜ 未实现 |
| 35 | QUIT | ✅ 已实现 |
| 36 | CANCEL | 🟡 flag 2 内部取消已实现；公共取消语义未完成 |
| 37 | PHOTO_SYNC | ⬜ 未实现 |
| 38 | FILE_CHANGE | ⬜ 未实现 |
| 39 | SYNC_MONITOR | ⬜ 未实现 |
| 40–41 | UPDATE_FILE_INFO 请求/响应 | ⬜ 未实现 |

### 4.2 连接与信任

- ⬜ Bonjour/mDNS 浏览 `_handshaker_ssp._tcp.`。
- ⬜ SRV/TXT/A/AAAA 解析、设备去重、地址选择和发现超时。
- ⬜ WiFi TCP connector。
- ⬜ WiFi `HANDSHAKE_REQUEST_01/02`、信任弹窗状态和多次 RESPONSE_02。
- ⬜ derived key 的 PBKDF2 生成、保存、读取和失效处理。
- ⬜ `TRUST_ONCE`、`TRUST_ALWAYS`、`TRUST_REMOVE` 的完整生命周期。
- ⬜ 查看、删除、重置信任记录的 library API 和 CLI。
- ⬜ WiFi 主机地址变化、手机重启和错误 derived key 的恢复流程。
- ⬜ USB AOA 枚举、Accessory 切换、接口 claim、端点读写和断开监听。
- ⬜ macOS/Linux 的 USB 后端实现与权限说明。
- ⬜ 二维码或手工地址连接流程。
- ⬜ 原版音频相关 `tcp:19999` HTTP 辅助通道。

### 4.3 主动推送与订阅

- 🟡 Session 能组装未匹配 sid 的普通消息，但尚未解析并对外发布。
- ⬜ 稳定的 `ClientEvent` 领域枚举和异步订阅 API。
- ⬜ 设备信息变更回调。
- ⬜ 照片、视频、音频库变更回调。
- ⬜ 剪贴板变更回调。
- ⬜ 目录监控和文件变更回调。
- ⬜ 推送与历史请求 sid 碰撞的完整业务级测试。
- ⬜ 慢消费者背压、事件队列溢出和断线重订阅策略。

### 4.4 文件与传输扩展

- ⬜ 多文件上传、下载。
- ⬜ 递归目录上传、下载。
- ⬜ 批量任务并发控制、汇总进度和逐项错误报告。
- ⬜ 下载 range、断点续传和临时文件恢复。
- ⬜ 上传恢复或重试语义；协议是否支持需先验证，不允许伪造断点续传。
- ⬜ gzip 文件传输模式。
- ⬜ `UPDATE_FILE_INFO` 文件元数据更新。
- ⬜ 完整文件权限/类型能力映射。
- ⬜ `is_sync=true` 对同步台账的正式支持。
- ⬜ 目录监控的启动、确认、停止与事件流。

### 4.5 媒体库

- ⬜ 照片库查询、相册和分页/范围参数。
- ⬜ 视频库查询和视频相册。
- ⬜ 音频库查询、音频专辑和音频元数据。
- ⬜ 图片、视频和音频缩略图请求。
- ⬜ EXIF、方向、经纬度、收藏状态和媒体 ID 的领域模型。
- ⬜ 媒体库变更增量合并。
- ⬜ 大型媒体库的流式/分页输出和内存上限。

### 4.6 照片同步

- ⬜ `PHOTO_SYNC_REQUEST` 初始状态机。
- ⬜ 本地与手机同步台账。
- ⬜ checksum 计算与增量 diff。
- ⬜ `FILE_CHANGE` 上传、删除、移动事件处理。
- ⬜ `SYNC_MONITOR_REQUEST` 实时同步。
- ⬜ 冲突检测、覆盖策略、失败恢复和幂等重放。
- ⬜ 同步状态持久化、schema 迁移和损坏恢复。
- ⬜ 同步 dry-run、计划预览和安全确认。

### 4.7 产品化和跨平台

- ⬜ Linux adb/网络/USB 的正式 CI 与安装包。
- ⬜ Windows 支持评估与实现。
- ⬜ shell 历史、补全和更完整的交互体验。
- ⬜ 配置文件中的默认 serial、timeout、输出格式等用户偏好。
- ⬜ 除中文外的语言资源与语言选择。
- ⬜ 稳定 API 文档、示例工程和 GUI 集成指南。
- ⬜ release CI、跨平台产物、校验和与安装说明。

## 5. 当前部分实现与已知限制

1. **主动事件尚不可消费**：未匹配 sid 消息只进入内部队列并记录日志，不能支撑监控和同步。
2. **取消不是远端强中断**：flag 2 不会停止手机已经开始的下载流；下载 Ctrl-C 必须关闭连接。
3. **传输仅限单文件全量模式**：没有目录、多文件、range、resume 或任务恢复。
4. **`stat` 不是独立协议命令**：通过根目录信息或父目录 `list_dir` 查找，性能和边界依赖目录列表。
5. **信任状态只是数据结构预留**：`state.json` 的 trust map 尚未接入任何连接流程。
6. **媒体 callback 全部关闭**：设备信息请求中的 photo/audio/video callback 标志当前为 false。
7. **删除的 `sync` 选项未形成同步功能**：公开 `DeleteOptions.sync` 只是映射协议字段。
8. **JSON schema 仍是 v1 首版**：新增事件和批量任务前必须先设计兼容扩展，不能临时改变 envelope。
9. **没有后台 daemon**：所有连接和任务随当前 CLI/library 进程结束，这是当前设计约束而非缺陷。

## 6. 后续开发原则与依赖关系

后续功能必须按以下依赖关系推进：

```text
现有 ADB 基线稳定
  -> 公共取消与强类型事件总线
  -> WiFi 发现、连接、信任
  -> 目录/剪贴板/设备主动事件
  -> 媒体查询与媒体变更
  -> 批量和递归传输
  -> 照片同步状态机
  -> USB AOA 与跨平台发布
```

核心原则：

- 先补通用 Session 能力，再实现依赖事件的上层功能。
- transport、handshake、session、业务 API 分层，不为 WiFi/USB 复制业务逻辑。
- 文档已确认不等于代码已实现；每个里程碑都必须有自动化测试和受控真机验收。
- 不增加未经验证的备用端口、包名、握手或自动重试。
- 不用后台 daemon 掩盖会话生命周期问题。
- 所有新用户文案继续写入语言文件；所有自动化字段保持英文稳定。

## 7. 完整开发里程碑

### M0：固化当前 ADB v0.1 基线

目标：把现有功能从“可运行首版”提升为可持续扩展的稳定基线。

任务：

- 为每个公开 `HandShakerClient` 方法补本地假 SSP 服务集成测试。
- 补齐设备信息缺字段、手机端错误码、部分删除失败和异常 EOF 测试。
- 增加上传 ready/完成异常、下载 MD5 错误和临时文件清理测试。
- 完善 JSON/JSONL 快照，固定 schema v1 的兼容边界。
- 为 public API 添加 rustdoc 和最小示例。
- 记录一次可复现的 Rust CLI 真机验收报告。
- 建立 macOS ARM64 的基础 CI：fmt、test、clippy、release build。

验收：

- 当前全部 CLI 命令有成功和关键失败路径测试。
- 真机完成设备信息、ping、目录 CRUD、上传/下载 MD5、剪贴板和清理。
- 退出后无残留 adb forward。

### M1：公共取消模型与强类型事件总线

目标：为监控、WiFi 信任状态和同步提供统一异步基础。

任务：

- 定义不暴露 Prost 的 `ClientEvent`、`EventKind` 和订阅句柄。
- 将未匹配 sid 的完整消息按预期类型解码并发布。
- 对设备信息、剪贴板、媒体库、目录和同步事件建立明确路由。
- 定义慢消费者、队列容量、溢出错误和连接关闭行为。
- 引入请求取消句柄或 cancellation token，区分“停止等待”和“远端已取消”。
- 下载取消继续采用关闭连接语义，并在 API 中明确报告。
- 完善 sid 碰撞、历史 sid 复用、并发请求和事件插入测试。

验收：

- 消费者可以稳定订阅、过滤和停止事件流。
- 任何事件都不会被误写入下载文件。
- 队列溢出、取消、关闭和重连行为有确定结果。

### M2：WiFi 发现、连接与持久化信任

目标：实现与抓包结果一致的局域网完整连接路径。

任务：

- 实现 Bonjour/mDNS 浏览和 `_handshaker_ssp._tcp.` 记录解析。
- 将发现结果建模为不依赖具体 mDNS 库的领域类型。
- 实现 `ConnectionTarget::Wifi` 和 `WifiConnector`。
- 实现独立的 `WifiTrustHandshake`，不得复用 ADB 裸握手流程。
- 实现 REQUEST_01/02、多次 RESPONSE_02 和信任状态转换。
- 实现 PBKDF2 derived key、host UUID、信任记录保存和 `0600` 权限。
- 增加 `device discover`、`trust list`、`trust remove/reset` 等 CLI。
- 明确信任删除、设备重装、错误 key 和超时后的恢复策略。
- 添加本地假 WiFi 服务、抓包向量和真实局域网测试。

验收：

- 首次连接能完成手机端授权。
- `TRUST_ALWAYS` 重连无需再次确认。
- 错误或被删除的信任记录不会静默降级，能够明确重建信任。
- WiFi 下设备、文件、传输和剪贴板复用同一业务 API。

### M3：目录监控与设备/剪贴板主动推送

目标：完成首批依赖事件总线的实时功能。

任务：

- 实现 `MONITOR_FOLDER_REQUEST`、确认头和文件事件回调。
- 提供开始监控、停止监控、递归范围和事件订阅 API。
- 将 `SSPFileEventType` 映射为稳定领域枚举。
- 实现设备信息变更和剪贴板变更事件。
- CLI 增加 `fs watch`、`clipboard watch`，JSONL 输出稳定事件 envelope。
- 实现断线终止和可选的显式重新订阅；不做静默无限重试。

验收：

- 创建、写入、移动、删除目录测试文件时能收到顺序正确的事件。
- Ctrl-C 能停止监控、发送必要关闭请求并清理连接。
- JSONL 事件可被脚本持续消费且不会混入 human 日志。

### M4：媒体库与缩略图

目标：覆盖图片、视频、音频及其变更推送。

任务：

- 新增 `ImageFile`、`ImageAlbum`、`VideoFile`、`VideoAlbum`、`AudioFile`、`AudioAlbum` 等领域类型。
- 实现照片、视频和音频库查询 API。
- 实现缩略图请求、错误字段和二进制输出策略。
- 支持相册、媒体 ID、EXIF、方向、时间、经纬度和收藏状态。
- 接入 PHOTO/AUDIO/VIDEO_LIB_CHANGE 推送。
- 设计大型媒体库分页、流式输出和内存限制。
- CLI 增加 `media photo|video|audio` 查询和 watch 命令。

验收：

- 三类媒体查询与手机系统媒体库结果可抽样核对。
- 缩略图数据可解码，错误条目不会导致整批失败。
- 媒体变更能通过 JSONL 持续输出。

### M5：批量、递归传输与文件元数据

目标：在保持单文件原语可靠的前提下提供实用批量文件管理。

任务：

- 在 library 层建立批量任务模型，不把遍历和并发控制塞进 CLI。
- 实现递归目录扫描、目标映射、冲突预检和任务计划。
- 实现受控并发、总进度、单项结果和部分失败报告。
- 增加 dry-run 和明确的覆盖/删除确认。
- 调研并验证 range 下载，验证通过后实现断点续传。
- 若协议不支持上传恢复，明确保持全量重传，不制造伪恢复。
- 实现 `UPDATE_FILE_INFO` 和可确认的文件元数据字段。
- 增加 `fs pull/push --recursive` 或独立批量命令，并保持单文件语义兼容。

验收：

- 多层目录往返传输后路径、大小和 MD5 一致。
- 单文件失败不会掩盖其他任务结果。
- 中断后临时文件和任务状态可解释、可清理。

### M6：照片同步与实时同步

目标：实现协议定义的增量照片同步，而不是简单复制目录。

任务：

- 设计同步 profile、根目录、方向、冲突策略和持久化 schema。
- 实现手机端 checksum 算法和本地台账。
- 实现 `PHOTO_SYNC_REQUEST` 初始 diff。
- 实现 `FILE_CHANGE` 增量上传、删除和移动。
- 实现 `SYNC_MONITOR_REQUEST` 实时阶段。
- 定义幂等操作、重放、崩溃恢复和台账原子提交。
- 提供 dry-run、计划预览、空间检查和危险操作确认。
- CLI 增加 `sync plan`、`sync run`、`sync watch`、`sync status`。

验收：

- 首次同步、无变化重跑、单边新增、删除、移动和冲突都有确定结果。
- 中断后恢复不会重复删除或损坏文件。
- 台账损坏时停止并给出可恢复操作，不自动猜测。

### M7：USB AOA 连接

目标：新增 USB 传输通道，并复用现有裸握手、Session 和业务 API。

任务：

- 定义可测试的 USB backend 接口。
- macOS ARM64 优先实现设备枚举、AOA identification、Accessory 切换和端点读写。
- 处理热插拔、权限、claim/release、短读写和设备消失。
- 实现 `ConnectionTarget::Usb` 与 `UsbConnector`。
- 复用 `UsbRawKeyExchange`，但用抓包向量验证线路细节。
- 评估 Linux udev 规则和权限安装。
- 增加传输层通用测试，确保 TCP 与 USB 对 Session 上层语义一致。

验收：

- USB 下完成与 ADB 相同的设备、文件、传输和剪贴板验收。
- 拔线能立即结束任务并给出明确错误，不残留资源。

### M8：跨平台发布与 GUI-ready 稳定化

目标：形成可分发、可被 GUI 长期依赖的后端版本。

任务：

- 为 macOS ARM64 和 Linux 建立 CI、release 产物和安装说明。
- 评估 Windows 的 adb、mDNS、USB 和路径差异。
- 稳定 public API、错误模型、事件模型和序列化 schema。
- 增加 API 文档、示例程序和兼容性策略。
- 增加配置迁移、语言选择和 shell 补全。
- 建立性能基准：大目录、大媒体库、大文件传输和长时间事件连接。
- 完成安全审计：日志、信任密钥、路径处理、权限和临时文件。

验收：

- 下游示例 GUI 可以只通过公开 library 完成连接、浏览、传输和事件订阅。
- 发布包可在干净环境安装运行，不依赖系统 protoc。
- 文档明确各平台支持矩阵和仍未实现功能。

## 8. 横向测试计划

### 8.1 单元测试

- 帧边界、字节序、分块、长度溢出和非法 flag。
- AES/RSA/PBKDF2 抓包向量和错误密钥。
- proto2 optional/default 字段及 field 1 缺失。
- 路径规范化、冲突计划、checksum 和状态迁移。
- 所有领域模型与 JSON schema 序列化。

### 8.2 Session 状态机测试

- 正常请求、并发请求、超时、EOF、取消和重复关闭。
- 上传 ready/数据/完成，下载头/裸流/MD5。
- 心跳与长任务并存。
- 主动推送、sid 碰撞、慢消费者和队列溢出。
- WiFi 信任等待和多阶段握手。

### 8.3 集成测试

- 本地假 SSP 服务覆盖 ADB/WiFi/USB 抽象的共同语义。
- 假 adb 验证设备选择、动态端口、命令超时和精确清理。
- 假 mDNS 与 WiFi 服务验证发现、地址变化和信任重连。
- 临时文件系统验证递归传输、同步恢复和权限错误。

### 8.4 CLI 测试

- 中文 help 与语言 key 完整性。
- human、JSON、JSONL 快照。
- stdout/stderr 分离。
- TTY/非 TTY 的确认策略。
- Ctrl-C、Ctrl-D、shell 复用命令和稳定退出码。

### 8.5 真机验收

每个连接通道至少覆盖：

1. 连接指定设备；
2. 设备信息和 ping；
3. 根目录列表；
4. 唯一测试目录 CRUD；
5. 上传/下载及 MD5；
6. 剪贴板读写；
7. 该里程碑新增的媒体、监控或同步功能；
8. QUIT、资源清理和无残留 forward/USB claim。

## 9. 版本建议

- 当前 `0.1.x`：稳定 ADB 首版、事件基础和小型 API 增补。
- WiFi 完整可用后：由维护者决定是否进入 `0.2.0`。
- 媒体、监控、同步或 USB 等较大里程碑：根据 public API 兼容性由维护者决定 Y 版本。
- `1.0.0` 前提：至少 ADB + WiFi 稳定、公开 API 和 JSON schema 有兼容承诺、跨平台发布流程成熟。
- 单次 Bug 修复和简单功能默认只递增 Z；纯文档不递增版本。
- 手机兼容身份 `2.5.6 / 408` 永远不跟随 Cargo 版本自动变化。

## 10. 下一步建议

建议紧接着执行 **M0 -> M1 -> M2**：

1. 先固化现有 ADB API 的假 SSP 集成测试与真机验收记录；
2. 再把内部 unmatched event queue 升级为公共强类型事件总线，并完成取消模型；
3. 最后实现 WiFi Bonjour、REQUEST_01/02 和持久化信任。

这样可以避免在媒体、监控和同步阶段重复改 Session，也能确保 WiFi 与 ADB 只在 connector/handshake
层分叉，后续全部业务 API 保持复用。

## 11. 相关文档

- `README.md`：项目简介和当前运行入口。
- `AGENTS.md`：开发约束、协议不变量和交付要求。
- `docs/README.md`：协议文档索引。
- `docs/13-verification-status.md`：协议结论的验证等级。
- `docs/14-capture-validation.md`：真实抓包验证结果。
- `proto/smartsync.proto`：完整 proto2 schema。
