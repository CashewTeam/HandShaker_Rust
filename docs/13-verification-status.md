# 13 验证状态与源码引用索引

> **重大更新（2026-08-02）**：已在真实设备（Smartisan OD103 / Android 7.1.1）上完成抓包验证，
> 关键未确认项全部落地。完整验证报告见 **[14-capture-validation](14-capture-validation.md)**。
> 验证工具与复现步骤见 `tools/capture/`。

本文档记录各结论的验证级别，以及核心源码引用，便于后续抓包核验与实现回归。

## 13.1 验证级别说明

- ✅ **已确认**：两端源码或权威 proto 明确、且已双向交叉核对。
- ⚠️ **需抓包确认**：单端源码推断、或依赖 native/未公开细节，建议用真实设备抓包（Wireshark 或
  `adb` 抓 SSP 端口）验证。
- ❓ **未知/推断**：未能从现有材料完全确定。

## 13.2 结论清单

| 主题 | 结论 | 级别 |
|---|---|---|
| 协议名 / 序列化 | SmartSync Protocol（SSP），protobuf proto2，package `smartsync` | ✅ |
| 命令类型枚举 | `SSPRequestType` 1..41，APK `.proto` + Mac 头 + 反编译三方一致 | ✅ |
| 66 个消息字段编号 | 与 `SmartSyncProtocol.proto` 逐字段一致 | ✅ |
| Bonjour 服务类型 | `_handshaker_ssp._tcp.`，服务名 `handshaker_ssp_`，随机端口 | ✅ **抓包实测**（PTR/SRV/TXT/A/AAAA 全记录） |
| Bonjour SRV 端口 | 与手机实际监听端口一致（实测 45656，期间 55954→45656 实时跟随） | ✅ **抓包实测** |
| 上行帧头 | `[sid:int32BE][flag:u8][len:int32BE]`，len≤0x400000 | ✅ **抓包实测** |
| 下行帧 | `[sid:int32BE][chunkLen:u16BE]`，分块 ≤32761B；普通响应数据首 8 字节为总长 | ✅ **抓包实测**（多帧：2/9/20 帧） |
| 文件下载帧流 | 无 8B 前缀、无 protobuf，按 32761 分块 | ✅ **抓包实测**（279438B / 9 帧 / MD5 一致） |
| flag 语义 | 0=握手 1=签名 2=取消 3=上传数据 4=信任取消 5=退出 | ✅（0/1/2/3/5 实测） |
| 签名 | RSA-1024 SHA256withRSA，128B 签 protobuf 体，仅上行 | ✅ **抓包实测**（错误签名回 `"rsa verify failed"`） |
| 握手 01/02 流程 | USB 裸密钥交换 + WiFi/ADB protobuf 握手；RESPONSE_02 可多次 | ✅ **WiFi 实测**（request01/02 + 信任弹窗 + result） |
| derived_key | PBKDF2WithHmacSHA1(hostUuid, 256B salt, 1500, 2048bit) | ✅（Android 源码） |
| 信任持久化 | 重连携带 derived_key → 无弹窗直接 TRUST_ALWAYS+ok；错误 key → `'failed'` | ✅ **WiFi 实测** |
| enckey 格式 / parseIoBuffer | `MD5(DER) + AES-256-CBC(PKCS7)(base64(DER))`，密钥表见 [14](14-capture-validation.md) §4 | ✅ **完全解开 + 实测握手成功** |
| 心跳 | 默认 30s 超时、1s 检查；写操作刷新 | ✅ |
| ADB 端口 | 手机监听 = `ADB_PORT` extra（实测 10086）；Mac 转发目标 `tcp:10086`（另 `tcp:19999` 音频） | ✅ **实测** |
| USB AOA 过滤 | manufacturer=Smartisan model=HandShaker version=1 | ✅ |
| 下载数据面 | header(13) → 裸二进制帧流；type 14 未使用 | ✅ **实测** |
| 上传数据面 | header(15) → ready(16) → flag=3 块 → 完成(18)；type 17 未使用 | ✅ **实测** |
| 上传 data_md5 校验 | **线上未强制执行**（错误 md5 仍 succeed=1） | ✅ **实测（新发现）** |
| 取消（flag=2） | 不中断进行中的下载流（会发完剩余数据） | ✅ **实测（新发现）** |
| protobuf field1 | 等于默认值时可能被省略（上传完成响应无 field1） | ✅ **实测（新发现）** |
| WiFi 通道传输 | 帧格式/签名/分块与 ADB 完全一致；上传 MD5 一致 | ✅ **WiFi 实测** |
| 文件监控 | FileObserver mask 0xFC8 → SSPFileEventType 1..8 | ✅ |
| 媒体库查询/推送 | MediaStore + ContentObserver，1s 防抖，need_*_callback 开关；推送用手机 sid 生成器 | ✅ |
| 缩略图 | JPEG 质量 86；视频 200x200；5s 超时 | ✅ |
| 照片同步 | 状态机 0/1/2；checksum=MD5(name_lower+len+base64(前100B)) | ✅ |
| 锁屏 | USB 通道回裸串 `"locked"` | ✅ |
| 版本兼容 | type1 min 2.1.0/333；type2 min 2.5.0/12 | ✅ |
| Mac 文件同步错误码 | filesync 域 -1000/-1030/-1040/-1110/-1140 | ⚠️ 具体文案未还原，可忽略（错误码语义按域区分） |

