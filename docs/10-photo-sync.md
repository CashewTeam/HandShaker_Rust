# 10 照片同步与实时同步监控（Photo Sync）

## 10.1 概述

照片同步的目标：Mac 与手机的照片“镜像”。采用 **先全量、后增量** 的模型：

1. Mac 发 `PHOTO_SYNC_REQUEST(37)`，带上 pc_id 和“上次同步快照”文件列表。
2. 手机回 `PHOTO_SYNC_RESPONSE`：`is_first` + 当前手机照片全量列表。
3. Mac 此后用 `SYNC_MONITOR_REQUEST(39)` 开启实时监控。
4. 手机把增量变更以 `FILE_CHANGE(38)` 推送（带 `SSPFileChangeStatus`）。

## 10.2 同步状态机（Android `f/e.java` SyncManager）

```
0 = 空闲
1 = SYNCING    （PHOTO_SYNC_REQUEST 置位）
2 = MONITOR    （SYNC_MONITOR_REQUEST is_sync_monitor=true 置位）
```

| 方法 | 状态转换 | 行为 |
|---|---|---|
| `startSync`（PHOTO_SYNC_REQUEST） | 0→1 | 状态必须为 0；启动 HandlerThread；`this.a`=PC 快照列表，`this.b`=本机枚举照片 |
| `is_first` | - | `!SharedPreferences.has(pc_id)`；成功后写标记“已同步过” |
| `requestSyncMonitor` | 延迟 1500ms | 把 `this.b`（同步完成）并入 `this.a`（监控列表） |
| `doRequestSyncMonitor` | →2 / →0 | isMonitor=true：flush 未发变更、清缓冲、触发媒体库重推；false：复位、注销 observer、停线程 |

## 10.3 PHOTO_SYNC_REQUEST(37) 处理（`decoder/a.java:159-161` → `f/e.java:135-175`）

1. 状态必须为 0（空闲），否则拒绝。
2. 枚举本机照片（selection：`_data LIKE '<root>%'` + 隐藏 bucket 排除 + `media_type=1`，按 date_added desc）。
3. `is_first = !hasSeen(pc_id)`。
4. 回 `SSPPhotoSyncResponse{is_first, files=本机全量, is_success=true}`。
5. 成功后注册照片 ContentObserver，开启 PHOTO_SYNC 事件开关（`d()`，`f/e.java:238-242`）。

## 10.4 增量 diff 与 FILE_CHANGE(38)

ContentObserver 变更（防抖 ≥1000ms）→ `diff`：

- 新照片在 `this.b`(同步中) 且不在 `this.a`(监控) → **Added(1)**
- 只在一侧 → 按 `checksum`(SSPFile 字段7) / `ext_data`(字段10) 判断
  **Modified(3) / InfoModified(4) / FileAndInfoModified(5)**
- 仅存于旧列表 → **Deleted(2)**

结果打包为 `SSPFileChange{ repeated file_change_items:SSPFileChangeItem{file, status} }`，
经 PHOTO_SYNC 通道推送（type=38）。

> `status` 枚举 `SSPFileChangeStatus`：None=0 Added=1 Deleted=2 Modified=3 InfoModified=4
> FileAndInfoModified=5。

## 10.5 同步台账维护

- 手机端在 Mac 端通过 `addOrUpdate/delete`（`f/e.java:446-577`）通知后反向更新 SyncManager，避免重复 diff。
- 锤子 ROM 下若 checksum 为空，从 `PhotoExtInfo` 补（`f/e.java:468-478`）。
- `DELETE_FILE_REQUEST.is_sync` / `UPDATE_FILE_INFO.is_sync` / `DOWNLOAD.is_sync` / `UPLOAD.is_sync`
  均联动 SyncManager 台账。

## 10.6 Mac 端（客户端）照片同步

Mac 端同步由 `SFSynchManager` + `SFSyncSession` + `SFSyncFile` + 一系列 Process 类实现：

- `SFSyncNotifyBeginProcess`、`SFSyncDisableRealTimeSyncProcess`、`SFSyncCheckFreeDiskCapacityProcess`、
  `SFSyncStepGroup` 等（`HandShaker_Mac.m`）。
- 同步完成后 `notifyPhotoSyncOver`（Mac 概念，手机端无对应消息，等价物是 FILE_CHANGE 推送 +
  sync-monitor 状态切换）。
- 错误域 `com.smartisan.handshaker.filesync`，错误码 -1000（同步项缺失/一般）、-1040（实时同步
  更新失败）、-1110、-1140（传输失败）等。
- 会话文件字典 `filesDict1AfterSync`（上次会话枚举结果）用于 diff（`HandShaker_Mac.m:16610-17214`）。

## 10.7 典型时序（含实时监控）

```
Host                                    Phone
  |---- PHOTO_SYNC_REQUEST(37) ---------►|  pc_id + last snapshot files[]
  |◄--- PHOTO_SYNC_RESPONSE -------------|  is_first + current files[]
  |---- SYNC_MONITOR_REQUEST(39) -------►|  is_sync_monitor=true
  |◄--- SYNC_MONITOR_RESPONSE -----------|  is_success
  |            （手机拍照 / 相册变化）      |
  |◄--- FILE_CHANGE(38) -----------------|  [{file, Added}]
  |◄--- FILE_CHANGE(38) -----------------|  [{file, Modified}]
  |---- UPDATE_FILE_INFO(40) -----------►|  Mac 侧打星/旋转后回写
  |◄--- UPDATE_FILE_INFO_RESPONSE(41) ---|
  |---- SYNC_MONITOR_REQUEST(39) -------►|  is_sync_monitor=false
  |◄--- SYNC_MONITOR_RESPONSE -----------|
```

## 10.8 checksum 计算（手机侧）

照片同步的 `SSPFile.checksum`（`f/e.java:105-107,593-625`）：

```
checksum = MD5( 文件名(小写) + 文件长度(字符串) + Base64(文件前100字节) )
```

配合 `ext_data`（star/orientation/updateTime JSON）判断“文件变了”还是“仅元数据变了”。

## 10.9 源码索引

- Android 同步核心：`f/e.java`（SyncManager）；observer：`f/f.java`；策略：`f/a.java`、`f/d.java`
- Android 媒体库推送：`h/v.java`；EXIF：`f/c.java`
- Mac 同步：`HandShaker_Mac.m`（`SFSynchManager`、`SFSyncSession`、`SFSyncFile`）
