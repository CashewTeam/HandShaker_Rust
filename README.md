# HandShaker_Rust

HandShaker 是 Smartisan（锤子科技）已经停止维护的 Android 文件传输与设备管理工具。本仓库用于整理现有逆向资料，逐步编写通信协议文档，并为后续开发兼容原版 HandShaker 的跨平台 Rust 后端建立基础。

## 项目目标

1. 基于现有逆向文件，解析并完整记录 HandShaker 通信协议。
2. 优先完成 macOS 版 `HandShaker_CLI`，提供纯命令行使用入口，用于功能测试和 Agent 调用。
3. 在 CLI 后端基础上，实现现代化、可复用的通用跨平台 Rust 后端，使其能够与原版 HandShaker 互通，并为后续 GUI 开发提供基础。

## 跨平台开发顺序

1. 现代 ARM64 macOS
2. Linux
3. 其他平台

## 目录结构

- `docs/`：通信协议文档、研究记录和设计说明（含真实抓包验证报告）。
- `plan.md`：当前后端实现状态、尚未实现的协议功能和完整开发计划。
- `proto/smartsync.proto`：完整 SSP proto2 schema；构建时由 Prost 生成 Rust 类型，不提交生成文件。
- `locales/zh-CN.json`：CLI、错误和诊断信息使用的中文语言资源；Rust 源码只引用稳定消息 key。
- `crates/`：Cargo Workspace——`handshaker-core`(协议/传输/Session)、
  `handshaker-application`(UI 无关业务层,统一契约)、`handshaker-cli`
  (`handshaker` 可执行文件)、`handshaker-ffi`(稳定 C ABI)。
- `tools/capture/`：SSP 协议抓包验证工具（Python，经 adb forward 与真机对话并逐字节日志）。
- `Android_jadx/`、`android_smali/`、`macos/`：本地保留的逆向与反编译资料，不纳入 Git 记录，也不会由本仓库重新分发。

## 当前状态

- 已完成对 HandShaker 通信协议（SmartSync Protocol / SSP）的完整逆向解析，文档见 `docs/`
  （协议分层、Bonjour 发现、USB AOA/ADB/WiFi 传输、握手与信任、封帧格式、protobuf 模式、文件/媒体/同步、异常处理）。
- 已在真实设备（Smartisan OD103 / Android 7.1.1）上完成**抓包验证**（`docs/14-capture-validation.md`）：
  - ADB 通道：端口、上下行封帧、分块边界、`parseIoBuffer`（AES-256-CBC）、RSA 签名、下载/上传数据面。
  - **局域网通道**：Bonjour/mDNS 发现（`_handshaker_ssp._tcp` 全记录 + SRV 端口实测）、WiFi 握手与
    信任（TRUST_REMOVE / derived_key 重连免弹窗）、局域网传输（数据 MD5 一致）。
  - 验证工具见 `tools/capture/`。
- Rust CLI `0.7.0` 已固化 ADB v0.1 基线，增加 library 级强类型事件订阅和请求取消，并实现 **WiFi（LAN）连接**：
  `device discover` mDNS 发现、`--wifi IP:PORT` 直连、REQUEST_01/02 握手与持久化信任
  （`TRUST_ALWAYS` 重连免弹窗、`trust list/remove/reset`）。
- **目录监控与主动推送（M3，0.2.0）**：`monitor_folder(path, register)` library API 与
  `watch` 命令——注册目录监控（`--path` 可重复）后实时输出设备主动推送（目录文件事件、
  剪贴板变更、设备信息等），human/jsonl 两种输出；事件 JSON `kind` tag 为 0.2.0 兼容契约。
- **媒体库与缩略图（M4，0.3.0）**：`get_photo/video/audio_library` 全量查询（真机 3005 张照片
  验证）、`get_thumbnails` 批量缩略图（JPEG）、`media photo|video|audio`（默认预览上限 50 条，
  `--limit`/`--all` 覆盖）与 `media thumbnail --output-dir` 写文件。
- **EXIF、增量合并与批量传输（M5，0.4.0）**：`fetch_exif` 落地（SSP 下载 + kamadak-exif 本地
  解析，WiFi/ADB 通用）；`media_merge::apply_photo/video/audio` 把 watch 变更增量合并进快照；
  `upload_many/download_many/upload_tree/download_tree` 批量/递归传输（串行、部分失败聚合），
  CLI `fs push/pull` 支持多目标与 `--recursive`。
