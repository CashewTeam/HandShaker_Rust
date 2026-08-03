# HandShaker_Rust 后端现状与完整开发计划

> 状态基线：2026-08-03，Cargo workspace `handshaker_rust 0.7.1`（M8 拆分为 core/application/cli/ffi；0.7.1 完成 CLI fs 迁移与 FFI 传输面）。
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

## M0 基线固化状态

- ✅ 假 ADB/假 SSP 服务已覆盖公开客户端成功路径和关键失败路径。
- ✅ CLI 命令树、JSON/JSONL envelope、危险操作确认和 shell 裸 `ls` 有回归测试。
- ✅ public library API 已补充 rustdoc 和可编译最小示例。
- ✅ macOS ARM64 基础 CI 已加入。
- ✅ Smartisan U2 Pro/OD103 真机验收已完成，报告见 `docs/15-adb-v0.1-baseline.md`。
- 🟡 当前仍仅支持 ADB 单文件传输；WiFi、USB AOA、媒体、监控、同步继续属于后续里程碑。

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
- ✅ 未匹配 sid 的完整普通消息通过固定容量 64 的广播总线发布为强类型 `ClientEvent`，支持过滤、独立游标、
  `Lagged` 和 `Closed`。
- ✅ 设备、剪贴板、媒体、目录、文件变更、照片同步、同步监控和远端取消均有领域事件映射；无法安全判断时
  只发布安全元数据 `UnknownEvent`。
- ✅ 请求和上传支持 `CancellationToken`/`RequestOptions`；普通请求发送 flag 2 后保持连接，下载取消关闭当前
  Session 并清理临时文件，远端取消与本地取消可区分。
- ✅ `connect_with_event_callbacks` 显式控制初始设备、照片、音频和视频 callback，普通 `connect()` 保持关闭。

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
- ✅ 事件 field 1 存在/缺失解码、Unknown 安全元数据、过滤、多订阅者、Lagged 和 Closed 测试。
- ✅ 本地 flag 2、远端取消识别、普通请求连接复用和下载取消临时文件清理测试。
- ✅ 假 adb 的设备解析、超时、动态端口和精确清理测试。
- ✅ CLI JSON envelope、中文帮助和 `device list` 无副作用测试。
- ✅ Rust 源码 CJK 文案扫描，防止用户文本重新硬编码。

## 4. 已写入协议文档但尚未实现的功能

### 4.1 SSP 请求类型覆盖表

