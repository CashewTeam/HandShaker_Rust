# M5:EXIF 拉取、媒体库增量合并与批量/递归传输(0.4.0)

本文记录 M5(0.4.0)的设计、实现依据与验收。自动测试与真机验证状态见 §6。

## 1. 范围

- `fetch_exif(path)` 落地:按路径拉取远程图片的完整 EXIF 元数据。
- 媒体库变更增量合并:把 `watch` 收到的 `MediaLibraryChange` 增量应用到查询快照。
- 多文件/递归传输:CLI `fs push/pull` 支持多目标与 `--recursive`,library 提供
  `upload_many`/`download_many`/`upload_tree`/`download_tree`。

明确不做:range/断点续传下载、`UPDATE_FILE_INFO`(40–41)、dry-run、并发 >1。

## 2. 实现依据与验证等级

| 事实 | 依据 | 等级 |
|---|---|---|
| EXIF 数据从媒体库查询只回部分字段(orientation/date_taken/经纬度),完整 EXIF 需按文件拉取 | docs/20 §4、proto 字段映射 | 反编译推断 |
| 本实现经 SSP 下载通道拉文件到内存(32 MiB 上限)后本地解析,不新增协议字段 | M5 设计决策(用户批准) | 设计决策 |
| EXIF 解析用 `kamadak-exif 0.6`(纯 Rust);时间按 UTC 解释(EXIF 无时区) | 库文档 | 推断 |
| 增量合并 key = `media_id` 优先、`path` 兜底(与缩略图匹配惯例一致) | client.rs 既有惯例 | 代码事实 |
| 事件通道 audio duration 为毫秒(proto 注释),查询通道为秒 | `proto/smartsync.proto`、client.rs `audio_file` | 反编译+代码 |
| 批量传输串行执行,失败聚合不中断 | M5 设计决策(用户批准) | 设计决策 |

## 3. library API

### 3.1 EXIF 拉取

```rust
pub async fn fetch_exif(&self, path: &str) -> Result<ExifData>
```

- 内部 `download_bytes(remote, 32 MiB)`:SSP 下载通道(`need_md5=false`)把文件收到
  内存缓冲(`VecSink`),声明长度超过上限时在缓冲前拒绝(`Error::Protocol`,
  `exif.file_too_large`)。
- `exif_parser` 模块(私有)用 `kamadak-exif` 解析,映射到公开 `ExifData`:
  orientation / date_taken(UTC Unix 秒)/ latitude/longitude(小数度字符串,与媒体库
  查询同形)/ make / model / software / lens_model / focal_length / exposure_time /
  f_number / iso。
- 非 JPEG/EXIF 文件返回 `Error::Protocol`(`exif.parse_failed`),退出码 5。
- WiFi 与 ADB 连接通用(都归一为 SSP TCP)。

### 3.2 媒体库增量合并

```rust
pub mod media_merge;
pub fn apply_photo(library: &mut PhotoLibrary, change: &MediaLibraryChange) -> Result<()>;
pub fn apply_video(library: &mut VideoLibrary, change: &MediaLibraryChange) -> Result<()>;
pub fn apply_audio(library: &mut AudioLibrary, change: &MediaLibraryChange) -> Result<()>;
```

- 纯数据变换,不触碰 RPC/事件流;调用方自行维护快照(事件与查询保持解耦)。
- 按 `change.kind` 校验:photo 变更应用到 `PhotoLibrary` 等,不匹配返回
  `Error::Protocol`(`media.change_kind_mismatch`)。
- `added`/`updated` 按 key(media_id 优先、path 兜底)upsert:只更新事件携带的重叠
  字段,快照独有字段(thumbnail、starred、GPS、mini_thumb_magic 等)保留。
- `deleted` 按同一 key 移除;无 key 的条目忽略。
- audio duration 单位:事件通道已修复为毫秒→秒(与查询通道一致)。

### 3.3 批量/递归传输

