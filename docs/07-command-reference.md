# 07 命令参考（Command Reference）

本文列出全部 41 种消息类型、请求方向、消息内容与典型交互时序。
模式细节见 [06-protobuf-schema](06-protobuf-schema.md)，封帧见 [05-message-framing](05-message-framing.md)。

## 7.1 消息方向速查

| type | 名称 | 方向 | 说明 |
|---|---|---|---|
| 1 | HEART_BEAT_REQUEST | Host→Phone | 心跳 |
| 2 | GET_DEVICE_INFO_REQUEST | Host→Phone | 获取设备信息 + 监控开关 |
| 3 | GET_THUMBNAIL_REQUEST | Host→Phone | 批量缩略图 |
| 4 | GET_PHOTO_LIB_REQUEST | Host→Phone | 照片库 |
| 5 | GET_VIDEO_LIB_REQUEST | Host→Phone | 视频库 |
| 6 | GET_AUDIO_LIB_REQUEST | Host→Phone | 音频库 |
| 7 | GET_DIR_FILES_REQUEST | Host→Phone | 列目录 |
| 8 | GET_FILE_COUNT_REQUEST | Host→Phone | 文件计数 |
| 9 | GET_FILE_EXIST_REQUEST | Host→Phone | 文件存在性 |
| 10 | GET_CREATE_FOLDER_REQUEST | Host→Phone | 建目录 |
| 11 | GET_RENAME_FILE_REQUEST | Host→Phone | 重命名 |
| 12 | GET_DOWNLOAD_FILE_REQUEST | Host→Phone | 下载（文件→Mac） |
| 13 | GET_DOWNLOAD_FILE_RESPONSE_HEADER | Phone→Host | 下载响应头 |
| 14 | GET_DOWNLOAD_FILE_RESPONSE_BODY | （未使用） | 下载主体（proto 注释） |
| 15 | GET_UPLOAD_FILE_REQUEST_HEADER | Host→Phone | 上传请求头 |
| 16 | GET_UPLOAD_FILE_RESPONSE_HEADER | Phone→Host | 上传头响应 |
| 17 | GET_UPLOAD_FILE_REQUEST_BODY | （未使用） | 上传主体（proto 注释） |
| 18 | GET_UPLOAD_FILE_RESPONSE | Phone→Host | 上传完成响应 |
| 19 | GET_DELETE_FILE_REQUEST | Host→Phone | 删除 |
| 20 | PHOTO_LIB_CHANGE | Phone→Host | 照片库变更推送 |
| 21 | AUDIO_LIB_CHANGE | Phone→Host | 音频库变更推送 |
| 22 | VIDEO_LIB_CHANGE | Phone→Host | 视频库变更推送 |
| 23 | MONITOR_FOLDER_REQUEST | Host→Phone | 目录监控注册/注销 |
| 24 | MONITOR_FOLDER_RESPONSE_HEADER | Phone→Host | 监控注册确认 |
| 25 | MONITOR_FOLDER_RESPONSE | Phone→Host | 文件事件回调 |
| 26 | GET_CLIPBOARD_REQUEST | Host→Phone | 取剪贴板 |
| 27 | POST_CLIPBOARD_REQUEST | Host→Phone | 写剪贴板 |
| 28 | CLEAR_CLIPBOARD_REQUEST | Host→Phone | 清空剪贴板 |
| 29 | DELETE_CLIPBOARD_REQUEST | Host→Phone | 删除剪贴板条目 |
| 30 | CLIPBOARD_CHANGE | Phone→Host | 剪贴板变更推送 |
| 31 | HANDSHAKE_REQUEST_01 | Host→Phone | 握手 01 |
| 32 | HANDSHAKE_RESPONSE_01 | Phone→Host | 握手 01 响应 |
| 33 | HANDSHAKE_REQUEST_02 | Host→Phone | 握手 02 |
| 34 | HANDSHAKE_RESPONSE_02 | Phone→Host | 握手 02 响应（可多次） |
| 35 | QUIT_REQUEST | 双向 | 退出 |
| 36 | CANCEL_REQUEST | 双向 | 取消 |
| 37 | PHOTO_SYNC_REQUEST | Host→Phone | 照片同步 |
| 38 | FILE_CHANGE | Phone→Host | 文件变更（同步） |
| 39 | SYNC_MONITOR_REQUEST | Host→Phone | 实时同步开关 |
| 40 | UPDATE_FILE_INFO | Host→Phone | 更新文件元数据 |
| 41 | UPDATE_FILE_INFO_RESPONSE | Phone→Host | 更新文件元数据响应 |

