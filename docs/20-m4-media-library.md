# 20 M4：媒体库与缩略图（设计与实现记录）

> 状态基线：2026-08，Cargo package `handshaker_rust 0.3.0`。
> 本文记录 M4 的实现范围、协议依据、CLI 预览上限、EXIF 扩展点与验证结果；
> 行为细节以源码与自动化测试为准。

## 1. 目标与范围

M4 覆盖图片、视频、音频三类媒体库的查询、缩略图获取与 CLI 预览：

- library：`get_photo_library` / `get_video_library` / `get_audio_library`（全量快照）；
- library：`get_thumbnails`（按 media_id/path 批量请求，失败条目单独标记）；
- CLI：`media photo|video|audio`（**默认预览上限 50 条**，`--limit`/`--all` 覆盖）与
  `media thumbnail <id|path>... --output-dir <dir>`；
- EXIF 扩展点：`ExifData` 领域类型与 `fetch_exif` 预留接口（未实现，规划 M5）。

不在本里程碑范围：独立 EXIF 拉取（原版走 ADB shell 通道，M5）；媒体库变更增量合并；
媒体库分页（协议请求无分页参数，响应全量，客户端仅裁剪预览）。

## 2. 协议依据

> ⚠️ 验证等级：**APK 反编译推断**（docs/09），无真实抓包向量；真机行为见 §6.1。

- `GET_PHOTO_LIB_REQUEST(4)`（空请求）→ `{repeated image(SSPImageFile),
  repeated album(SSPImageAlbum), camera_album_id}`；`camera_album_id = ("<root>/DCIM/Camera")
  .toLowerCase().hashCode()`。
- `GET_VIDEO_LIB_REQUEST(5)` → `{repeated video, repeated album}`；
  `SSPVideoFile.duration` 为秒。
- `GET_AUDIO_LIB_REQUEST(6)` → `{repeated audio, repeated album}`；
  `SSPAudioFile.duration` 为毫秒，需 `/1000.0` 转秒。
- `GET_THUMBNAIL_REQUEST(3)` → 同构响应填充 `thumbnail` 字节；失败条目
  `get_thumbnail_error=true` 且省略 thumbnail；JPEG 质量固定 86。
- 媒体变更推送 `PHOTO_LIB_CHANGE(20)`/`AUDIO_LIB_CHANGE(21)`/`VIDEO_LIB_CHANGE(22)`
  M1 已解码、M3 `watch` 已输出。

## 3. 实现

### 3.1 library API

```rust
pub async fn get_photo_library(&self) -> Result<PhotoLibrary>          // + _with_options
pub async fn get_video_library(&self) -> Result<VideoLibrary>          // + _with_options
pub async fn get_audio_library(&self) -> Result<AudioLibrary>          // + _with_options
pub async fn get_thumbnails(&self, images, videos, audio_albums) -> Result<Thumbnails>
                                                                       // + _with_options
pub async fn fetch_exif(&self, path: &str) -> Result<ExifData>         // 预留，未实现
```

- 查询响应全量映射到领域类型（`ImageFile`/`ImageAlbum`/`VideoFile`/`VideoAlbum`/
  `AudioFile`/`AudioAlbum`，字段覆盖 EXIF 方向、经纬度、收藏、date_taken 等）；
  `audio duration /1000.0` 转秒；`thumbnail_error`/`starred` 缺省为 `false`。
- 缩略图请求按 `media_id` 优先、`path` 兜底；响应条目 `get_thumbnail_error` 单独标记，
  不整批失败。

### 3.2 CLI `media`

```
handshaker [--wifi IP:PORT | --serial SN] media photo|video|audio [--limit N | --all]
handshaker [--wifi IP:PORT | --serial SN] media thumbnail <id|path>... --output-dir <dir>
```

- **预览上限**：`DEFAULT_MEDIA_PREVIEW_LIMIT = 50`；`--limit` 覆盖、`--all` 全量。
  超出时只输出前 N 条，human 附加"共 {total} 条，仅显示前 {N} 条"提示，
  json 附加 `total` 与 `truncated: true`（调用方可知被截断）。协议请求无分页参数，
  上限只在 CLI 输出层裁剪。