| SSPRequestType | 协议功能 | Rust 后端状态 |
|---:|---|---|
| 1 | HEART_BEAT | ✅ Session 心跳和 `device ping` |
| 2 | GET_DEVICE_INFO | ✅ 连接初始化和 `device info` |
| 3 | GET_THUMBNAIL | ✅ `get_thumbnails`（M4，批量 JPEG，错误条目不整批失败） |
| 4 | GET_PHOTO_LIB | ✅ `get_photo_library`（M4，全量映射） |
| 5 | GET_VIDEO_LIB | ✅ `get_video_library`（M4，全量映射） |
| 6 | GET_AUDIO_LIB | ✅ `get_audio_library`（M4，全量映射） |
| 7 | GET_DIR_FILES | ✅ 已实现 |
| 8 | GET_FILE_COUNT | ✅ 已实现 |
| 9 | GET_FILE_EXIST | ✅ 已实现 |
| 10 | GET_CREATE_FOLDER | ✅ 已实现 |
| 11 | GET_RENAME_FILE | ✅ 已实现 |
| 12–14 | DOWNLOAD 请求、响应头、数据面 | ✅ 单文件全量下载；✅ range 一次性定位下载（`TransferOptions.offset`，M5 0.4.1）；⬜ 断点续传/恢复未实现（协议无续传状态） |
| 15–18 | UPLOAD 请求头、ready、数据面、完成 | ✅ 单文件上传 |
| 19 | DELETE_FILE | ✅ 已实现 |
| 20 | PHOTO_LIB_CHANGE | ✅ library 强类型事件；CLI `watch` 已接入（M3/M4 真机验证） |
| 21 | AUDIO_LIB_CHANGE | ✅ library 强类型事件；CLI `watch` 已接入（M3/M4 真机验证） |
| 22 | VIDEO_LIB_CHANGE | ✅ library 强类型事件；CLI `watch` 已接入（M3/M4 真机验证） |
| 23–25 | MONITOR_FOLDER 请求、确认、回调 | ✅ `monitor_folder` + CLI `watch --path`（M3 真机验证） |
| 26 | GET_CLIPBOARD | ✅ 已实现 |
| 27 | POST_CLIPBOARD | ✅ 已实现 |
| 28 | CLEAR_CLIPBOARD | ✅ 已实现 |
| 29 | DELETE_CLIPBOARD | ✅ 已实现 |
| 30 | CLIPBOARD_CHANGE | ✅ library 强类型事件；CLI `watch` 已接入（M3 真机验证） |
| 31–34 | WiFi REQUEST_01/02 握手与信任 | ✅ `WifiTrustHandshake` + `--wifi`（M2，真机验证） |
| 35 | QUIT | ✅ 已实现 |
| 36 | CANCEL | ✅ 公共本地/远端取消模型和 flag 2 路由 |
| 37 | PHOTO_SYNC | ✅ `photo_sync` 发送侧 + 初始 diff（M6 0.5.0，真机验证） |
| 38 | FILE_CHANGE | ✅ 增量同步状态机（M6 0.5.0，真机验证） |
| 39 | SYNC_MONITOR | ✅ `sync_monitor` + 实时同步（M6 0.5.0，真机验证） |
| 40–41 | UPDATE_FILE_INFO 请求/响应 | ✅ `update_files_info`（M5 0.4.1，仅 library API + 测试） |

### 4.2 连接与信任

- ✅ Bonjour/mDNS 浏览 `_handshaker_ssp._tcp.`（`mdns-sd`）。
- ✅ SRV/TXT/A/AAAA 解析、设备去重、地址选择和发现超时（`device discover --browse-timeout`）。
- ✅ WiFi TCP connector（`WifiConnector`）。
- ✅ WiFi `HANDSHAKE_REQUEST_01/02`、信任弹窗等待和多次 RESPONSE_02。
- 🟡 derived key 由手机生成、主机保存与回传已实现；主机侧 PBKDF2 派生/校验路径未使用
  （未引入 `pbkdf2`/`sha1` 依赖，需要时按需添加）。
- ✅ `TRUST_ALWAYS`、`TRUST_REMOVE` 生命周期；`TRUST_ONCE`/`TRUST_UNKNOW`/`TRUST_NO` 仅协议枚举。
- ✅ 查看、删除、重置信任记录的 library API 和 CLI（`trust list/remove/reset`）。
- 🔬 WiFi 主机地址变化、手机重启和错误 derived key 的恢复流程已明确（显式重建），真机验证待补。
- ✅ USB AOA 枚举、identification、Accessory 切换、接口 claim、端点读写和断开监听（M7 0.6.0，macOS ARM64 真机验证）。
- 🟡 macOS ARM64 USB 后端已实现（M7）；Linux udev 权限评估待做。
- ⬜ 二维码或手工地址连接流程。
- ⬜ 原版音频相关 `tcp:19999` HTTP 辅助通道。

### 4.3 主动推送与订阅

- ✅ `ClientEvent`、`EventKind`、`EventFilter` 和 `EventSubscription` 公共领域 API。
- ✅ 64 容量广播、独立订阅游标、慢消费者 `Lagged`、主动关闭和连接关闭 `Closed` 行为。
- ✅ 设备信息、剪贴板、照片/视频/音频、目录、FILE_CHANGE、PHOTO_SYNC、SYNC_MONITOR 和远端取消解码。
- ✅ field 1 缺失时的唯一候选推断和歧义 `UnknownEvent` 安全元数据。
- ✅ pending sid 优先路由、历史 sid 复用和下载裸流冲突保护。
- ✅ **M3（0.2.0）**：`monitor_folder(path, register)` library API（MONITOR_FOLDER_REQUEST 注册/注销）；
  CLI `watch`（`--path` 可重复注册目录监控、全量事件流 human/jsonl 输出、Ctrl-C 注销后退出 130）。