- **M5 收尾（0.4.1）**：`fs push/pull --dry-run` 预演报告（文件/目录/字节，不传输）；区间下载
  `TransferOptions.offset`（一次性定位，非自动续传）；`update_files_info`（UPDATE_FILE_INFO
  40/41，library API）；批内受控并发 `BatchTransferOptions.concurrency`（1..=8，默认串行）。
- **照片同步（M6，0.5.0）**：`PHOTO_SYNC`(37)/`SYNC_MONITOR`(39) 发送侧；完整增量状态机
  （plan diff → 执行下载/删除 → 台账原子提交 → 实时 FILE_CHANGE 增量落账）；单向 手机→主机；
  独立同步台账 `<config>/sync/<uuid>.json`（0600/0700、原子提交、损坏硬错误）；CLI
  `sync plan/run/watch/status`。
- **USB AOA 连接（M7，0.6.0）**：传输层抽象（握手/帧读写泛型化，Session 对 TCP/USB 透明）；`rusb` 集成——枚举 accessory（0x18d1）与 Smartisan 常驻 accessory（0x29a9）设备、AOA identification（对照 Mac 版 `sendAOAStartupRequest`：请求码十进制 51/52/53、字符串 0 基 index、UTF-8 编码）与 `--usb [--serial <bus-ports>]` 直连（复用 ADB 裸握手）；`device list` 同时列出 ADB 与 USB 设备；真机完整业务验收通过（连接/文件/传输 MD5/剪贴板/重命名，单连接批量）。
- **长连接批量会话（0.6.1）**：`handshaker batch` 从 stdin 逐行读取命令、在单个持久连接上顺序执行（心跳保活，规避手机端 accessory 会话单次性）；输出遵循 `--output`，命令级失败继续、致命错误中断、`exit`/`quit`/EOF 结束并 QUIT。
- **Workspace 拆分与应用服务模型（M8，0.7.0）**：仓库改为 Cargo Workspace（`handshaker-core`/`handshaker-application`/`handshaker-cli`/`handshaker-ffi`），CLI 行为与 JSON 契约不变；建立应用层 v1 契约（`HandShakerRuntime`、Session/Transfer/事件、`PublicError` 分区码，当前为 preview 状态，见 `docs/application-api-v1.md` 与 `docs/architecture.md`）；新增 `handshaker-ffi`（稳定 C ABI 1.0.0：Runtime/设备/连接/文件列表/事件订阅，panic 隔离 + Buffer 所有权，C 与 Swift smoke 测试通过，见 `docs/ffi-v1.md`）。
- **CLI 渐进迁移与 FFI 传输面（M8 后续，0.7.1）**：CLI 连接统一走 `HandShakerRuntime`（`session_client` 过渡 API 供未迁移命令复用同一连接，冻结前移除）；`fs ls/stat/exists/mkdir/mv` 与 `fs pull/push` 批量用例迁移到 Application（树枚举与路径逃逸防护保留在 core，CLI 只留参数/确认/展示；`rm`/`count` 因输出契约暂留 core）；`handshaker-ffi` 升至 ABI 1.1.0，导出 `hs_transfer_start_download/start_upload/cancel/get/list`，Swift smoke 覆盖传输面。
- **FFI 补齐与 CLI 常用命令迁移（0.7.2）**：`handshaker-ffi` 升至 ABI 1.2.0，
  新增 `hs_create_directory` 与 `hs_ping`；CLI `fs rm/count`、`clipboard` 全量、
  `media` 全量（photo/video/audio/thumbnail）迁移到 `HandShakerRuntime`
  （Application `DeleteResultDto` 改为携带 `FileEntryDto`，fs.rm JSON 从路径数组
  变为条目对象数组——兼容性变化见 `docs/m8-migration.md` §7.1）；Phase 7 脚本
  `generate-ffi-header.sh`/`build-ffi-linux.sh` 与 `dist/apple/` 产物；
  shell/batch/watch/sync 评估保留 core（边界见 §7.2，因此“全量迁移”不成立，
  当前状态是常用命令已迁移、交互/长连接/同步仍走 Core）。