- human 输出为制表符分隔行（photo：序号/标题/宽x高；video：序号/路径/时长；
  audio：序号/曲名/歌手/路径）；json 输出完整对象（含 albums）。
- `media thumbnail`：数字参数按 media_id、其余按远程路径；缩略图写入
  `--output-dir`，文件名为本地生成的 `{index}_{本地名}.jpg`（不信任远端文件名）；
  `get_thumbnail_error`/缺数据条目写入 `failed` 列表并 stderr 提示，不中断成功条目。

## 4. EXIF 扩展点（预留接口）

- `ExifData { orientation, date_taken, latitude, longitude }` 已作为公开领域类型定义。
- `HandShakerClient::fetch_exif(path)` 公开签名存在，当前恒返回
  `Error::Protocol("exif.not_implemented")`（退出码 5），不触碰手机；
  doc 注释标注 M5 规划走 ADB shell 通道（原版 `SFADBForwardExifFetchOperation`）。
- 查询响应中的 `orientation`/`latitude`/`longitude`/`date_taken` 已随媒体库返回，
  独立 EXIF 拉取（含 ROM 差异的 ext_data 解析）留待 M5。

## 5. 公开 API 变更（相对 0.2.0）

- 新增：`ImageFile`/`ImageAlbum`/`VideoFile`/`VideoAlbum`/`AudioFile`/`AudioAlbum`、
  `PhotoLibrary`/`VideoLibrary`/`AudioLibrary`/`Thumbnails`/`ExifData` 领域类型；
  `get_photo_library`/`get_video_library`/`get_audio_library`/`get_thumbnails`/`fetch_exif`；
  CLI `media` 子命令。
- 无破坏性变更：M1 的 `MediaItem`/`MediaLibraryChange` 事件模型保持不变（事件与查询分离）。
- 版本从 `0.2.0` 升至 `0.3.0`（功能里程碑，由维护者决定）。

## 6. 测试与验收

- 单元/集成：`wifi_media_libraries_decode_and_map_fields`（photo/video/audio 字段映射、
  audio duration 毫秒转秒）、`wifi_thumbnails_carry_bytes_and_report_failed_entries`
  （JPEG 字节、失败条目标记）、`fetch_exif_is_a_reserved_interface`（Protocol/退出码 5）、
  `media_preview_limit_defaults_to_preview_cap`、`media_truncation_reports_total_and_flags`、
  `thumbnail_file_names_are_local_and_safe`（防路径穿越）、`media_help_is_localized`。

### 6.1 真机验收（2026-08，Smartisan OD103，Android 7.1.1）

> 环境：Mac 与手机（192.168.2.47）同一局域网；隔离 HOME；手机端首次连接信任确认后执行。
> 验收后测试图片、临时目录与进程均已清理。

| 验收项 | 结果 |
|---|---|
| photo 库查询 | ✅ 返回 3005 张图片 + 25 个相册；字段完整（media_id/album_id/宽高/orientation/date_taken/经纬度(Polarr 相册 30.279339/120.16548)/mini_thumb_magic/starred/mime） |
| **预览上限** | ✅ json `"total":3005,"truncated":true` 只输出前 50 条；human 51 行 + "共 3005 条，仅显示前 50 条"；`--limit 3` 生效 |
| video 库查询 | ✅ 相册（Camera/weiboIntl_video/ScreenRecorder 等）与视频条目 |
| audio 库查询 | ✅ 专辑（含 artist ミツキヨ、专辑名、year 等）与曲目 |
| thumbnail 写文件 | ✅ `media thumbnail 620925 613610 --output-dir` 写入 `0_620925.jpg`/`1_613610.jpg`，**JPEG magic `ff d8 ff`** 可解码；json 返回 written 列表 |
| watch 媒体变更 | ✅ 推送图片并广播 MediaScanner 后 watch 输出 `media_library_changed`（kind=photo，added 含新图片全字段）——验证 watch 需开启全部 callback（`connect_with_event_callbacks`） |
| 清理 | ✅ 手机测试图片已删、临时 HOME/缩略图目录已删、无残留进程 |

**结论**：三类媒体库查询、预览上限（默认 50）、缩略图写入（JPEG 可解码）与媒体变更推送全部真机验证通过；
这是媒体库协议首次真机互通证据（此前仅 APK 反编译推断）。