- ✅ **M4（0.3.0）**：媒体库查询（`get_photo/video/audio_library`）与缩略图（`get_thumbnails`，
  JPEG、失败条目不整批失败）；CLI `media photo|video|audio`（**默认预览上限 50 条**，
  `--limit`/`--all` 覆盖，json 带 `total`/`truncated`）与 `media thumbnail --output-dir`；
  `fetch_exif` 预留接口（M5 实现，见 docs/20 §4）。
- ✅ 事件 JSON `kind` tag 与 watch jsonl 信封为 0.2.0 兼容契约（docs/19 §4）。
- ✅ **M6（0.5.0）**：照片同步 `sync plan/run/watch/status`（`photo_sync` 37 初始 diff + FILE_CHANGE 38 增量 + `sync_monitor` 39 实时，单向下载；真机验证，见 docs/22）。
- ⬜ 断线后的显式重新订阅策略由后续 watch/API 里程碑定义；当前不自动重连。

### 4.4 文件与传输扩展

- ✅ 多文件上传、下载（M5 0.4.1：`upload_many`/`download_many`，串行、失败聚合）。
- ✅ 递归目录上传、下载（M5 0.4.1：`upload_tree`/`download_tree`，镜像目录结构，路径逃逸防护）。
- ✅ 批量任务并发控制（默认 1 保序、上限 8，`futures-util` buffer_unordered）、汇总进度和逐项错误报告。
- ✅ 下载 range 一次性定位（`TransferOptions.offset`，`FileInputStream.skip` 语义）；⬜ 断点续传/临时文件恢复（协议无续传状态，需先验证，不允许伪造）。
- ⬜ 上传恢复或重试语义；协议是否支持需先验证，不允许伪造断点续传。
- ⬜ gzip 文件传输模式（M3 遗留；4 MiB 炸弹内存上限待定）。
- ✅ `UPDATE_FILE_INFO` 文件元数据更新（M5 0.4.1：type 40/41，仅 library API + 测试）。
- ⬜ 完整文件权限/类型能力映射。
- ✅ `is_sync=true` 对同步台账的正式支持（M6 单向下载方向）。
- ✅ 目录监控的启动、确认、停止与事件流（M3 0.2.0，真机验证）。

### 4.5 媒体库

- ✅ 照片库查询、相册和 `camera_album_id`（M4，真机 3005 张验证）。
- ✅ 视频库查询和视频相册。
- ✅ 音频库查询、音频专辑和音频元数据（artist/专辑/year）。
- ✅ 图片、视频和音频缩略图请求（`get_thumbnails`，JPEG、失败条目不整批失败）。
- ✅ CLI `media photo|video|audio`（默认预览上限 50、`--limit`/`--all`）与 `media thumbnail --output-dir`。
- ✅ EXIF 方向/经纬度/date_taken 随查询返回；独立 EXIF 拉取为 `fetch_exif` 预留接口（M5）。
- ✅ 媒体库变更增量合并（M5 0.4.1：`media_merge::apply_photo/video/audio`，key=media_id 优先、path 兜底）。
- 🟡 媒体库分页（协议请求无分页参数，当前 CLI 层预览截断 `--limit`/`--all`；服务端分页需协议确认）。
- ✅ EXIF 方向/经纬度/date_taken/收藏/媒体 ID 领域模型（M4）；独立 EXIF 拉取（M5 0.4.1，`kamadak-exif`）。
- 🟡 大型媒体库内存上限：session 线级 + 媒体解码 64 MiB 双上限；流式/分页输出待后续。

### 4.6 照片同步