## 7.2 典型交互时序

### 连接建立（WiFi/ADB）

```
Host                                Phone
  |---- HANDSHAKE_REQUEST_01(31) ----►|
  |◄--- HANDSHAKE_RESPONSE_01(32) ----|
  |---- HANDSHAKE_REQUEST_02(33) ----►|
  |◄--- HANDSHAKE_RESPONSE_02(34) ----|  (TRUST_WAITING，弹窗)
  |      [用户点“始终信任”]             |
  |◄--- HANDSHAKE_RESPONSE_02(34) ----|  (TRUST_ALWAYS + base64(RSA_enc('ok')))
  |---- GET_DEVICE_INFO_REQUEST(2) ---►|
  |◄--- GET_DEVICE_INFO_RESPONSE ------|
  |---- HEART_BEAT_REQUEST(1) ×N -----►|
  |◄--- HEART_BEAT_RESPONSE -----------|
```

### 浏览文件

```
Host                                    Phone
  |---- GET_DIR_FILES_REQUEST(7) -------►|  dir + maxdepth
  |◄--- GET_DIR_FILES_RESPONSE ----------|  dir + maxdepth + timecost + repeated file
  |---- GET_FILE_COUNT_REQUEST(8) ------►|  dir + maxdepth + exclusion_pattern[]
  |◄--- GET_FILE_COUNT_RESPONSE ---------|  count
```

### 下载

```
Host                                    Phone
  |---- GET_DOWNLOAD_FILE_REQUEST(12) --►|  file + range{offset,length} + need_md5 + gzip
  |◄--- RESPONSE_HEADER(13) -------------|  file + range(实际) + ready + data_md5
  |◄--- [sid][chunkLen] 文件字节帧流 ----|
```

### 上传

```
Host                                    Phone
  |---- UPLOAD_REQUEST_HEADER(15) ------►|  file(含大小) + data_md5 + gzip + is_sync
  |◄--- UPLOAD_RESPONSE_HEADER(16) ------|  file + ready + error_code
  |---- flag=3 数据块（[sid][flag=3][len][bytes]）►|
  |◄--- UPLOAD_FILE_RESPONSE(18) --------|  file + succeed + error_code
```

### 目录监控

```
Host                                    Phone
  |---- MONITOR_FOLDER_REQUEST(23) -----►|  file + register=true
  |◄--- MONITOR_RESPONSE_HEADER(24) -----|  succeed
  |◄--- MONITOR_FOLDER_RESPONSE(25) -----|  event[]（文件变化时）
  |---- MONITOR_FOLDER_REQUEST(23) -----►|  file + register=false （注销）
```

### 照片同步（详见 [10-photo-sync](10-photo-sync.md)）

```
Host                                    Phone
  |---- PHOTO_SYNC_REQUEST(37) ---------►|  pc_id + files[]（上次快照）
  |◄--- PHOTO_SYNC_RESPONSE -------------|  is_first + files[]（当前全量）
  |---- SYNC_MONITOR_REQUEST(39) -------►|  is_sync_monitor=true
  |◄--- SYNC_MONITOR_RESPONSE -----------|
  |◄--- FILE_CHANGE(38) ×N --------------|  file_change_items[]（增量变更）
  |---- UPDATE_FILE_INFO(40) -----------►|  files[]（打星/旋转/回收站回写）
  |◄--- UPDATE_FILE_INFO_RESPONSE(41) ---|
```

### 剪贴板

```
Host                                    Phone
  |---- GET_CLIPBOARD_REQUEST(26) ------►|  → GET_CLIPBOARD_RESPONSE clipboard[]
  |---- POST_CLIPBOARD_REQUEST(27) -----►|  required clipboard → POST_CLIPBOARD_RESPONSE succeed
  |---- CLEAR_CLIPBOARD_REQUEST(28) ----►|  → CLEAR_CLIPBOARD_RESPONSE succeed
  |---- DELETE_CLIPBOARD_REQUEST(29) ---►|  required clipboard → DELETE_CLIPBOARD_RESPONSE succeed
  |◄--- CLIPBOARD_CHANGE(30) ------------|  手机剪贴板变化时推送
```