- **CI clippy 清零（0.7.3）**：`clippy --all-targets --all-features -D warnings`
  全部告警修复（collapsible-if/match-result-ok/io-other-error/needless-borrow
  等，行为不变）；`BackendEvent::SessionStateChanged` 装箱为
  `Box<SessionSnapshot>`（serde JSON 输出不变，Application 事件 API 源码级调整）。
- **M8.1 Phase A 契约止血（进行中）**：FFI ABI 单一事实来源建立——Header 注释、
  `docs/ffi-v1.md` 与新增 `docs/ffi-abi-snapshot.md` 全部对齐 ABI 1.2.0；
  `scripts/check-ffi-abi.py` 校验符号/签名/ABI 版本注释/snapshot 一致性，
  CI 新增 ABI 检查与 C/Swift smoke；`APPLICATION_API_VERSION` 改为
  `1.0.0-preview.1`（v1 契约收口前允许破坏性修改，见 `docs/application-api-v1.md`）。
- **M8.1 Phase B Runtime 并发修复（进行中）**：`state_dir`/`wire_log` 配置
  真实生效（Core 公开 `StateStore::from_dir` 与 `connect_with_state`，FFI
  `wire_log_utf8` 落地）；Session Registry 不再有锁跨网络 await（短临界区
  clone client）；`disconnect` 重构为确定性关闭（Disconnecting → 取消传输
  并等待 → 显式 QUIT → Closed 事件，异常发 Warning）；`shutdown` 单次执行、
  join 全部任务、EventHub 显式关闭（订阅者收 `{"closed":true}`），删除固定
  sleep（详见 `docs/m8-migration.md` §9）。
- **M8.1 Phase C 传输事件与取消（进行中）**：进度事件携带 `total_bytes`
  并按 100ms/256KiB 节流发布（30MB 真机实测 108 个事件）；取消立即终态
  （`finished_at_ms` + 事件，不等待后台任务）；本地/手机端取消经
  `TransferCancelled`/`RemoteCancelled` 区分；下载取消或传输丢失同步
  Session `Failed` 事件；Transfer history 有容量（默认 64）/TTL 边界；
  修复 macOS 14 provenance 文件上 `fchmodat` EPERM 导致的状态文件权限
  失败（`docs/m8-migration.md` §10）；真机 ADB 验收通过（传输 MD5、进度、
  传输中取消、清理无残留）。
- 剪贴板/目录监控之外的推送发送侧仍属于后续里程碑。

## 命令行教程

### 1. 使用前准备

当前 CLI 已实现 ADB 与 WiFi 两条通道。使用前需要：

1. 安装 Rust stable 工具链；
2. 安装 Android SDK Platform-Tools，确保终端能直接运行 `adb`；
3. 在手机上安装兼容的 HandShaker APK；
4. 通过 USB 连接手机、开启 USB 调试，并在手机上允许当前电脑调试。

刚安装 Rust 后，如果当前终端还找不到 `cargo`，先加载 Cargo 环境：

```sh
. "$HOME/.cargo/env"
```

确认 Rust 和 ADB 均可用：

```sh
rustc --version
cargo --version
adb version
adb devices -l
```

`adb devices -l` 中目标设备的状态应为 `device`。`unauthorized` 表示还需要在手机上确认 USB 调试；
`offline` 表示设备当前不可用。

### 2. 构建或安装 CLI

项目使用 vendored `protoc`，开发机不需要额外安装 protobuf 编译器。

开发构建和测试：

```sh
cargo build
cargo test
```

构建优化版本：

```sh
cargo build --release
./target/release/handshaker --version
```

也可以安装到 Cargo 的可执行文件目录，之后直接使用 `handshaker`：

```sh
cargo install --path .
handshaker --version
```

如果不安装，本文所有 `handshaker ...` 示例都可以改写为：

```sh
cargo run -- ...
```

例如 `handshaker device list` 等价于 `cargo run -- device list`。

### 3. 全局命令格式

```text
handshaker [--serial SERIAL] [--output human|json|jsonl]
           [--timeout 30s] [--yes] [-v|-vv] [--wire-log PATH]
           <COMMAND>
```

