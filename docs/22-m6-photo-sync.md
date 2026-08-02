# M6:照片同步与实时同步(0.5.0)

本文记录 M6(0.5.0)的设计、实现依据与验收。自动测试与真机验证状态见 §6。

## 1. 范围

- 发送侧落地:`PHOTO_SYNC_REQUEST(37)` 与 `SYNC_MONITOR_REQUEST(39)` 的 host→phone
  发送 API(`client.photo_sync` / `client.sync_monitor`);`FILE_CHANGE(38)` 增量处理。
- 完整增量状态机:全量 diff → 执行(下载/删除)→ 台账原子提交 → 实时监控增量落账。
- 方向固定为**单向:手机 → 主机**(用户批准),不做上传方向。
- CLI:`sync plan` / `sync run` / `sync watch` / `sync status`。
- 独立同步台账文件 `<config>/sync/<device_uuid>.json`(0600,目录 0700,原子提交)。

明确不做:上传方向、跨设备冲突合并、daemon 化、视频/音频同步(仅照片)。

## 2. 实现依据与验证等级

| 事实 | 依据 | 等级 |
|---|---|---|
| `PHOTO_SYNC_REQUEST(37){pc_id, files=上次快照}` → `PHOTO_SYNC_RESPONSE{is_first=!hasSeen(pc_id), files=当前状态, is_success}` | `proto/smartsync.proto` 674-689、docs/10 §10.2-10.3、**smali `a$dp.smali:597-612`(1.2.0-r7c)、mac 端 SmartFinderCore.h `SSPPhotoSyncRequest`(pcId=macUUID)** | 三端交叉确认 |
| `SYNC_MONITOR_REQUEST(39){is_sync_monitor}` → 手机推 `FILE_CHANGE(38){file_change_items}`;状态 0=空闲/1=SYNCING/2=MONITOR | docs/10 §10.2、docs/07 §7.2 | 反编译推断 |
| diff 依据:checksum(`MD5(文件名小写+长度+Base64(前100字节))`,手机端计算)区分内容变更;ext_data JSON(star/orientation/updateTime)区分仅元数据变更;status Added/Deleted/Modified/InfoModified/FileAndInfoModified | docs/10 §10.4/§10.8、`SSPFileChangeStatus` | 反编译推断 |
| 下载/删除由 host 侧对照本地台账执行(手机只回当前状态列表与增量事件) | M6 设计决策(用户批准) | 设计决策 |
| 单向同步冲突策略:本地文件 SHA-256 与台账一致 → 按手机状态执行;不一致 → 保留并报告冲突 | M6 设计决策(用户批准) | 设计决策 |
| 幂等:重跑用旧台账快照,`PHOTO_SYNC_REQUEST` 幂等;下载写 `.hs-part` + SHA-256 校验后原子 rename;台账整体原子提交 | M6 设计决策(用户批准) | 设计决策 |
| 37/38/39 发送侧此前无实现,docs/14 无抓包向量 | 调研 | 代码事实 |

> ⚠️ **验证等级**:**37/38/39 发送侧为本仓库首次真机互通**。协议依据已由三端交叉确认:
> `proto/smartsync.proto`、jadx(1.2.0-r6)、**smali(1.2.0-r7c,`a$dp.smali:597-612`
> 与 `f/e.smali` SyncManager)**、**mac 端参考源码(SmartFinderCore.h `SSPPhotoSyncRequest`/
> `SSPPhotoSyncResponse` + `SFDeviceClient execPhotoSyncWithMacUUID:lastFiles:`)**
> 完全一致;仍无真实抓包向量,真机验收受限(§6.2)。

## 3. 领域与台账

### 3.1 领域类型(`src/domain.rs`)

```rust
pub struct SyncConfig { device_uuid, phone_root, local_root, pc_id }
pub struct SyncFileRecord { size, checksum, ext_data, modified_at, local_path, local_sha256 }
pub struct SyncSnapshot { files: BTreeMap<String, SyncFileRecord> }   // 键 = 手机绝对路径
pub struct SyncDiff { added, info_modified, deleted, conflicts }
pub struct PhotoSyncResult { is_first, files, is_success }
```