- ✅ `PHOTO_SYNC_REQUEST`（37）初始状态机与初始 diff（M6 0.5.0，真机验证）。
- ✅ 独立同步台账（`sync_store`，原子提交、损坏时停止并给出恢复操作）。
- ✅ checksum 计算与增量 diff（`plan_diff`，`SyncSnapshot`）。
- ✅ `FILE_CHANGE`（38）增量事件处理（M6，单向下载方向）；⬜ 上传/移动方向未做（M6 范围决策）。
- ✅ `SYNC_MONITOR_REQUEST`（39）实时同步（M6，真机验证）。
- ✅ 冲突检测（`check_conflicts`）、失败聚合与幂等重跑；⬜ 跨设备冲突合并未做（M6 范围决策）。
- ✅ 同步状态持久化、schema 版本与损坏恢复（不静默重建）。
- ✅ 同步 dry-run、计划预览（`sync plan`）与安全确认（`sync run --yes`）。

### 4.7 产品化和跨平台

- ⬜ Linux adb/网络/USB 的正式 CI 与安装包。
- ⬜ Windows 支持评估与实现。
- ⬜ shell 历史、补全和更完整的交互体验。
- ⬜ 配置文件中的默认 serial、timeout、输出格式等用户偏好。
- ⬜ 除中文外的语言资源与语言选择。
- ⬜ 稳定 API 文档、示例工程和 GUI 集成指南。
- ⬜ release CI、跨平台产物、校验和与安装说明。

## 5. 当前部分实现与已知限制

1. **事件订阅已接入 CLI**：目录/媒体/剪贴板/同步 watch 均已提供；普通 `connect()` 仍默认不开启手机 callback，需 `connect_with_event_callbacks()`/`connect_with_all_callbacks()`。
2. **取消不是远端强中断**：flag 2 不会停止手机已经开始的下载流；下载取消必须关闭当前连接。
3. **无断点续传/上传恢复**：协议无续传状态，range 仅一次性定位（`TransferOptions.offset`）；批量传输默认串行（并发上限 8）。
4. **`stat` 不是独立协议命令**：通过根目录信息或父目录 `list_dir` 查找，性能和边界依赖目录列表。
5. **USB accessory 会话单次性**：手机端 QUIT 后不再监听（Android 生命周期），重连需物理拔插；`batch` 长连接在同一会话内规避（0.6.1）。
6. **USB identification 仅 macOS ARM64 验证**：Linux udev 权限与热插拔未实现；Windows 未评估。
7. **删除的 `sync` 选项**：公开 `DeleteOptions.sync` 映射协议字段；M6 同步删除走独立台账。
8. **JSON schema 仍是 v1 首版**：新增事件和批量任务前必须先设计兼容扩展，不能临时改变 envelope。
9. **没有后台 daemon**：所有连接和任务随当前 CLI/library 进程结束（除 `batch` 单进程多命令），这是当前设计约束而非缺陷。
10. **EXIF/gzip 等大响应内存上限**：session 线级 + 媒体解码 64 MiB 双上限；gzip 传输模式未实现（M3 遗留）。

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
  -> USB AOA
  -> 内部分层整理、应用服务模型冻结与 handshaker-ffi（Swift UniFFI）
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

> 状态：✅ 已完成（2026-08-02）；真机报告见 `docs/15-adb-v0.1-baseline.md`。

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

> 状态：✅ 已完成（2026-08-02）；API 和行为文档见 `docs/16-m1-events-cancellation.md`。

目标：为监控、WiFi 信任状态和同步提供统一异步基础。

已完成：

- 定义不暴露 Prost 的 `ClientEvent`、`EventKind`、`EventFilter` 和订阅句柄。
- 将未匹配 sid 的完整消息按预期类型解码并发布，无法安全判定时只发布 `UnknownEvent` 元数据。
- 对设备信息、剪贴板、媒体库、目录和同步事件建立明确路由，并保留 field 1 缺失的安全推断。
- 定义 64 容量广播、独立游标、慢消费者 `Lagged` 和连接关闭 `Closed` 行为。
- 引入 `CancellationToken`/`RequestOptions`，区分本地取消、手机取消、超时和协议错误。
- 下载取消继续采用关闭连接语义，删除临时文件并保留原目标。
- 增加事件过滤、多订阅者、队列溢出、事件解码、取消 flag 2 和 sid 路由测试。

验收：

