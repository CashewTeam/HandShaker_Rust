# 08 文件操作（File Operations）

文件操作是 SSP 的核心能力：列目录、计数、存在性、建目录、重命名、删除、下载、上传、目录监控。
全部由 Mac（Host）发起，手机（APK）执行。

## 8.1 文件对象 `SSPFile`

| field | 含义 | 说明 |
|---|---|---|
| 1 | `path` | 绝对路径（手机侧） |
| 2 | `file_size` | 字节 |
| 3 / 4 | `created_timestamp` / `modified_timestamp` | Unix 秒 |
| 6 | `isDirectory` | |
| 7 | `checksum` | 同步用校验和：MD5(文件名小写 + 文件长度 + Base64(文件前100字节)) |
| 8 | `file_type` | NORMAL=0 / DATA=1 |
| 9 | `prefixMd5` | 文件前缀 MD5 |
| 10 | `ext_data` | 扩展数据 JSON（锤子 ROM 图片的 star/orientation/updateTime） |
| 11-14 | `is_trash`/`succeed`/`error_code`/`id` | 删除响应 & 媒体库 id |

## 8.2 列目录 GET_DIR_FILES_REQUEST(7)

- 请求：`{type, dir:SSPFile, maxdepth}`；`maxdepth=1` 只取当前目录。
- 响应：`{type, dir, maxdepth, timecost(ms), repeated file:SSPFile}`。
- Android 处理：`d/c.java:110-190`；目录以 `SSPFile.isDirectory=true` 返回。
- 文件类型仅 `NORMAL/DATA` 两档，细分（图片/视频/音频/下载等）由客户端按目录推断。

### 根目录

Mac 通过 `GET_DEVICE_INFO_RESPONSE.root_path`（主存储）与 `external_storage_path`（外置 SD）获得根。
手机上文件系统检查仅硬编码 `startsWith("/system")` 判为系统文件（`h/s.java:8-10`）。

## 8.3 文件计数 GET_FILE_COUNT_REQUEST(8)

- 请求：`{type, dir, maxdepth, repeated exclusion_pattern}`。
- 响应：`{type, dir, maxdepth, exclusion_pattern, count}`。
- 排除规则：由 **Mac 下发正则**，手机用 `FilenameFilter.accept` 的 `name.matches(pattern)` 命中即排除
  （`d/c.java:793-810,470-483`）。典型用于下载/照片/视频/音频目录的数量统计。

## 8.4 存在性 GET_FILE_EXIST_REQUEST(9)

- 请求：`{type, file}`；响应 `{type, file, exist}`。
- Mac 侧 `isFileExistWithPath:` / `isDirectoryExistWithPath:`。

## 8.5 建目录 GET_CREATE_FOLDER_REQUEST(10)

- 请求：`{type, file}`（file 为目标目录，path 必填）。
- 响应：`{type, file, succeed, error_code, error_message}`。
- 校验：路径合法性、写权限；重复/非法名返回对应 `SSPFileIOError`。

## 8.6 重命名 GET_RENAME_FILE_REQUEST(11)

- 请求：`{type, source_file, target_file}`（target 只需 path + 新名）。
- 响应：`{type, source_file, target_file, succeed, error_code, error_message}`。
- 错误场景：源不存在 → `FILE_IO_INVALID_SOURCE(3)`；目标已存在 → `FILE_IO_TARGET_ALREADY_EXIST(4)`；
  系统文件 → `FILE_IO_SYSTEM_FILE(8)`；名字超长 → `FILE_IO_PATH_OR_NAME_TOO_LONG(11)`。

## 8.7 删除 GET_DELETE_FILE_REQUEST(19)

- 请求：`{type, repeated file, is_sync, is_trash}`；file 可为目录（递归删除全部内容）。
- 响应：`{type, repeated file, succeed, error_code, error_message}`。
- `is_trash=true`：移动回收站（媒体库 update，`is_trash` 在 `SSPFile` 字段 11 回填）。
- `is_sync=true`：维护同步台账（`f/e.java` SyncManager）。
- Android 处理：`d/c.java` + `SFDeviceClient deleteSFFiles:...isSyncScene:isTrash:needUpdateMediaLibrary:...`。
- 错误场景：权限 → 5；SD 卡拔出 → 9；取消操作 → 12。

## 8.8 下载 GET_DOWNLOAD_FILE_REQUEST(12)（文件 → Mac）

- 请求：`{type, file, range{offset,length}, need_md5, gzip, is_sync}`。
  - `range.length=0` → 全量；`length > 剩余` → 返回剩余字节（服务器裁剪）。
- 响应：`GET_DOWNLOAD_FILE_RESPONSE_HEADER(13)` `{type, file, range(实际), need_md5, data_md5, ready, error_code}`，
  然后以 `[sid][chunkLen]` 帧流直接发文件字节（见 [05](05-message-framing.md) §5.2.2）。
- 不支持断点续传的“请求任意区间”之外的语义：range 只做一次性定位（`FileInputStream.skip(start)`）。
- 取消：flag=2 或服务器 `CANCEL_REQUEST(36)`；服务器侧 `stopWriteFile` 中止文件流（`g/a.java:244-250`）。
- Mac 端实现：`SSPDownloadFileRequestOperation`（累积数据到 bufferPath，计算 MD5，进度回调）。