- `RemoteFile` 新增 `ext_data: Option<String>`(手机端 star/orientation/updateTime
  JSON,host 视为不透明),`ssp_file_from_remote`/`remote_file` 双向透传,供
  `UPDATE_FILE_INFO(40)` 回写与 diff 分类使用。

### 3.2 台账(`src/sync_store.rs`)

- 路径:`<config>/sync/<device_uuid>.json`(与 `state.json` 同级,`sync/` 子目录),
  文件 0600、目录 0700;信封 `{schema_version: 1, snapshot}`。
- **原子提交**:写 `<uuid>.json.tmp` → `fsync` → `rename` → 父目录 `fsync`;
  任何一步失败删除临时文件,旧台账保持完整。
- `load` 对损坏/版本不符返回 `Error::Configuration`(**不静默重建**),操作者可人工恢复。
- `pc_id` = host_uuid **原文**(来自 `state.json`,跨运行稳定;与 mac 端 `SFGenericDevice
  getMacUUID` 作为 `pcId` 的语义一致,见 docs/10 §10.6.1;手机端以此判断 `is_first`)。

## 4. 同步引擎(`src/sync.rs`)

### 4.1 plan(diff)

```rust
pub fn plan_diff(phone_files: &[RemoteFile], snapshot: &SyncSnapshot) -> SyncDiff
pub fn check_conflicts(diff: &SyncDiff, snapshot: &SyncSnapshot) -> Vec<String>
```

- 分类:手机有/台账无 → `added`;checksum 变 → `added`(重下载);checksum 同但
  ext_data/modified_at 变 → `info_modified`;台账有/手机无 → `deleted`。
- 冲突:计划触碰(下载/删除)且本地文件 SHA-256 与台账不符 → 保留并报告,
  执行时跳过。

### 4.2 run(执行)

```rust
pub async fn execute_plan(client, config, phone_files, diff, snapshot, conflicts)
    -> Result<(SyncRunResult, SyncSnapshot)>
```

- 串行执行;每个下载:目标路径 `local_destination`(`strip_prefix(phone_root)` +
  组件白名单拒绝 `..`/越界)→ 建父目录 → 下载到 `<目标>.hs-part` → SHA-256 →
  `rename`;删除:台账 `local_path` 存在则删;`info_modified` 仅更新台账字段。
- 失败聚合到 `failures` 不中断;调用方拿到更新快照后 `SyncStore::save` 原子提交。
- 重跑幂等:diff 为空;残留 `.hs-part` 由下次下载覆盖(失败路径主动清理)。

### 4.3 增量与实时

```rust
pub async fn apply_file_change(client, config, change: &FileChange, snapshot: &mut SyncSnapshot)
    -> Result<SyncRunResult>
```

- Added/Modified/FileAndInfoModified → 下载 + 台账 insert;Deleted → 删本地 +
  台账移除;InfoModified/None/Unknown → 仅更新台账元数据。
- 实时:`sync watch` 先跑一次全量(37 → diff → 执行),再 `sync_monitor(true)`
  (39)并订阅事件;`FILE_CHANGE(38)` 逐条 `apply_file_change` 后每批原子保存台账;
  Ctrl-C 时 `sync_monitor(false)` 注销 + 保存后返回 `Error::Interrupted`。

## 5. CLI 用法

```text
handshaker sync plan   [--root <手机端照片根目录>] --output-dir <本地目录>
handshaker sync run    [--root <...>] --output-dir <本地目录> [--yes]
handshaker sync watch  [--root <...>] --output-dir <本地目录>
handshaker sync status
```

- `--root` 默认 `/storage/emulated/0/DCIM/Camera`;`--output-dir` 必填(本地下载根)。
- `run` 遵循确认策略:非 TTY 或 JSON 模式必须 `--yes`,否则 exit 8
  `confirmation_required`。
- `plan` 输出 JSON:`{added, info_modified, deleted, conflicts, total}`(不写任何文件)。
- `run` 输出 JSON:`{downloaded, deleted, failures, conflicts}`。
- `watch` 输出 JSONL 信封:`{"schema_version":1,"command":"sync.watch","data":{"applied","failures"}}`;
  human 输出经 `sanitize_human` 防终端注入。