- 消费者可以稳定订阅、过滤和停止事件流；慢消费者不影响其他订阅者或普通请求。
- 任何事件都不会被误写入下载文件；Unknown 事件不暴露原始 payload。
- 队列溢出、取消和关闭行为有确定结果；不自动重连，重连策略留给后续功能。
- 真机事件验证若无法在不扩大操作范围的情况下完成，必须在验收报告中明确记录。

### M2：WiFi 发现、连接与持久化信任（已完成，2026-08，0.1.5）

目标：实现与抓包结果一致的局域网完整连接路径。实现记录见 `docs/18-m2-wifi-trust.md`。

完成情况：

- ✅ Bonjour/mDNS 浏览 `_handshaker_ssp._tcp.`（`mdns-sd 0.20`，`src/discovery.rs`）。
- ✅ 发现结果建模为 `WifiDevice` 公开领域类型（不依赖 mDNS 库类型）。
- ✅ `ConnectionTarget::Wifi`、`WifiConnector`（TCP 直连 + 超时）。
- ✅ 独立 `WifiTrustHandshake`（REQUEST_01/02 多轮、TRUST_REMOVE、120s 信任等待）。
- ✅ derived_key 持久化（按 device_uuid，base64 存储，state.json `0600`），重连自动携带；
  host UUID 沿用既有 StateStore。
- ✅ `device discover`、`trust list`、`trust remove`、`trust reset` CLI；`--wifi IP:PORT` 与 `--serial` 互斥。
- ✅ 恢复策略：错误 derived_key → `failed` 明确报错；信任删除通过 `trust reset/remove` 显式重建；
  信任等待超时明确报错，不静默降级。
- ✅ 本地假 WiFi 服务（`FakeWifiSsp`）、抓包向量（docs/14 §10.2）与真实局域网发现验证。
- ✅ 真机完整验收（2026-08，OD103）：首次连接授权、重连免弹窗、WiFi 业务（文件/传输 MD5 一致/剪贴板）、
  trust reset、failed 自动清理与信任重建全部通过；记录见 `docs/18 §4.3`。

### M3：目录监控与设备/剪贴板主动推送（已完成，2026-08，0.2.0）

目标：完成首批依赖事件总线的实时功能。实现记录见 `docs/19-m3-directory-watch.md`。

完成情况：

- ✅ `MONITOR_FOLDER_REQUEST`（type 23）/ `SSPMonitorFolderResponseHeader`（type 24）：
  `client.monitor_folder(path, register)` 注册/注销 API，手机拒绝时按 `error_message` 映射 `RemoteIo`。
- ✅ 设备/剪贴板/媒体/目录主动推送经既有事件总线广播（`EventFilter` 订阅、`Lagged` 明确报错）。
- ✅ CLI `watch` 命令：`--path` 可重复注册、human/jsonl 输出稳定事件 envelope（`schema_version=1`）、
  Ctrl-C 注销后返回中断、断开（`Closed`）明确报错；`connect_with_all_callbacks` 全 callback 连接
  （修复媒体变更推送收不到的问题）。
- ✅ 安全加固：session 线级 64 MiB 响应上限（声明 total 早拒）、watch/媒体 human 输出
  `sanitize_human`（C0/DEL/C1 剥离，防终端转义注入）。
- ✅ 真机完整验收（2026-08，OD103）：目录事件（create→close_write→moved_from）、剪贴板事件、
  媒体库变更推送（MediaScanner 广播后 `media_library_changed`）全部收到；记录见 `docs/19 §6`。

### M4：媒体库与缩略图（已完成，2026-08，0.3.0）

目标：覆盖图片、视频、音频及其变更推送。实现记录见 `docs/20-m4-media-library.md`。

完成情况：

- ✅ 公开领域类型：`ImageFile`/`ImageAlbum`/`VideoFile`/`VideoAlbum`/`AudioFile`/`AudioAlbum`
  与容器 `PhotoLibrary`/`VideoLibrary`/`AudioLibrary`/`Thumbnails`（EXIF 方向/经纬度/date_taken/收藏）。