## 8.9 上传 GET_UPLOAD_FILE_REQUEST_HEADER(15)（Mac → 文件）

- 请求头：`{type, file(含大小), data_md5, gzip, is_sync}`；file 已存在则**覆盖**。
- 响应头：`GET_UPLOAD_FILE_RESPONSE_HEADER(16)` `{type, file, ready, error_code}`。
- 数据面：Mac 以 **flag=3** 分块发送原始文件字节（帧 `[sid:4][flag=3][len:4][bytes]`，len≤4MB）。
- 完成响应：`GET_UPLOAD_FILE_RESPONSE(18)` `{type, file, canceled, succeed, error_code}`。
  - 成功后会触发媒体库扫描（`MediaScannerService.scanFile`）。
  - 失败会删除半成品并回 `CANCEL_REQUEST`。
- `data_md5` 非空则服务器校验，失败 → `FILE_IO_MD5_CHECK_ERROR(7)`。
- Mac 端实现：`SSPUploadFileRequestOperation`（分块读文件、进度、`isReadyToUploadFileData`）。

## 8.10 目录监控 MONITOR_FOLDER_REQUEST(23)

- 请求：`{type, file, register}`；`register=true` 开始监控，false 停止。
- 确认：`MONITOR_FOLDER_RESPONSE_HEADER(24)` `{type, succeed, error_message}`。
- 事件：`MONITOR_FOLDER_RESPONSE(25)` `{type, repeated event:SSPFileEvent}`；
  `SSPFileEvent = {file:SSPFile, event:SSPFileEventType}`。
- Android 监控实现（`h/ab.java` 工厂 + `h/r.java`）：
  - 路径为 MediaStore URI → `ContentStorageChangeObserver`（ContentObserver，事件不推送）。
  - 普通路径 → `FileStorageChangeObserver`：**FileObserver mask = 0xFC8** =
    `CREATE(0x100)|DELETE(0x200)|CLOSE_WRITE(0x8)|MOVED_FROM(0x40)|MOVED_TO(0x80)|DELETE_SELF(0x400)|MOVE_SELF(0x800)`。
  - 外置 SD（DocumentsContract）→ `DocumentFileChangeObserver`。
- 事件 → `SSPFileEventType` 映射（`a/b.java:52-94`）：

| FileObserver 掩码 | SSPFileEventType |
|---|---|
| 0x8 (CLOSE_WRITE) | FILE_EVENT_CLOSE_WRITE(3) |
| 0x40 (MOVED_FROM) | FILE_EVENT_MOVED_FROM(4) |
| 0x80 (MOVED_TO) | FILE_EVENT_MOVED_TO(5) |
| 0x100 (CREATE) | FILE_EVENT_CREATE(1) |
| 0x200 (DELETE) | FILE_EVENT_DELETE(2) |
| 0x400 (DELETE_SELF) | FILE_EVENT_DELETE_SELF(6) |
| 0x800 (MOVE_SELF) | FILE_EVENT_MOVE_SELF(7) |
| 0xFC8 全量 | FILE_EVENT_DIR_CHANGED(8) |

- 校验（`d/c.java:552-582`）：SD 未拔出、非 `/system`、有写权限、必须是目录。
- Mac 端：`SFADBManager watchDirectory:changed:` / `SSPManager watchDirectory:changed:`，
  watch 由 `SSPWatchCallbackItem`/`WatchCallbackItem` 按 uuid 管理。

## 8.11 更新文件元数据 UPDATE_FILE_INFO(40)

- 请求：`{type, repeated files, is_sync}`。
- 响应：`UPDATE_FILE_INFO_RESPONSE(41)` `{type, is_success}`。
- Android 处理（`d/c.java:508-545`）：解析 `ext_data`（star/orientation/updateTime）后写回
  MediaStore.Images（`date_added`, `datetaken`, `orientation`）；锤子 ROM 额外更新
  `smartisanos_gallery/files`。用于 Mac 端打星标、旋转、回收站等元数据回写。
- `is_sync=true` 时同步喂给 SyncManager（`decoder/a.java:166-175`）。

## 8.12 文件路径 & 目录约定

- 手机侧主存储根：`Environment.getExternalStorageDirectory()`（如 `/storage/emulated/0`）。
- 相机目录：`<root>/DCIM/Camera`；`cameraAlbumId = <该目录小写>.hashCode()`。
- 下载/fromMac 等目录是 **Mac 端概念**，上传目标路径完全由 Mac 在 header 里指定。
- Mac 端 `SFADBManager` 缓存了手机目录树（rootDir、sdcardRootDir、fromMacDir、downloadDir、audioDir、
  cameraDir、videoDir、screenshotsDir、quickcaptureDir）。

## 8.13 Rust 实现要点

- 列目录：以 `maxdepth` 递归；注意手机返回的 `path` 直接可当本地路径用。
- 下载：先收 header（含实际 range），再按帧流收数据，自行做 MD5（服务器不保证填 data_md5）。
- 上传：header 请求 → 等 ready → flag=3 分块（≤4MB/块）→ 等 type 18。
- 删除：一次可批量；is_trash 决定是否进回收站。
- 监控：注册后收 `MONITOR_FOLDER_RESPONSE(25)` 事件；注意 FileObserver 事件为 inotify 风格。