- `status` 输出台账摘要:`{device_uuid, files, bytes}`。
- `sync watch` 不支持在 REPL 中嵌套(与 `watch` 一致)。

## 6. 验证与验收

### 6.1 自动化测试(146 个全通过:lib 114 / bin 21 / cli 10 / localization 1)

- `sync_store` 4 个:首写 0600/0700、原子提交无残留 tmp、损坏/版本不符硬错误、
  pc_id 稳定。
- 发送侧:`photo_sync_sends_snapshot_and_returns_phone_state`(37 携带 pc_id/快照,
  回显 minus 删除项)、`sync_monitor_enable_receives_file_change_push`(39 开启后
  收到 38 Added 推送)。
- 引擎:`plan_diffs_added_modified_info_and_deleted`、`local_destination_rejects_escapes`、
  `conflicts_flag_user_modified_local_files`;集成 `execute_plan_downloads_new_files_and_rerun_is_idempotent`
  (首同步+重跑空 diff+part 无残留)、`execute_plan_deletes_locally_when_phone_file_disappears`、
  `execute_plan_preserves_user_modified_local_files_as_conflicts`、
  `apply_file_change_adds_then_deletes_and_updates_metadata`(Added→下载、
  InfoModified→仅台账、Deleted→清理)。
- CLI:`sync_commands_parse_and_require_output_dir`(status 解析、run 缺
  --output-dir exit 2 usage、plan --output-dir 解析、sync --help 本地化)。

### 6.2 真机验收(OD103,2026-08-03)

受控执行(隔离 HOME、唯一测试目录 `hs_m6_test2/3`、MediaScanner 广播入库、验收后清理):

| 步骤 | 结果 |
|---|---|
| 首次 `sync run --yes`(pc_id 带 `hs-` 前缀) | **受阻**——手机返回 HEART_BEAT 型响应 |
| **pc_id 改为 host_uuid 原文后** `sync run` | **37 成功**:手机返回 `PHOTO_SYNC_RESPONSE`(500 KB 照片列表,`is_first` 首次 true/复跑 false) |
| 重复 37(进程内第二次) | **被拒**(`is_success=false`)——手机端 SyncManager 处于 SYNCING(status=1)拒绝再次 startSync,与 `f/e.java` 一致;**已修复**:run/watch 只发一次 37 |
| 39 行为 | `SYNC_MONITOR_REQUEST` 在 status=0 时返回 `is_success=false`(参考 `f/e.java` requestSyncMonitor 需 status≠0);**已修复**:`sync_monitor` 返回 `Result<bool>` 由调用方决定;run 结束发送 39(false) 复位。**注意**:run 前置 39(false) 复位曾导致后续 37 被按心跳处理(真机观察),已移除前置复位 |
| 照片枚举 | 37 响应为手机照片库列表。**交叉验证确认**:1.2.0 枚举走锤子相册 `content://smartisanos_gallery/files`,selection = `_data LIKE '/storage/emulated/0%' AND bucket_id NOT IN (status=2) AND media_type = 1`;新 push 到**非相册目录**的文件 `bucket_id=-837405474`(status=2 排除项)故不在响应;`DCIM/Camera` 下新照片 bucket 正常 → **实测进入响应**(`sync plan` added 第一项) |
| 顺带发现 1 | 手机端会**主动推送** `type=37` 消息(500 KB 照片状态列表)——与 docs/14 §7.5 推送机制一致,事件解码已支持 |
| 顺带发现 2 | 业务请求 sid 与手机推送 sid 同从 `0x80000001` 递增会碰撞(docs/14 §7.5 已预警)——**已修复**:业务请求 sid 起点改为 `0x80001000`(session.rs) |
| 顺带发现 3 | `PHOTO_SYNC_RESPONSE` 成功时**省略 `is_success` 字段**(proto2 默认字段省略,与 AGENTS 不变量一致)——判定改为仅显式 `false` 视为拒绝 |
| 清理 | `hs_m6_test2/3` 已删除、/tmp 测试目录/日志/台账已删、无残留 adb forward |