- ✅ 三类查询 API：`get_photo/video/audio_library`（type 4/5/6，全量映射，audio duration ms→s）。
- ✅ 缩略图：`get_thumbnails`（批量 JPEG，`get_thumbnail_error` 条目单独标记不整批失败）。
- ✅ `fetch_exif` 预留接口（公开签名，返回 `Error::Protocol` 未实现错误；M5 走 ADB shell 落地）。
- ✅ CLI `media photo|video|audio`：默认预览上限 50 条（`--limit`/`--all` 覆盖，JSON 恒输出
  `total`/`truncated`，albums 同限截断）；`media thumbnail <id|path>... --output-dir <dir>` 写文件
  （按回显 media_id/path 匹配、数字 id 溢出报 Usage、失败条目不中断、文件名仅本地生成防穿越）。
- ✅ 安全：session 64 MiB 响应上限 + 媒体解码二次上限（`decode_media_response`）。
- ✅ 真机完整验收（2026-08，OD103）：照片 3005 张 + 25 相册（含 Polarr 经纬度 30.279339/120.16548）、
  预览截断 `"total":3005,"truncated":true`、视频/音频查询、缩略图 JPEG 可解码（magic `ffd8ff`）、
  watch 实时收到媒体变更；记录见 `docs/20 §6`。

任务（原始规划，供对照）：

- ✅ 媒体库变更增量合并（M5 0.4.1 实现，见上；`media_merge::apply_photo/video/audio`）。
- ⬜ 媒体库分页（协议请求无分页参数，当前 CLI 层预览截断；需协议确认后实现服务端分页）。
- ⬜ 大型媒体库的流式输出（当前 session 64 MiB 响应上限保护）。

### M5：批量、递归传输与文件元数据（已完成，2026-08，0.4.0）

目标：在保持单文件原语可靠的前提下提供实用批量文件管理。实现记录见 `docs/21-m5-exif-batch.md`。

完成情况：

- ✅ EXIF 拉取落地：`fetch_exif(path)`（SSP 下载通道 32 MiB 上限 + `kamadak-exif` 本地解析，
  WiFi/ADB 通用；ExifData 扩展厂商/型号/镜头/焦距/曝光/光圈/ISO）。
- ✅ 媒体库变更增量合并：`media_merge::apply_photo/video/audio`（key = media_id 优先、path 兜底；
  added/updated upsert 保留快照独有字段；deleted 按 key 移除；kind 不匹配报协议错）；
  顺带修复事件通道 audio duration 毫秒→秒。
- ✅ 批量/递归传输：`upload_many/download_many`（串行、失败聚合）、`upload_tree/download_tree`
  （镜像目录结构）；CLI `fs push/pull` 多目标 + `--recursive`（`--` 分隔目标），批量覆盖预检+
  确认、批量进度与结果聚合输出。
- ✅ 自动测试 120 个全部通过；真机验收见 `docs/21 §6.3`。

任务（原始规划，供对照）：

- ✅ dry-run（`fs push/pull --dry-run`，计划预览；0.4.1 真机验证）。
- ✅ range 下载一次性定位（`TransferOptions.offset`；协议无续传状态，不做断点续传）。
- ✅ `UPDATE_FILE_INFO`（40–41）library API + 测试（0.4.1）。
- ✅ 受控并发（默认 1 保序，上限 8，`futures-util` buffer_unordered）。

### M6：照片同步与实时同步（已完成，2026-08，0.5.0）

目标：实现协议定义的增量照片同步，而不是简单复制目录。实现记录见 `docs/22-m6-photo-sync.md`。

完成情况（含用户范围决策：单向下载、独立台账、不做上传/跨设备冲突合并）：