```rust
pub async fn upload_many(&self, items: &[BatchTransferItem], options: BatchTransferOptions) -> Result<BatchTransferResult>;
pub async fn download_many(&self, items: &[BatchTransferItem], options: BatchTransferOptions) -> Result<BatchTransferResult>;
pub async fn upload_tree(&self, local_dir: &Path, remote_dir: &str, options: BatchTransferOptions) -> Result<BatchTransferResult>;
pub async fn download_tree(&self, remote_dir: &str, local_dir: &Path, options: BatchTransferOptions) -> Result<BatchTransferResult>;
```

- `BatchTransferItem { source, target }`(source = 本地 push / 远端 pull)。
- `BatchTransferOptions { overwrite, progress }`;`BatchTransferProgress { done, total }`
  每完成一个文件回调一次(串行)。
- `BatchTransferResult { ok: Vec<BatchTransferItem>, failures: Vec<BatchTransferFailure> }`,
  单文件失败收集后继续,不中断整批;`BatchTransferFailure` 带 per-file 错误文本。
- `upload_tree`:本地同步遍历目录树 → 远端目录逐级 `create_dir`(先 `file_exists`
  预检跳过已存在)→ `upload_many`。
- `download_tree`:先 `create_dir_all` 本地根 → `list_dir(remote, u32::MAX)` 一次取
  整棵子树 → 目录条目本地建目录 → `download_many`。

## 4. CLI 用法

`fs push/pull` 支持多目标与递归;位置参数用 `--` 分隔末尾目标(clap `last`):

```text
handshaker fs push <本地文件路径>... -- <远端文件路径>      # 单文件或批量上传
handshaker fs push --recursive ./dir -- /sdcard/dir        # 目录递归上传
handshaker fs pull <远端文件路径>... -- [本地文件路径]       # 单文件或批量下载
handshaker fs pull --recursive /sdcard/dir -- ./out        # 目录递归下载
```

- 单文件模式保持旧语义(目标已存在且未 `--overwrite` → 明确报错)。
- 批量覆盖:先预检目标是否存在,冲突时一次确认(`--yes` 通过后执行;非 TTY 需
  `--yes` 同 JSON 模式)。
- human 输出批量进度(`file.batch_progress`)与结果摘要(`file.batch_done`);
  JSON/JSONL 输出 `BatchTransferResult`(ok/failures)。
- 目录未加 `--recursive` → `cli.recursive_required` 报错。

### 4.1 dry-run(0.4.1)

`fs push/pull` 增加 `--dry-run`:`push` 用本地递归扫描,`pull` 用
`list_dir` 只读展开,输出 `{files, dirs, bytes, dry_run: true}` 报告
(human 文案 `cli.dry_run_report`)后直接返回,**不传输任何数据、不建目录、
不确认覆盖**;与 `--recursive` 组合可预演整棵目录树。

```text
handshaker fs push --dry-run --recursive ./dir -- /sdcard/dir
handshaker fs pull --dry-run --recursive /sdcard/dir -- ./out
```

### 4.2 区间下载与并发(0.4.1)

- **区间下载**:`TransferOptions.offset` / `BatchTransferOptions.offset` 指定
  起始字节,请求 `range{offset, length: 0}`(0.4.1 起)。语义为**一次性定位**
  (`FileInputStream.skip`),不是自动断点续传——原版没有续传状态,`docs/14`
  亦无续传抓包。offset 超过文件长度时手机返回空流。
- **受控并发**:`BatchTransferOptions.concurrency`(1..=8,默认 1=串行)。
  底层 `buffer_unordered` 共享同一会话(原子 sid + per-sid channel 分发);
  进度回调与聚合顺序不保证与输入一致,成功/失败计数仍准确。

## 5. 安全与已知限制

- `fetch_exif` 有 32 MiB 文件上限;媒体库/缩略图/普通响应沿用 64 MiB 双层上限。
- 批量失败信息来自协议/本地错误文本,不包含文件内容。
- 增量合并只合并事件携带的字段;快照独有字段需后续全量查询补齐。
- 批量传输默认串行;`concurrency` 1..=8 受控并发(0.4.1)。大目录下
  `download_tree` 单次 `GET_DIR_FILES` 全量拉取,受 64 MiB 响应上限保护。
