# 13 验证状态与源码引用索引

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
| Bonjour 服务类型 | `_handshaker_ssp._tcp.`，服务名 `handshaker_ssp_`，随机端口 | ✅（两端字符串） |
| 上行帧头 | `[sid:int32BE][flag:u8][len:int32BE]`，len≤0x400000 | ✅（Android 读端源码） |
| 下行帧 | `[sid:int32BE][chunkLen:u16BE]`，分块 ≤32761B；普通响应数据首 8 字节为总长 | ✅（Android 写端源码） |
| flag 语义 | 0=握手 1=签名 2=取消 3=上传数据 4=信任取消 5=退出 | ✅ |
| 签名 | RSA-1024 SHA256withRSA，128B 签 protobuf 体，仅上行 | ✅ |
| 握手 01/02 流程 | USB 裸密钥交换 + WiFi/ADB protobuf 握手；RESPONSE_02 可多次 | ✅ |
| derived_key | PBKDF2WithHmacSHA1(hostUuid, 256B salt, 1500, 2048bit) | ✅（Android 源码） |
| enckey 格式 | `parseIoBuffer( base64(DER RSAPublicKey) )`，field8=MD5(公钥字节) | ⚠️ `parseIoBuffer` 为 native（`libsmartfolder.so`），需抓包确认其变换 |
| 心跳 | 默认 30s 超时、1s 检查；写操作刷新 | ✅ |
| ADB 命令 | `am startservice ...AdbForwardService --ei ADB_PORT <port>` + `adb forward tcp:<host> tcp:10086` | ⚠️ 命令串来自 Mac 二进制字符串，端口建议抓包确认 |
| USB AOA 过滤 | manufacturer=Smartisan model=HandShaker version=1 | ✅ |
| 下载数据面 | header(13) → 裸二进制帧流；type 14 未使用 | ✅ |
| 上传数据面 | header(15) → ready(16) → flag=3 块 → 完成(18)；type 17 未使用 | ✅ |
| 文件监控 | FileObserver mask 0xFC8 → SSPFileEventType 1..8 | ✅ |
| 媒体库查询/推送 | MediaStore + ContentObserver，1s 防抖，need_*_callback 开关 | ✅ |
| 缩略图 | JPEG 质量 86；视频 200x200；5s 超时 | ✅ |
| 照片同步 | 状态机 0/1/2；checksum=MD5(name_lower+len+base64(前100B)) | ✅ |
| 锁屏 | USB 通道回裸串 `"locked"` | ✅ |
| 版本兼容 | type1 min 2.1.0/333；type2 min 2.5.0/12 | ✅ |
| Mac 文件同步错误码 | filesync 域 -1000/-1030/-1040/-1110/-1140 | ⚠️ 具体文案未还原，可忽略（错误码语义按域区分） |

## 13.3 建议的抓包验证项

1. 一次完整 WiFi 握手（request01/02 → response01/02）字节级比对 enckey 变换。
2. 一个 10MB 文件的下载/上传帧流（确认分块大小与总长前缀边界跨越）。
3. 取消（flag=2）与 CANCEL_REQUEST(36) 的字节序列。
4. ADB 通道实际端口（10086 vs 其他）与 AdbForwardService intent extra。
5. 心跳实际间隔与超时（30s 或协商值）。
6. `SSPHandShakeResponse02.result` 各取值实际出现场景。

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
