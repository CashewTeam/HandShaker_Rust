# 09 媒体库、缩略图、EXIF 与剪贴板

## 9.1 照片库 GET_PHOTO_LIB_REQUEST(4)

Android 查询（`f/a.java` 普通 ROM / `f/d.java` 锤子 ROM）：

- 普通 ROM：`MediaStore.Images.Media.EXTERNAL_CONTENT_URI`，投影 `{"_data","_size","date_added"}`。
- 锤子 ROM：`content://smartisanos_gallery/files`，投影含 `star/orientation/date_attribute_update`，
  条件 `media_type = 1`；star/orientation/updateTime 拼成 JSON 作为 `SSPFile.ext_data`。

响应 `SSPGetPhotoLibraryResponse`：`repeated image(SSPImageFile)` + `repeated album(SSPImageAlbum)`
+ `camera_album_id`。

`SSPImageFile` 字段映射（`h/v.java:96`）：`_data→path`、`_size→fileSize`、`date_added→createTime`、
`date_modified→modifyTime`、`width`、`height`、`orientation`、`_id→media_id`、`bucket_id→album_id`、
`mime_type`、`_display_name→title`、`bucket_display_name→album_name`、`datetaken→date_taken`、
`latitude/longitude`、`mini_thumb_magic`、`star→starred` + thumbnail。

`SSPImageAlbum`：`album_path=_data 去文件名`、`album_id=bucket_id`、`album_name=bucket_display_name`、
`cover_image`。

`camera_album_id = ("<root>/DCIM/Camera").toLowerCase().hashCode()`（`h/d.java:184`）。

## 9.2 视频库 GET_VIDEO_LIB_REQUEST(5)

Android 查询（`d/e.java:319-386`）：`MediaStore.Video.Media.EXTERNAL_CONTENT_URI`，按 `bucket_id` 排序。
`SSPVideoFile` 映射 `duration/1000`（秒）；`SSPVideoAlbum` 按 bucket 分组（albumId=bucket_id、path=目录、name=末级目录名）。
响应 `SSPGetVideoLibraryResponse`：`repeated video + repeated album`。

## 9.3 音频库 GET_AUDIO_LIB_REQUEST(6)

Android 查询（`d/e.java:76-148`）：歌曲 `h()` + 专辑 `g()`。

- 排除条件（`d/e.java:28-30`）：隐藏 bucket（`TrackAddonsProvider/hide_dir`）、非音乐/播客、
  小文件（`_size <= 800000` 且非白名单目录）、`.ogg/.3gp/.ac3` 后缀。
- 白名单目录：`/Ringtones/`、`/smartisan/music/cloud/`、`/Music/`。
- `SSPAudioFile.duration` 需 `/1000.0` 转秒（proto 注释明确）。
- 响应 `SSPGetAudioLibraryResponse`：`repeated audio + repeated album`。

## 9.4 媒体库变更推送

- 监听：`MediaDataProvider`（`h/v.java:196-213`）在专用 HandlerThread 上注册 **ContentObserver**：
  - `MediaStore.Audio/Images/Video.Media.EXTERNAL_CONTENT_URI`
  - 锤子 `smartisanos_gallery/files`、`smartisanos_gallery/bucket`、`TrackAddonsProvider/hide_dir`
- 变更 → 延迟 1000ms（防抖，`h/v.java:224-227`）→ 与内存快照 diff（按 `_id` 二分）→
  构造 `PHOTO_LIB_CHANGE(20)` / `VIDEO_LIB_CHANGE(22)`（含 added/deleted/updated）/
  `AUDIO_LIB_CHANGE(21)`。
- 推送受 `GET_DEVICE_INFO_REQUEST` 的 `need_*_callback` 开关控制。
- 照片同步进行中会跳过图片库推送（避免重复），改走 FILE_CHANGE 通道（见 [10](10-photo-sync.md)）。

## 9.5 缩略图 GET_THUMBNAIL_REQUEST(3)

- 请求：`repeated image(SSPImageFile)` + `repeated video(SSPVideoFile)` + `repeated audio_album(SSPAudioAlbum)`。
  优先按 `media_id`，无则按 `path`。
- 响应：同构，thumbnail 字节填充。

Android 生成（`d/h.java` ThumbnailHandler）：

- 线程池 10 线程 + CountDownLatch + **整体 5 秒超时**。
- **JPEG 质量固定 86**（`h.a=86`）。
- 图片：`MediaStore.Images.Thumbnails.getThumbnail(id, MINI_KIND=1)`；无 id 时
  `ThumbnailUtils.extractThumbnail` + ExifInterface 旋转补正。
- 视频：`MediaStore.Video.Thumbnails.getThumbnail(id,1)`；fallback `MediaMetadataRetriever.getFrameAtTime(-1)`，
  **等比缩放到 200 边**后 **200x200 居中裁剪**。
- 专辑封面：`content://media/external/audio/albumart/<id>`。
- 出错时 `get_thumbnail_error=true` 且 thumbnail 为空字节。

## 9.6 EXIF

- 判定（`f/c.java:69-89`）：`ExifInterface` 的 `ExifVersion` 与 `DateTime` 均非空才视为含 EXIF。
- 图片 EXIF 拉取由 Mac 侧 `SFADBForwardExifFetchOperation`（ADB 通道）实现，走手机端 shell/媒体库。
- orientation 从 ext_data JSON 读取（锤子 ROM 场景）。

## 9.7 剪贴板

协议消息见 [06](06-protobuf-schema.md) §剪切板。要点：

- `SSPClipboard.content` 是 **gzip 压缩**后的剪贴板内容；`mstimestamp` 为毫秒时间戳。
- `POST_CLIPBOARD_REQUEST` / `DELETE_CLIPBOARD_REQUEST` 的 `clipboard` 为 **required** 字段。
- Android 侧：`h/ai.java` `ClipboardManager.OnPrimaryClipChangedListener` → `CLIPBOARD_CHANGE(30)` 推送；
  `d`（Transfer 内）处理 GET/POST/CLEAR/DELETE。
- Mac 侧：`SFClipboard`（content + mstimestamp）、`SSPClipboard`；`SFDeviceClient
  getClipboard/postClipboard/clearClipboard/deleteClipboards/registerClipboardChange`。

## 9.8 存储统计

`SSPGetDeviceInfoResponse` 中的 `audio_size / pic_video_size / download_size / other_size / app_size / cache_size`：

- 图片/视频：`DCIM + MOVIES + PICTURES` 目录大小（`h/f.java:620`）。
- 下载：`DOWNLOADS`（`h/f.java:621`）。
- 音频：MediaStore 求和（`h/ac.java:53-102`）。
- 外置 SD：`StorageManager.getVolumeList()` 反射取非主卷且已挂载的路径（`h/d.java:258-281`），
  权限见 `external_storage_permission`。

## 9.9 源码索引

- Android 媒体查询：`d/e.java`；媒体变更推送：`h/v.java`、`h/x.java`
- Android 缩略图：`d/h.java`；照片同步策略：`f/a.java`、`f/d.java`
- Mac 侧：`SFPhotoLocalDataProvider`、`SFADBForwardMediaFetchOperation`（ADB 拉媒体库）、
  `SFADBForwardMediaThumbnailOperation`、`SFADBForwardExifFetchOperation`
