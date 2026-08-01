# 05 线路封帧（Wire Framing）

> 重要：**上行（Host→Phone）与下行（Phone→Host）的帧格式不对称**。实现互通时必须分别处理。

## 5.1 上行请求帧（Host → Phone）

Android 读端 `service/i.java:173-192`（`SspReader`，TCP 与 AOA 共用）：

```
| sessionId : int32 大端 (4B) | flag : uint8 (1B) | length : int32 大端 (4B) | payload : length 字节 |
```

- `sessionId`：请求会话号（Mac 生成，用于关联响应/数据流，见 §5.5）。
- `flag`：消息类型，见 §5.3。
- `length`：payload 字节数。合法范围 `0 ..= 4,194,304 (0x400000)`，超限抛
  `InvalidSSPPacketException`（携带 sid+flag）并断连（`service/i.java:183,191`）。
- payload 读取分块：首批最多 16375 字节，之后每批 16384 字节（`service/i.java:194-210`）。

## 5.2 下行响应/数据帧（Phone → Host）

Android 写出端 `g/a.java`（`Connection$c`，OutputStream 包装器）。每个响应是**帧流**：

```
每帧： | sessionId : int32 大端 (4B) | chunkLen : uint16 大端 (2B) | data : chunkLen 字节 |
```

- 分块缓冲 32767 字节，数据最大 `32767 - 6 = 32761` 字节（`g/a.java:200,216`）。
- 数据写入会刷新心跳计时（`g/a.java:229-234`）。

### 5.2.1 普通 protobuf 响应

`a(int sessionId, byte[] protobufBytes)`（`g/a.java:163-169`）：

1. 先写 **8 字节大端总长度**（`putLong(length)`），再拼 protobuf 字节。
2. 经分块器写出（`a(sid, stream, len)`）。8 字节前缀可能跨越帧边界。

即手机发出的第一个数据包形如：

```
| sid:4 | chunkLen:2 | 8字节总长 | protobuf... | ...（后续帧）|
```

Mac 端应：读帧头 `[sid:4][chunkLen:2]` → 积累数据 → 从数据首 8 字节取出总长 → 继续读帧直到
累计达到总长 → 解析 protobuf。

### 5.2.2 文件下载数据流

`a(int sessionId, File, start, len, isExternal)`（`g/a.java:171-180`）：

- `FileInputStream.skip(start)` 跳过起始字节，直接从文件读出 `len` 字节。
- **没有 8 字节总长前缀**，也没有 protobuf 包裹：直接以 `[sid:4][chunkLen:2]` 帧流输出文件字节。
- Mac 依据 `SSPDownloadFileResponseHeader.range` 得知应接收的总字节数。
- `isExternal=true` 时注册“外发文件”会话，支持 `stopWriteFile`（`g/a.java:244-250`）在取消/SD卡拔出时中止。

## 5.3 flag 语义（上行方向）

`g/h.java:312-344`（SspExecutorManager 按 flag 分派）：

| flag | 语义 | payload | Android 处理 |
|---|---|---|---|
| 0 | 握手/裸消息（NORMAL） | 裸字节（USB 密钥交换）或握手 protobuf | `decoder.a(sid,0,body)` |
| 1 | 已签名业务请求（DATA） | `[128B 签名][protobuf]` | 验签后 `decoder.a(sid,1,body)` |
| 2 | 取消 | int32 大端 sessionId | 取消该 sid 的排队任务 / 上传 |
| 3 | 上传文件数据块 | 原始文件字节 | `SspFileTransferTask` 写盘 |
| 4 | 取消信任 | - | 发 `TrustCancelEvent` |
| 5 | 退出 | - | 静默关连接（QUIT_FLAG） |

> protobuf 枚举 `SSPFileType` 里只有 `NORMAL=0 / DATA=1`，但实际协议 flag 用到 0-5。

## 5.4 签名消息格式（flag=1）

`decoder/a.java:57-62,187-206`：

```
payload = [ 128 字节 RSA-1024 SHA256withRSA 签名 ][ protobuf 体 ]
```

- **签名对象 = 仅 protobuf 体字节**（不含帧头、sessionId、flag、签名自身）。
- 验签：`Signature.getInstance("SHA256withRSA")`，`initVerify(hostPublicKey)`，
  `update(protobuf体)`，`verify(前128字节)`。
- 失败：回裸字符串 `"rsa verify failed"`（`decoder/a.java:183`）。
- **仅上行方向签名**；下行（手机→Mac）不签名。

## 5.5 sessionId

- 由**客户端（Mac）**生成。Android 服务器把收到的 sessionId 原样回填到响应帧头。
- Android 服务端内部有一个递增计数器 `c`（初值 `-2147483647` = `0x80000001`），用于主动发
  QUIT/CANCEL 时分配 sid（`a/a.java:49,74-78`），单调递增。
- Mac 端 `SSPRequestOperation` 持 `sessionId`、`data`、`lenData`、`timeout`，按会话排队
  （`SSPManager._sessionQueue/_sessionDict`）。