- ✅ `PHOTO_SYNC_REQUEST`（37）发送侧与初始 diff（`photo_sync`，pc_id = host_uuid 原文，三端交叉确认）。
- ✅ 独立同步台账（`sync_store`：原子提交 0600、损坏停止不静默重建、device_uuid 路径净化）。
- ✅ checksum 与增量 diff（`plan_diff`/`SyncSnapshot`/`SyncDiff`）。
- ✅ `FILE_CHANGE`（38）增量事件处理（新增/删除/信息变更；下载方向）。
- ✅ `SYNC_MONITOR_REQUEST`（39）实时同步（`sync_monitor`；idle 拒绝不是协议错）。
- ✅ 冲突检测（`check_conflicts`）、失败聚合、幂等重跑（重复 37 被拒的修复：每次连接仅一次 37）。
- ✅ CLI `sync plan/run/watch/status`（dry-run 计划预览、`--yes` 确认、watch 补确认）。
- ✅ 真机验收（2026-08，OD103）：41 added、幂等重跑空 diff、单边删除、清理无残留；原始 1.2.0 smali
  交叉验证 37=PHOTO_SYNC 与 bucket 排除根因。

原始任务对照：

- ✅ 设计同步 profile、根目录、方向、冲突策略和持久化 schema。
- ✅ 实现手机端 checksum 算法和本地台账。
- ✅ 实现 `PHOTO_SYNC_REQUEST` 初始 diff。
- ✅ 实现 `FILE_CHANGE` 增量上传、删除和移动（上传/移动方向按范围决策未做）。
- ✅ 实现 `SYNC_MONITOR_REQUEST` 实时阶段。
- ✅ 定义幂等操作、重放、崩溃恢复和台账原子提交。
- ✅ 提供 dry-run、计划预览、空间检查和危险操作确认。
- ✅ CLI 增加 `sync plan`、`sync run`、`sync watch`、`sync status`。

验收：

- ✅ 首次同步、无变化重跑、单边新增、删除、移动和冲突都有确定结果（真机验证首跑/重跑/删除）。
- ✅ 中断后恢复不会重复删除或损坏文件（台账原子提交、幂等）。
- ✅ 台账损坏时停止并给出可恢复操作，不自动猜测。

### M7：USB AOA 连接（已完成，2026-08，0.6.0/0.6.1）

目标：新增 USB 传输通道，并复用现有裸握手、Session 和业务 API。实现记录见 `docs/23-m7-usb-aoa.md`。

完成情况：

- ✅ rusb（libusb）传输后端（`src/transport/usb.rs`：枚举/模式判定/identification/claim/bulk 读写）。
- ✅ AOA identification 对照 mac 版反汇编：请求码 0x33/0x34/0x35（GET_PROTOCOL/SEND_STRING/START）、
  UTF-8、index 0..=4、0x29A9 常驻 accessory 接口恒走 identification + 2s 等待 App openAccessory。
- ✅ `ConnectionTarget::Usb` + `UsbConnector`；复用 `UsbRawKeyExchange` 裸握手（Session 上层语义与 TCP 一致）。
- ✅ 热插拔/claim/release/短读写/设备消失；reader 线程独占 release+reset（恰好一次）。
- ✅ CLI `--usb [--serial locationId]`；0.6.1 新增 `batch` 长连接批量会话（stdin 单连接、心跳保活、
  失败聚合/致命中止、嵌套拒绝）。
- ✅ 真机完整业务验收（2026-08，OD103）：连接/文件/传输 MD5 一致/剪贴板/重命名/清理全 PASS；
  identification 三修复（请求码/编码/索引）实测验证。
- ✅ macOS ARM64 后端与权限说明；Linux udev 待评估。

验收：

- ✅ USB 下完成与 ADB 相同的设备、文件、传输和剪贴板验收。
- ✅ 拔线能立即结束任务并给出明确错误，不残留资源。

### M8：内部分层整理、应用服务模型冻结与 handshaker-ffi（Swift UniFFI 接入）（已完成，2026-08，0.7.0/0.7.1）

目标：把 0.6.1 之后的后端整理成可被 GUI 长期依赖的分层结构，并建立 FFI 边界。
实现记录见 `docs/architecture.md`、`docs/application-api-v1.md`、`docs/ffi-v1.md`、
`docs/m8-migration.md`、`docs/m8-test-report.md`。

任务：

- 整理 Rust 内部分层：审计并固化 transport/protocol/session/client/domain/cli 边界，
  收敛 `pub` 面（domain/公开 API 不泄露 Prost 与传输类型），消除跨层直接访问。
