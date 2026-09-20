# M8 基线记录(baseline)

> 记录时间:2026-08-03
> 分支:`refactor/m8-workspace-application-ffi`
> 基线 commit:`fecc4bb6061477fcd07aa2443b6529c69e04e15d`(main 上 "Update plan.md")
> 版本:`handshaker_rust 0.6.1` / binary `handshaker 0.6.1`

## 1. 目的

M8 任何迁移阶段都能与本基线比较:

- 已知失败必须在此记录,不允许在后续误称为新回归;
- CLI 命令、JSON/JSONL 字段、退出码、binary 名称与版本输出以本记录为参照;
- 测试数量变化必须可解释。

## 2. 工程形态(迁移前)

- 单 Cargo package(`handshaker_rust`),同时提供 library + binary `handshaker`。
- `src/lib.rs` 直接导出 `HandShakerClient`、`ConnectionTarget`、`ClientOptions`、
  取消/事件、ADB/WiFi/USB 设备模型、文件/剪贴板/媒体/批量传输/同步模型、
  `State`/`SyncStore`/同步算法、`WifiDevice`/`TrustRecordInfo` 等。
- `src/client.rs` 承担设备枚举、传输选择、连接握手、设备信息、文件/剪贴板/媒体/
  批量传输、信任记录、Session 生命周期与部分解码。
- `src/cli.rs` 承担 Clap 命令树、参数本地化、路径处理、确认、连接目标、文件/
  批量/同步编排、shell/REPL、watch。
- `build.rs` + `proto/smartsync.proto`(proto2,prost-build,vendored protoc)。
- `locales/zh-CN.json` 唯一语言资源;测试 `tests/localization.rs` 拒绝 src 内嵌 CJK。

## 3. 版本与身份

- Cargo package version:`0.6.1`。
- 手机兼容身份:`host_app_version = 2.5.6`、`host_app_version_code = 408`
  (**不随 Cargo 版本变化**,AGENTS.md 约束)。
- ABI/FFI 版本:M8 引入,初始 `1.0.0`,与 Cargo 版本独立记录。

## 4. 测试基线(迁移前)

`cargo fmt -- --check`:FMT_OK。

| 目标 | 数量 | 结果 |
|---|---|---|
| lib(`handshaker_rust-*`) | 120 | pass |
| bin(`handshaker-*`) | 21 | pass |
| cli(`cli-*`) | 12 | pass |
| localization | 1 | pass |
| **合计** | **154** | **全部通过** |

执行方式:沙箱内 `cargo test` 被策略拦截,使用 `cargo build --tests` +
直接运行 `target/debug/deps/` 下各测试二进制(`--test-threads=1`)。

## 5. CLI 行为快照(迁移前)

- `handshaker --version` → `handshaker 0.6.1`。
- `handshaker --help` 顶层命令与全局参数:见 `tests/fixtures/m8/help.txt`
  (完整快照,迁移后逐项比对)。
- 输出格式:`human`(中文,文案来自 `locales/zh-CN.json`)/ `json`(单个最终对象,
  `schema_version: 1`)/ `jsonl`(进度与事件流);`--output` 全局参数。
- 退出码稳定:参数 2 / 配置与设备选择 3 / 连接握手 4 / 协议 5 / 手机端 6 /
  本地 I/O 7 / 缺少确认 8 / 用户中断 130。
- 破坏性操作确认规则:非 TTY 或 JSON 模式必须 `--yes`。
- 连接入口:`--serial <adb serial>` / `--wifi IP:PORT` / `--usb [location]`(互斥)。
- 子命令族:`device`、`fs`(含 push/pull `--` 分隔、`--recursive`、`--dry-run`)、
  `clipboard`、`media`、`trust`、`sync plan|run|watch|status`、`shell`、`batch`、
  `watch`、`ping`、`quit`。

## 6. 公开 Rust API 快照(迁移前,摘要)

`src/lib.rs` 公开导出(完整列表以 `cargo doc` 为准):

- `HandShakerClient`、`ConnectionTarget`、`ClientOptions`、`RequestOptions`、
  `CancellationToken`、`EventCallbacks`、`PingResult`;
- 领域模型:`RemoteFile`、`DeviceInfo`、`ClipboardEntry`、`ImageFile`/`ImageAlbum`/
  `VideoFile`/`VideoAlbum`/`AudioFile`/`AudioAlbum`、`PhotoLibrary`/`VideoLibrary`/
  `AudioLibrary`/`Thumbnails`、`ExifData`、`WifiDevice`、`TrustRecordInfo`;
- 批量/同步:`BatchTransferOptions`/`BatchTransferResult`/`BatchTransferItem`/
  `BatchTransferProgress`/`BatchTransferFailure`、`SyncConfig`/`SyncSnapshot`/
  `SyncDiff`/`SyncFileRecord`/`PhotoSyncResult`、`plan_diff`/`check_conflicts`/
  `execute_plan`/`apply_file_change` 等;
- 状态:`State`/`StateStore`/`TrustRecord`、`SyncStore`;
- 事件:`ClientEvent`/`EventKind`/`EventFilter`/`EventSubscription`/`FileEvent` 等。

M8 阶段一不删除上述 API(兼容导出);新增 GUI/FFI 入口以 Application 层为准。

## 7. 已知失败与限制(基线)

- 无已知测试失败。
- 运行环境限制:沙箱中 `cargo test`/`cargo clippy` 被策略拦截;CI 上仍应跑全量。
- USB accessory 会话单次性:手机端 QUIT 后不再监听,重连需物理拔插;
  `batch` 长连接在同一会话内规避(0.6.1)。
- USB identification 仅 macOS ARM64 验证;Linux udev 未实现。