| 参数 | 说明 |
|---|---|
| `--serial SERIAL` | 指定 ADB 设备序列号 |
| `--output human` | 默认中文输出，适合人在终端中阅读 |
| `--output json` | 只输出一个最终 JSON 对象，适合脚本调用 |
| `--output jsonl` | 逐行输出传输进度和最终结果，适合流式处理 |
| `--timeout 30s` | 设置连接和请求超时，支持 `ms`、`s`、`m` |
| `--yes` | 跳过危险操作的交互确认；自动化环境通常需要显式添加 |
| `-v` / `-vv` | 输出 info/debug 级别诊断日志到 stderr |
| `--wire-log PATH` | 记录完整 SSP 线路数据，可能包含文件和剪贴板正文 |
| `-h`, `--help` | 查看当前层级帮助 |
| `-V`, `--version` | 查看 CLI 版本 |

查看总帮助或某个子命令的精确参数：

```sh
handshaker --help
handshaker fs --help
handshaker fs pull --help
handshaker clipboard set --help
```

完整命令速查：

```text
handshaker device list
handshaker device info
handshaker device ping

handshaker fs ls [REMOTE_PATH] [--depth N]
handshaker fs stat REMOTE_PATH
handshaker fs count REMOTE_PATH [--depth N] [--exclude REGEX]...
handshaker fs exists REMOTE_PATH
handshaker fs mkdir REMOTE_PATH
handshaker fs mv SOURCE TARGET
handshaker fs rm REMOTE_PATH... [--recursive] [--trash]
handshaker fs pull REMOTE_FILE [LOCAL_FILE] [--overwrite]
handshaker fs push LOCAL_FILE REMOTE_FILE [--overwrite]

handshaker clipboard get
handshaker clipboard set [TEXT]
handshaker clipboard set --stdin
handshaker clipboard delete TIMESTAMP
handshaker clipboard clear

handshaker shell
```

`--serial`、`--output`、`--timeout`、`--yes`、`-v` 和 `--wire-log` 是全局参数，可与上面的业务命令
组合使用。

### 4. 选择设备

先列出设备：

```sh
handshaker device list
```

未指定 `--serial` 时：

- 恰好一台在线设备：自动选择；
- 没有在线设备：报错退出；
- 多台在线设备：报错并要求明确指定，不会猜测设备。

多设备环境中先从列表复制序列号：

```sh
handshaker --serial DEVICE_SERIAL device info
handshaker --serial DEVICE_SERIAL fs ls
```

`device list` 只执行 `adb devices -l`。其他命令会自动启动手机端服务、建立动态 adb forward、完成
握手，命令结束后发送 QUIT 并清理本次创建的 forward。

### 4a. WiFi 设备发现与直连

手机与电脑处于同一局域网并开启 HandShaker 服务时，可以通过 mDNS 发现设备：

```sh
handshaker device discover                # 默认浏览 6 秒
handshaker device discover --browse-timeout 15s
```

`device discover` 只做发现，不建立连接。发现结果中的端口是手机动态端口，每次都要以最新
mDNS 响应为准。用 `--wifi` 直接指定地址连接（与 `--serial` 互斥），后续命令与 ADB 通道完全一致：

```sh
handshaker --wifi 192.168.2.47:45656 device info
handshaker --wifi 192.168.2.47:45656 fs ls
handshaker --wifi '192.168.2.47:45656' shell
```

首次连接时手机会弹出信任对话框，请在手机上确认；确认后主机保存信任记录，重连无需再次确认：

```sh
handshaker trust list                     # 查看本地信任记录
handshaker --yes trust remove DEVICE_UUID # 删除本地信任记录
handshaker --yes --wifi IP:PORT trust reset DEVICE_UUID  # 清除手机端信任记录
```

信任记录按手机 device_uuid（android_id）保存，不包含明文密钥材料。

### 5. 设备命令

列出 ADB 设备：

```sh
handshaker device list
```

显示当前手机信息：

```sh
handshaker device info
```

输出包括序列号、名称、型号、品牌、系统版本、APK 版本、手机根目录、电量和锁屏状态。

检测 SSP 连接并查看往返延迟：