## 7.3 各请求关键字段与校验（Android 处理）

| 请求 | 关键字段 | 校验 / 特殊行为 | 来源 |
|---|---|---|---|
| GET_DIR_FILES(7) | dir, maxdepth | 深度限制；返回 timecost | `d/c.java:110-190` |
| GET_FILE_COUNT(8) | dir, maxdepth, exclusion_pattern | 排除用正则 `FilenameFilter` matches | `d/c.java:470-483,793-810` |
| GET_FILE_EXIST(9) | file | 路径存在性 | `d/c.java` |
| CREATE_FOLDER(10) | file | 校验路径/权限 | `d/c.java` |
| RENAME(11) | source_file, target_file | 校验源存在/目标冲突/系统文件 | `d/c.java` |
| DELETE(19) | file[], is_sync, is_trash | 递归删除；回收站（is_trash）走媒体库 update | `d/c.java` + `f/e.java` |
| MONITOR_FOLDER(23) | file, register | 非 /system、需写权限、必须目录 | `d/c.java:552-582` |
| DOWNLOAD(12) | file, range, need_md5, gzip | range 按文件大小裁剪；MD5 见下 | `d/c.java:606-671` |
| UPLOAD_HEADER(15) | file, data_md5, gzip, is_sync | 空间/路径校验；覆盖已有文件 | `d/c.java:334-409` |
| THUMBNAIL(3) | image[], video[], audio_album[] | 并发线程池 + 5s 超时；JPEG 质量 86 | `d/h.java` |
| DEVICE_INFO(2) | 各 need_*_callback 开关 | 版本兼容检查；保存开关 | `decoder/a.java:66-84` |
| PHOTO_SYNC(37) | pc_id, files[] | 状态机 0→SYNCING；is_first | `f/e.java:135-175` |
| SYNC_MONITOR(39) | is_sync_monitor | 状态机 →MONITOR / →空闲 | `f/e.java:177-236` |
| UPDATE_FILE_INFO(40) | files[], is_sync | 写回 MediaStore（date_added/datetaken/orientation） | `d/c.java:508-545` |
| HEART_BEAT(1) | host_timestamp | 回显 + client_timestamp | `g/j.java:264-266` |

### 下载 MD5

服务器 `need_md5` 响应头中的 `data_md5`：当前 Android 实现 `d/c.java` 在响应头里 data_md5 填空串，
进度由客户端按 header.range 自算。若需兼容旧版/其他实现，客户端应自行计算 MD5 校验。

### 上传 MD5

`data_md5` 非空时 Android 会做 MD5 校验；失败返回 `FILE_IO_MD5_CHECK_ERROR(7)`。

## 7.4 推送消息（Phone→Host）

| 消息 | 触发 | payload |
|---|---|---|
| PHOTO_LIB_CHANGE(20) | MediaStore 图片库 ContentObserver 变更 | added_image[] / deleted_image[] |
| VIDEO_LIB_CHANGE(22) | MediaStore 视频库变更 | added / deleted / updated |
| AUDIO_LIB_CHANGE(21) | MediaStore 音频库变更 | added_audio / deleted_audio / added_album |
| CLIPBOARD_CHANGE(30) | 手机剪贴板变化 | clipboard[] |
| MONITOR_FOLDER_RESPONSE(25) | 监控目录 inotify 事件 | SSPFileEvent[] |
| FILE_CHANGE(38) | 照片同步 diff | SSPFileChangeItem[] |

> 媒体库推送受 `GET_DEVICE_INFO_REQUEST` 中 `need_photo/audio/video_library_callback` 开关控制
> （`EventManager.a(type, data)`，`aoa/a/a.java:80-92`）。

## 7.5 源码索引

- Android 命令分发：`decoder/a.java`（type→Handler 映射）
- Android 文件操作：`d/c.java`；媒体：`d/e.java`；同步：`f/e.java`；缩略图：`d/h.java`
- 传输/握手：`g/j.java`、`g/h.java`
- Mac 侧对应 Operation 类：`SSPGetDirFilesRequestOperation`、`SSPUploadFileRequestOperation`、
  `SSPDownloadFileRequestOperation`、`SSPThumbnailRequestOperation` 等（见 [12-macos-implementation](12-macos-implementation.md)）