- 冻结应用服务模型：定义稳定的应用服务接口（连接生命周期、设备/文件/媒体/同步/事件订阅、
  错误与取消语义），作为 CLI 与 FFI 的共同基座（`AppService` 或等价层）。
- 建立 `handshaker-ffi` crate：提供 C ABI/UniFFI 绑定（UDL + 生成器），导出冻结后的领域类型；
  明确 async 桥接（运行时线程、回调、取消传播）与错误映射（FFI 错误码 ↔ `Error`）。
- 实现 Swift UniFFI 接入：生成 Swift 绑定，提供最小示例工程，验证连接、浏览、传输与事件订阅
  四条 GUI 消费路径。
- 版本与文档：版本 0.7.0；新增 FFI/应用服务模型文档与兼容性策略；回归 + security_review + 真机冒烟。
- **后续（0.7.1）**：CLI 连接统一走 runtime；`fs ls/stat/exists/mkdir/mv` + `fs pull/push` 批量用例迁移到 Application（`rm`/`count` 因输出契约暂留 core）；`handshaker-ffi` ABI 1.1.0 导出传输任务面（`hs_transfer_*`）；192 测试。

验收：

- CLI 与 FFI 共用同一应用服务模型，行为一致（同一批自动化测试覆盖两侧）。
- Swift 示例通过 `handshaker-ffi` 完成连接、浏览、传输与事件订阅。
- public API/错误模型/事件模型/JSON schema 冻结并有文档；FFI 错误码稳定。
- 既有 154 测试不回归；security_review 通过。

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

- 当前 `0.7.1`（M8）：ADB/WiFi/USB 基线能力全保留；Workspace 拆分 core/application/cli/ffi；
  应用服务模型冻结（Runtime/Session/Transfer/事件/PublicError v1）；FFI C ABI 1.0.0 最小闭环
  （设备/连接/文件/事件订阅，C 与 Swift smoke 通过）。
- 媒体、同步或 USB 等较大里程碑：根据 public API 兼容性由维护者决定 Y 版本。
- `1.0.0` 前提：至少 ADB + WiFi 稳定、公开 API 和 JSON schema 有兼容承诺、跨平台发布流程成熟。
- 单次 Bug 修复和简单功能默认只递增 Z；纯文档不递增版本。
- 手机兼容身份 `2.5.6 / 408` 永远不跟随 Cargo 版本自动变化。

## 10. 下一步建议

M0–M7 均已完成（M5 = EXIF 拉取 + 媒体库增量合并 + 批量/递归传输，0.4.0；M5 收尾 = dry-run/区间下载/UPDATE_FILE_INFO/受控并发，0.4.1；M6 = 照片同步与实时同步，0.5.0；M7 = USB AOA 连接，0.6.0，含传输层抽象与 AOA identification，真机完整业务验收通过；0.6.1 = `batch` 长连接批量会话，规避 accessory 单次会话）。建议优先处理遗留项：

1. M6 遗留：`sync run` 串行下载可接入 `batch_transfer` 并发；37/38/39 发送侧真机验收（§6.2 待执行）；
2. 安全遗留：剪贴板 gzip 解压输出上限（M3 记录）、`fs ls/device.info/clipboard.get` human 输出
   控制字符净化（既有 LOW，与 watch/media 一致化）。

依赖已就绪的强类型事件总线，同步阶段不需要再改 Session。

## 11. 相关文档

- `README.md`：项目简介和当前运行入口。
- `AGENTS.md`：开发约束、协议不变量和交付要求。
- `docs/README.md`：协议文档索引。
- `docs/13-verification-status.md`：协议结论的验证等级。
- `docs/14-capture-validation.md`：真实抓包验证结果。
- `proto/smartsync.proto`：完整 proto2 schema。
- `docs/16-m1-events-cancellation.md`：M1 事件订阅与取消行为。
- `docs/17-m1-device-validation.md`：M1 真机事件与清理验收。
- `docs/18-m2-wifi-trust.md`：M2 WiFi 发现、连接与持久化信任。