```sh
handshaker device ping
```

### 6. 远端路径规则

“远端路径”指手机文件系统中的路径。

- 一次性命令中的相对路径以手机 `root_path` 为基准；
- 绝对路径以 `/` 开头；
- `shell` 中的相对路径以当前远端目录为基准；
- 当前上传和下载只支持单文件，不支持递归目录传输。

例如，假设手机 `root_path` 是 `/storage/emulated/0`：

```sh
handshaker fs ls Download
handshaker fs ls /storage/emulated/0/Download
```

以上两个命令指向同一目录。

### 7. 浏览和查询文件

列出手机根目录：

```sh
handshaker fs ls
```

列出指定目录：

```sh
handshaker fs ls Download
handshaker fs ls /storage/emulated/0/DCIM
```

设置最大遍历深度，默认深度为 1：

```sh
handshaker fs ls DCIM --depth 2
```

查看单个路径的信息：

```sh
handshaker fs stat Download/example.jpg
```

检查路径是否存在：

```sh
handshaker fs exists Download/example.jpg
```

统计目录中的文件数量：

```sh
handshaker fs count DCIM --depth 3
```

使用正则表达式排除路径；`--exclude` 可以重复指定：

```sh
handshaker fs count DCIM --depth 5 \
  --exclude '\.thumbnails' \
  --exclude '\.tmp$'
```

### 8. 创建、移动和删除

创建目录：

```sh
handshaker fs mkdir Download/HandShakerTest
```

移动或重命名路径：

```sh
handshaker fs mv Download/old.txt Download/new.txt
```

删除文件会要求确认：

```sh
handshaker fs rm Download/new.txt
```

删除目录必须显式添加 `--recursive`。这里的参数表示允许删除目录，当前 CLI 不提供电脑端递归逐项
删除：

```sh
handshaker fs rm Download/HandShakerTest --recursive
```

一次删除多个路径：

```sh
handshaker fs rm Download/a.txt Download/b.txt
```

请求手机将路径移入回收站：

```sh
handshaker fs rm Download/a.txt --trash
```

在脚本、CI、重定向输入或 JSON 模式中无法进行交互确认，必须显式添加 `--yes`：

```sh
handshaker --yes --output json fs rm Download/a.txt
handshaker --yes fs rm Download/OldFolder --recursive
```

### 9. 下载单个文件

下载到当前目录，并自动使用远端文件名：

```sh
handshaker fs pull Download/example.jpg
```

指定本地目标路径：

```sh
handshaker fs pull Download/example.jpg ./example.jpg
```

目标已存在时默认拒绝。允许覆盖需要 `--overwrite`，TTY 中仍会询问确认：

```sh
handshaker fs pull Download/example.jpg ./example.jpg --overwrite
```

非交互环境还必须添加 `--yes`：

```sh
handshaker --yes --output json \
  fs pull Download/example.jpg ./example.jpg --overwrite
```

下载先写入同目录下的唯一临时文件，完整接收后会在手机返回 MD5 时进行校验，再移动到目标路径。
校验或传输失败不会覆盖已有目标。

### 10. 上传单个文件

上传本地文件到手机：

```sh
handshaker fs push ./example.jpg Download/example.jpg
```

远端目标已存在时默认拒绝。允许覆盖：

```sh
handshaker fs push ./example.jpg Download/example.jpg --overwrite
```

用于脚本时同时添加 `--yes`：

```sh
handshaker --yes --output json \
  fs push ./example.jpg Download/example.jpg --overwrite
```

当前只接受普通本地文件，不支持直接上传目录。

### 11. 剪贴板

读取手机剪贴板历史：

```sh
handshaker clipboard get
```

human 输出会显示每个条目的毫秒时间戳；删除单条记录时需要使用这个时间戳。

写入一段文本：

```sh
handshaker clipboard set '来自电脑的文本'
```

从标准输入读取文本，适合多行内容或管道：

```sh
printf '第一行\n第二行\n' | handshaker clipboard set --stdin
handshaker clipboard set --stdin < note.txt
```

删除指定时间戳的条目：

```sh
handshaker clipboard delete 1720000000000
```

清空手机剪贴板：

```sh
handshaker clipboard clear
```