**结论**:照片同步**协议层**(37 请求/响应、pc_id、`is_first`、状态机保护、39 语义)已在真机
验证:37 成功返回照片列表、重复 37 按 SYNCING 拒绝、39 的 status 依赖一致。**照片枚举**
经原始 1.2.0 smali 交叉验证确认(锤子相册 provider + `media_type=1` + `bucket_id NOT IN
(status=2)` 排除);新 push 到非相册目录的文件因 bucket 排除不进响应,`DCIM/Camera` 下
新照片正常进入。**端到端下载链路真机验证完成(2026-08-03)**:测试照片置于 `DCIM/Camera`
(相册可见)→ `sync run --root DCIM/Camera` 下载 41 张到隔离临时目录 → 测试文件 **MD5
与源完全一致** → 重跑**幂等空 diff** → 手机端删除后重跑**本地对应删除** → 台账原子写入
(0600,phone_id 键)。隔离目录/台账/forward 全部清理,未改动手机端任何用户文件(仅
push/删除自己的测试照片)。
验证等级「原始 1.2.0 smali + mac 端交叉确认 + 自动化测试 + 真机 E2E」。

**原始 1.2.0 smali 交叉验证(2026-08-03,从真机提取 APK 反编译,`original_smali_1.2.0/`)**:

从真机 `/system/app/HandShaker/HandShaker.apk`(versionCode 201,versionName 1.2.0)提取并用
apktool 3.0.1 反编译,与 mac 端/proto/维护版三方结论**逐条确认**:

1. **type 37 枚举**:`original_smali_1.2.0/smali/com/smartisanos/smartfolder/a/a$dp.smali:597-612`
   `PHOTO_SYNC_REQUEST = 0x25(37)`、`SYNC_MONITOR_REQUEST = 0x27(39)`——与 mac 端、proto
   完全一致;**早前"老 APK 枚举不同"的推测被推翻**(真机 HEART_BEAT 响应实为 pc_id 带
   `hs-` 前缀/前置 39 复位导致,均已修复)。
2. **SyncManager 照片枚举**(`f/e.smali` 的 `c()`):URI=`content://smartisanos_gallery/files`
   (锤子相册 provider,`d/e.smali:31`),selection = `_data LIKE '/storage/emulated/0%' AND
   <bucket_id NOT IN (status=2)> AND media_type = 1`,排序 `date_added desc`,无 limit;
   projection 含 star/orientation/date_attribute_update → `ext_data` JSON。
3. **真机"当日新照片不在响应"根因(交叉验证确认)**:新 push 到非相册子目录的文件其
   `bucket_id = -837405474`,恰为 bucket 表 `status=2` 的排除项 → 1.2.0 查询按设计剔除;
   而 `DCIM/Camera` 下新文件 bucket 正常(如 -1739773001)→ **进入 37 响应**(实测
   `sync plan` added 第一项即新照片,共 41 张,过滤/排序正确)。锤子相册对非相册目录
   的 bucket 排除是**设计行为**,非协议缺陷。
4. **is_success**:真机实测响应尾部 `20 01`(field 4 is_success=true)显式返回与省略两种情况
   均出现;M6 判定(仅 `Some(false)` 拒绝)对两者兼容。
5. **pcId 语义**:1.2.0 的 `startSync` 用 `czVar.l()`(pcId)判 `is_first`(`aa.a` 判空),
   与 mac 端 getMacUUID 用法一致。

## 7. 已知限制

- 37/38/39 发送侧无抓包向量,依赖真机验收确认(§2)。
- checksum 由手机端计算并下发;host 只做字段比对,不自行计算(避免与手机算法
  版本不一致)。
- 冲突文件只报告不合并;台账损坏时停止并给出错误(不静默重建)。
- `sync run` 串行下载;并发(>1)未纳入(与 M5 的 `concurrency` 参数正交,可后续
  接入 `batch_transfer`)。
- 实时阶段依赖手机端正确推送 `FILE_CHANGE`;若手机端无监控行为(未拍照/无媒体
  变更),`watch` 只是静默监听。