- 假设备上传分支是单槽状态,并发上传无法在自动测试中验证(已用并发下载覆盖;
  真机验收覆盖并发上传)。
- `fs push/pull` 多目标语法引入 `--` 分隔(与 clap `last` 参数一致)。
- 批量进度 JSONL envelope 的 `data` 字段由单文件的 `transferred/total` 变为
  批次的 `done/total`(0.3.0 消费方需适配;envelope 外壳与 `event:"progress"`
  不变)。

### 5.1 UPDATE_FILE_INFO(40/41,0.4.1)

`HandShakerClient::update_files_info(&[RemoteFile], is_sync) -> Result<bool>`:
请求 `SSPUpdateFileRequest{type=40, files[], is_sync}`,响应
`SSPUpdateFileResponse{type=41, is_success}`(`proto/smartsync.proto:732-742`)。
字段语义来自 Android `d/c.java:508-545` 反编译(打星/旋转/时间戳/回收站回写
MediaStore;`is_sync=true` 喂 SyncManager,`docs/08` §8.11、`docs/07` 照片同步
时序)。**验证等级:反编译推断,尚无真机抓包**——实现按消息结构发送,字段回写
行为待照片同步(M6)真机验证。`is_success != true` 报协议错
(`client.update_file_info_failed`)。

## 6. 测试与验收

### 6.1 自动测试(全部通过)

- `exif_parser`:EXIF 字段解析(手工构造 TIFF/EXIF/GPS fixture)、UTC 时间转换、
  非 JPEG 报错、days_from_civil。
- client:`fetch_exif` 拉取+解析(WiFi 假设备喂 EXIF JPEG)、非 JPEG Protocol;
  `upload_many`/`download_many`/`upload_tree`/`download_tree` 假设备集成。
- `media_merge`:photo added/updated(独有字段保留)/deleted(双 key)、video/audio
  duration 秒、kind 不匹配、空变更 no-op。
- CLI:push/pull 参数顺序(`--` 分隔)、command_tree 解析、localization。
- 合计 lib 93 + bin 18 + cli 8 + localization 1 = 120。

### 6.2 真机验收(2026-08,OD103,ADB 通道)

WiFi mDNS 广播在本轮验收时不可见(手机服务重启后组播未恢复),故经已验证的
ADB forward 通道执行(SSP 通道归一,业务等价)。

- ✅ `fetch_exif` 真机照片 `IMG_20190421_173614R.jpg` 返回 orientation=1、
  make=Smartisan、model=Smartisan T1、focal=3.78、exposure=0.0333(1/30s)、iso=125、
  date_taken=1555868174(经系统 `date` 核对 = 2019-04-21 17:36:14 UTC,与 EXIF
  DateTimeOriginal 一致)。
- ✅ `fs push --recursive`(含子目录)与 `fs pull --recursive` 往返,root.bin/leaf.bin
  MD5 与原文件完全一致(64c0ece4…/c46b0731…)。
- ✅ 批量部分失败:push 含一个缺失文件 → 其余成功(ok=1),失败聚合进 `failures`
  (message 为本地错误),warning 提示"有 1 项失败"。
- ✅ 清理:远端 `hs_m5_test` 已删、临时目录/示例已删、无残留 forward。
- 增量合并语义由单元测试覆盖(媒体变更推送真机验证见 docs/20 §6)。

### 6.3 验收结果对照(§6.2 清单)

1. ✅ `fetch_exif` 真机解析成功(见 §6.2);非图片路径报协议错误由自动测试覆盖。
2. 🟡 增量合并语义由单元测试覆盖(7 项);watch 媒体变更推送真机验证见 docs/20 §6,
   本轮未做真机快照 apply 核对。
3. ✅ 目录递归往返 MD5 一致(见 §6.2)。
4. ✅ 批量部分失败聚合(见 §6.2)。
5. ✅ 验收目录与临时文件已清理。