删除和清空属于危险操作。非交互执行时需要 `--yes`：

```sh
handshaker --yes clipboard delete 1720000000000
handshaker --yes clipboard clear
```

### 12. 常驻 shell

如果需要连续执行多个操作，可以只建立一次连接：

```sh
handshaker shell
```

进入 shell 后，业务命令不再加最前面的 `handshaker`：

```text
device info
device ping
fs ls
clipboard get
```

切换目录使用 shell 内建命令：

| 内建命令 | 作用 |
|---|---|
| `pwd` | 显示当前远端目录 |
| `cd REMOTE_PATH` | 切换当前远端目录 |
| `lpwd` | 显示当前本地目录 |
| `lcd LOCAL_PATH` | 切换当前本地目录 |
| `help` | 显示 shell 帮助 |
| `exit` / `quit` | 发送 QUIT 并退出 |

一个完整示例：

```text
handshaker(DEVICE_SERIAL) /storage/emulated/0> pwd
handshaker(DEVICE_SERIAL) /storage/emulated/0> cd Download
handshaker(DEVICE_SERIAL) /storage/emulated/0/Download> fs ls
handshaker(DEVICE_SERIAL) /storage/emulated/0/Download> lpwd
handshaker(DEVICE_SERIAL) /storage/emulated/0/Download> lcd /tmp
handshaker(DEVICE_SERIAL) /storage/emulated/0/Download> exit
```

注意：

- shell 当前只支持 human 输出；
- 使用上、下方向键浏览当前 shell 会话中已经输入过的命令，左右方向键编辑当前命令；
- shell 中不能使用 `clipboard set --stdin`，因为标准输入正在用于读取命令；
- Ctrl-D 等同于正常退出；
- Ctrl-C 取消当前请求；下载过程中 Ctrl-C 会关闭连接，因为手机不会停止已经开始的下载流。

### 13. JSON 和 JSONL 自动化输出

获取一个最终 JSON 对象：

```sh
handshaker --output json device info
handshaker --output json fs ls Download
handshaker --output json fs exists Download/example.jpg
```

成功结果使用固定 envelope：

```json
{
  "schema_version": 1,
  "ok": true,
  "command": "fs.exists",
  "device": {
    "serial": "DEVICE_SERIAL",
    "name": "DEVICE_NAME"
  },
  "data": {
    "path": "/storage/emulated/0/Download/example.jpg",
    "exists": true
  },
  "warnings": []
}
```

错误也使用同一 envelope，并通过非零退出码报告：

```json
{
  "schema_version": 1,
  "ok": false,
  "command": "fs.ls",
  "device": null,
  "error": {
    "code": "device_selection",
    "message": "设备选择失败：没有在线的 ADB 设备",
    "details": null
  },
  "warnings": []
}
```

传输时使用 JSONL 获取逐行进度和最终结果：

```sh
handshaker --output jsonl fs pull Download/large.zip ./large.zip
handshaker --output jsonl fs push ./large.zip Download/large.zip
```

JSON 字段、命令名、事件名和错误 code 固定使用英文，不随显示语言变化。普通诊断日志写入 stderr，
不会混入 JSON stdout。

### 14. Library 事件订阅与取消

CLI v0.3.0 的事件总线和取消模型首先面向 Rust library 使用。订阅不会自动打开手机端 callback；需要主动事件时，
连接时显式启用对应的 `EventCallbacks`：

```rust,no_run
use handshaker_rust::{
    ClientOptions, ConnectionTarget, EventCallbacks, EventFilter, HandShakerClient,
};

# #[tokio::main]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
let client = HandShakerClient::connect_with_event_callbacks(
    ConnectionTarget::Adb { serial: None },
    ClientOptions::default(),
    EventCallbacks {
        device_info: true,
        ..EventCallbacks::default()
    },
).await?;
let mut events = client.subscribe_events(EventFilter::all());
let event = events.recv().await?;
println!("{}", serde_json::to_string(&event)?);
client.close().await?;
# Ok(())
# }
```

`ClientEvent` 使用稳定的英文 kind 名称，覆盖设备信息、剪贴板、媒体库、目录、文件变更、照片同步、同步监控和
远端取消。`EventFilter::only([...])` 可筛选事件。订阅容量固定为 64；慢消费者先收到 `Lagged { missed }`，
处理该错误后仍可继续读取后续事件。连接关闭后返回 `Closed`，不会自动重连。