## 13.3 建议的抓包验证项

> 已于 2026-08-02 完成大部分验证（见 [14](14-capture-validation.md)），含**局域网**部分。剩余可选验证：

1. ~~一次完整 WiFi 握手字节级比对 enckey 变换~~ → **已完成**（enckey = AES-256-CBC，见 §4）。
2. ~~一个 10MB 文件的下载/上传帧流~~ → **已完成**（下载 279KB/9 帧、照片库 648KB/20 帧）。
3. ~~取消（flag=2）与 CANCEL_REQUEST(36) 的字节序列~~ → **已完成**（flag=2 不中断下载流）。
4. ~~ADB 通道实际端口与 intent extra~~ → **已完成**（ADB_PORT=10086，`/proc/net/tcp` 确认）。
5. ~~mDNS/Bonjour 发现~~ → **已完成**（PTR/SRV/TXT/A/AAAA 全记录，SRV 端口与监听一致）。
6. ~~WiFi 握手 + 信任流程~~ → **已完成**（request01/02、TRUST_REMOVE、信任弹窗、derived_key 重连）。
7. 心跳实际间隔与超时（建议挂 2 分钟观察心跳频率）。
8. `SSPHandShakeResponse02.result` 的 `'locked'/'needauth'` 取值（需锁屏/并发场景触发）。
9. 锁屏场景回 `"locked"` 的字节序列。

## 13.4 核心源码引用索引

### Android（`Android_jadx/sources/`）

| 文件 | 内容 |
|---|---|
| `com/smartisanos/smartfolder/a/a.java` | 全部 protobuf 消息/枚举反编译（17166 行） |
| `.../aoa/decoder/a.java` | 帧分发、验签、命令→Handler 映射、锁屏 |
| `.../aoa/decoder/b.java` | RSA 公钥解析（MD5 指纹 + DER） |
| `.../aoa/decoder/C.java` | native `parseIoBuffer` |
| `.../aoa/g/a.java` | Connection + 写出端 `Connection$c`（下行分块帧） |
| `.../aoa/g/h.java` | SspExecutorManager（flag 分派、上传会话、取消） |
| `.../aoa/g/j.java` | Transfer（握手、心跳、设备信息响应） |
| `.../aoa/d/c.java` | FileProcessor（文件操作、下载/上传、取消、UPDATE_FILE_INFO） |
| `.../aoa/d/e.java` | 媒体库查询（视频/音频） |
| `.../aoa/d/h.java` | ThumbnailHandler |
| `.../aoa/f/e.java` | SyncManager（照片同步状态机） |
| `.../aoa/h/v.java` | MediaDataProvider（媒体库变更推送） |
| `.../aoa/h/r.java` | FileStorageChangeObserver（FileObserver 0xFC8） |
| `.../aoa/h/z.java` | PBKDF2 |
| `.../aoa/service/i.java` | TcpSocketManager（上行 9 字节头读取） |
| `.../aoa/service/m.java` | WifiConnectionManager（NsdManager 广播） |
| `.../aoa/service/ConnectionManagerService.java` | 连接服务（ADB_PORT extra） |
| `.../aoa/service/a.java` | AccessoryManager（AOA 打开） |
| `resources/main/proto/SmartSyncProtocol.proto` | **权威协议定义** |

### macOS（`macos/`）

| 文件 | 内容 |
|---|---|
| `decompiled/headers/SmartFinderCore.h` | 协议核心类头（SSP、SFGenericDevice、SFWifiDeviceManager、SFADBManager…） |
| `decompiled/headers/SmartFinderNetwork.h` | 账号/云 HTTP 层（非本协议） |
| `interfaces/Protocols/SmartFinderCore_Protocols.h` | `SFGenericDeviceIOProtocol` 等接口 |
| `HandShaker_Mac.m` | 主程序反编译（同步流程、错误码、SFSynchManager 等） |
| `analysis/core.json` | SmartFinderCore 方法索引 |
| `HandShaker.app/Contents/Frameworks/SmartFinderCore.framework/...` | 二进制字符串（Bonjour 类型、ADB 命令、音频 URL） |

### 逆向工具

- `macos/tools/objcdump.py` — ObjC 方法 dump 工具
- `macos/tools/patch_sfbutton.py`、`patch_crashhandler.py` — 兼容性修复（与协议无关）

## 13.5 实现前的最后确认

请优先完成 13.3 的抓包验证，特别是 enckey 的 native 变换与下行分块帧边界；
这两点对 Rust 端实现「与原版互通」影响最大。