- 上传会话以 `sid` 为键（`g/h.java:36` 的 `ConcurrentHashMap<Integer, UploadSession>`）。

## 5.6 取消（Cancel）

- 客户端（Mac）发 **flag=2**，payload = int32 大端 sessionId（`g/h.java:319-328`）。
- 服务器取消对应 sid 的任务（Future.cancel），并清理上传会话。
- 服务器主动取消：发 `SSPCancelRequest`（type=36）`{type, session_id, error_code}`（`d/c.java:682-699`）：
  - 下载超时/异常时。
  - `error_code`：`ERROR_CODE_UNKNOWN=1` / `ERROR_CODE_SDCARD_REMOVED=2`。
- Mac 侧 `SFGenericDeviceIOProtocol cancelRequest:withOldSessionId:`。

## 5.7 下载流程（数据面）

```
Mac:   GET_DOWNLOAD_FILE_REQUEST(12) {type, file, range{offset,length}, need_md5, gzip, is_sync}
Phone: GET_DOWNLOAD_FILE_RESPONSE_HEADER(13) {type, file, range(实际区间), need_md5, data_md5, ready, error_code}
       然后直接以 [sid][chunkLen] 帧流发送文件字节（无 protobuf 包裹）
```

- `range.length=0` 表示全量；`length > 剩余` 则返回剩余字节；服务器按文件实际大小裁剪区间
  （`d/c.java:620-623`）。
- `GET_DOWNLOAD_FILE_RESPONSE_BODY(14)` **未在线上使用**（proto 中注释掉），数据走 sessionId + binary。

## 5.8 上传流程（数据面）

```
Mac:   GET_UPLOAD_FILE_REQUEST_HEADER(15) {type, file(含大小), data_md5, gzip, is_sync}   ← flag=1 签名请求
Phone: GET_UPLOAD_FILE_RESPONSE_HEADER(16) {type, file, ready, error_code}
Mac:   以 flag=3 发数据块（每帧 [sid:4][flag=3][len:4][原始文件字节]，len≤4MB）
Phone: 收齐后 GET_UPLOAD_FILE_RESPONSE(18) {type, file, canceled, succeed, error_code}；并刷新媒体库
```

- 服务器收到 header 后校验空间/路径，`ready=true` 才接受（`d/c.java:334-409`）。
- 上传会话记录 `docUri, totalSize, modifyTime`；累计 `uploaded == total` 后 flush+sync 并回 type 18
  （`g/h.java:155-215`）。
- 中途失败：删除半成品，回 `CANCEL_REQUEST`（type 36）。
- `GET_UPLOAD_FILE_REQUEST_BODY(17)` **未在线上使用**（proto 注释掉）。

## 5.9 与 Mac 实现的对应

- Mac 发送请求头 `[sid:4][flag:1][len:4]`：`SFGenericDevice sendData:withSessionId:withFlag:`
  （flag 由 `SSPRequestOperation` 决定；握手用 `syncSendRequestData:withMSTimeout:error:`）。
- Mac 读取响应：`SFGenericDevice syncReadResponseDataWithMSTimeout:error:` +
  `SSPRequestOperation` 解析 `[sid:4][chunkLen:2]` 帧流并重组 8 字节长度前缀（`lenData`）。
- 大文件：`sendFileData:withSessionId:error:` / `sendRequestData:withSessionId:error:`。
- `SFWifiSocket receive:amount:withMSTimeout:` / `syncWriteData:withMSTimeout:` 提供底层读写。

## 5.10 Rust 实现清单

1. 发送：`writeInt32BE(sid) + writeU8(flag) + writeInt32BE(len) + payload`
2. 接收（模拟手机时的写）：
   - 普通响应：`writeInt32BE(sid) + writeU16BE(chunkLen)`，分块；首 8 字节为总长。
   - 文件流：同上但无总长前缀。
3. 校验：上行 len ∈ [0, 0x400000]；下行 chunkLen ∈ [0, 32761]。
4. 签名：RSA-1024 / SHA256withRSA，签 protobuf 体。
5. 按 flag 分发 0-5。

## 5.11 抓包实测补充（2026-08，见 [14](14-capture-validation.md)）

- 下行实测分块：40613B 响应 → 2 帧 `[32761, 7860]`；279438B 文件 → 9 帧 `[32761×8, 17350]`；
  648227B 照片库 → 20 帧。单帧数据 **恒 ≤32761**，首帧数据前 8 字节即大端总长（`0x9EA5=40613` 实测）。
- **protobuf field 1（type）在等于默认值时会被省略**：`SSPUploadFileResponse` 线上只有 field 2/3/4，
  没有 field 1。解析器不得假定 field 1 必然存在。
- **flag=2 取消不中断进行中的下载流**：下载中途取消，手机仍发完剩余文件字节。
- 上传响应头 type 线上回显为 **15**（请求类型），而非 proto 默认值。