所有公开请求都保留原方法，并提供 `*_with_options` 版本：

```rust,no_run
use handshaker_rust::{CancellationToken, RequestOptions};

# async fn example(client: &handshaker_rust::HandShakerClient) -> handshaker_rust::Result<()> {
let token = CancellationToken::new();
let options = RequestOptions::with_cancellation(token.clone());
let task = client.ping_with_options(options);
token.cancel();
let _ = task.await;
# Ok(())
# }
```

普通请求和上传取消会发送 flag `2`，并返回 `ErrorCode::Cancelled`；手机主动取消会标记为 remote，退出码为 `6`。
下载取消会删除临时文件、保留原目标文件并关闭当前 Session，之后需要重新连接。原有 `connect()` 默认关闭设备和媒体
callback；使用 `connect_with_event_callbacks()` 才会显式启用它们。

### 15. 超时、日志和线路记录

设置 500 毫秒、45 秒或 2 分钟超时：

```sh
handshaker --timeout 500ms device ping
handshaker --timeout 45s fs ls
handshaker --timeout 2m fs pull Download/large.zip
```

增加普通或详细诊断日志：

```sh
handshaker -v device info
handshaker -vv fs ls Download
```

显式记录 SSP 完整字节流：

```sh
handshaker --wire-log ./handshaker-wire.log device info
```

wire log 文件在 Unix 上使用 `0600` 权限，但内容仍可能包含文件数据和剪贴板正文。不要上传、提交或
公开分享未经清理的线路日志。

### 16. 退出码

| 退出码 | 含义 |
|---:|---|
| `0` | 成功 |
| `2` | 命令参数错误 |
| `3` | 配置、ADB 可用性或设备选择错误 |
| `4` | 连接、握手或超时错误 |
| `5` | SSP 协议错误 |
| `6` | 手机端文件或业务操作失败 |
| `7` | 本地文件或 I/O 错误 |
| `8` | 危险操作缺少明确确认 |
| `130` | 用户中断 |

脚本中应同时检查 JSON 的 `ok/error.code` 和进程退出码：

```sh
if handshaker --output json device ping > result.json; then
  echo '设备在线'
else
  status=$?
  echo "handshaker 失败，退出码：$status" >&2
fi
```

### 17. 常见问题

#### 找不到 `cargo`

重新打开终端，或加载 Rust 环境：

```sh
. "$HOME/.cargo/env"
```

#### 找不到 `adb`

安装 Android SDK Platform-Tools，并把其目录加入 `PATH`，然后确认 `adb version` 可以运行。

#### 没有在线设备

依次检查 USB 数据线、USB 调试开关、手机授权弹窗和：

```sh
adb devices -l
```

只有状态为 `device` 的设备会被选择。

#### 检测到多台设备

通过序列号明确选择：

```sh
handshaker --serial DEVICE_SERIAL device info
```

#### 危险操作在脚本中被拒绝

确认命令目标无误后添加 `--yes`。覆盖操作还必须同时带 `--overwrite`：

```sh
handshaker --yes fs rm Download/example.txt
handshaker --yes fs pull Download/example.txt ./example.txt --overwrite
```

#### 检查是否残留 adb forward

正常关闭、错误和超时路径都会清理本次创建的 forward。可以只读检查当前列表：

```sh
adb forward --list
```

不要自动删除无法确认归属的 forward。

### 18. 当前尚未支持

- 断点续传（library 仅提供一次性区间下载 `TransferOptions.offset`，CLI 未暴露）；
- Swift/GTK/.NET GUI 前端（后端与 FFI 处于 M8.1 收口中，Swift 集成原型可开始）；
- Linux/Windows 平台 FFI 交付（构建脚本已有，未进 CI 验证）。

完整后续计划见 `plan.md`。

> `GET_DEVICE_INFO` 中向手机报告的主机兼容身份固定为原版 macOS HandShaker
> `2.5.6 / 408`，用于通过手机端最低主机版本检查；它与本项目自身的 CLI/Cargo
> 版本 `0.7.x` 相互独立。
