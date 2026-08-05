# 14 真实抓包验证报告（Real Capture Validation）

> 2026-08-02，真实设备验证。
>
> **环境**：macOS (Apple Silicon, Rosetta2) + Smartisan OD103（Android 7.1.1，root 不可用）
> + adb。手机已安装 `com.smartisanos.smartfolder.aoa`（HandShaker APK 1.2.0-r6）。
>
> **方法**：手动启动手机端 `ConnectionManagerService`（ADB_PORT=10086），`adb forward tcp:10086 tcp:10086`，
> 用自研 Python 客户端（`tools/capture/ssp_capture.py`）按 docs 假设直接与真机对话，**逐字节记录**
> 双向流量。未使用 GUI，无需 root，无第三方中间件。
>
> **结论先行**：docs 中关于 ADB 端口、上下行封帧、分块边界、RSA 签名、握手、下载/上传数据面的假设
> **全部得到真实验证**；`parseIoBuffer` 之谜完全解开（AES-256-CBC + 内嵌密钥）；另发现 3 个
> 文档未记载的线上行为（见 §7）。

## 1. ADB 端口（原未确认项 → 已确认）

- 手机端：`am startservice -n com.smartisanos.smartfolder.aoa/.service.ConnectionManagerService --ei ADB_PORT 10086`
  后，`/proc/net/tcp` 显示 `...0000:2766 ... 0A`（端口 10086=0x2766，**LISTEN**）。
- 即手机监听端口完全由 `ADB_PORT` intent extra 决定（`ConnectionManagerService.java:124-126`）。
- Mac 客户端转发目标为 `tcp:10086`（二进制字符串 `%@ tcp:(\d+) tcp:10086` 确认），另见 `tcp:19999`
  （与音频 HTTP 服务相关，未在本轮验证）。
- 宿主端口（Mac 侧 forward 的 host port）是动态的，本验证直接使用 10086。

## 2. 上行请求帧（已确认）

```
| sessionId : int32 BE (4B) | flag : uint8 (1B) | length : int32 BE (4B) | payload |
```

- 实测发出 `[0x80000001][0x00][0x000000D0][enckey 208B]` 被接受；`[sid][0x01][len][签名+proto]`
  被接受。
- 帧头 9 字节，payload 上限 0x400000 未触发（未测）。

## 3. 下行响应帧 + 分块边界（原未确认项 → 已确认）

每帧：`| sessionId : int32 BE (4B) | chunkLen : uint16 BE (2B) | data : chunkLen bytes |`

| 响应 | 总字节 | 帧数 | 帧大小 | 首帧前 8 字节 |
|---|---|---|---|---|
| 握手回复（base64 ok） | 175 | 1 | 183 | 8B 总长 175 |
| GET_DEVICE_INFO | 286 | 1 | 294 | 8B 总长 286 |
| HEART_BEAT | 12 | 1 | 20 | 8B 总长 12 |
| GET_DIR_FILES(maxdepth=2) | 40613 | **2** | 32761, 7860 | 8B 总长 `0x9EA5`=40613 |
| 照片库 | 648227 | **20** | 32761×19 + 14798 | 8B 总长 |
| **下载文件流** | 279438 | **9** | 32761×8 + 17350 | **无 8B 前缀** |

- **普通 protobuf 响应**：数据首 8 字节为 **大端总长度**，随后是 protobuf；超长拆多帧，单帧数据
  最大 **32761**（=32767 缓冲 − 6 字节帧头）。
- **文件下载流**：无 8B 前缀、无 protobuf 包裹，直接以帧流发文件字节，同样按 32761 分块。
- 下载数据与手机文件 **MD5 完全一致**（`b4faba61...`，279438B 零误差）。

## 4. 握手与 `parseIoBuffer` 之谜（原待验证 → 完全解开）

`parseIoBuffer` 不是恒等变换，而是 **AES-256-CBC 解密**，密钥内嵌于二进制：

- **密钥表**（48 字节，Mac `SmartFinderCore` @0x229f60 / Android `libsmartfolder.so` 内嵌）：
  - `IV   = 表[0:16]` = `2b 9e 34 d4 e1 d9 08 89 94 93 9e c4 e3 e9 60 c5`
  - `KEY  = 表[16:48]`（AES-256）= `28 e3 ee 32 b0 de 27 ef 6b c2 97 92 05 4e f9 73 9c e8 e8 7b b4 95 f2 ea 0d 72 d4 f4 f4 0b 3b de`
- **enckey 报文**（USB 密钥交换 / request01 字段9 通用）：
  ```
  [16B MD5(DER公钥)] [ AES-256-CBC( PKCS7( base64( DER公钥 ) ), key, iv ) ]
  ```
- 手机端 `decoder/b.java` 用 `parseIoBuffer` = AES-CBC **解密**出 base64 字符串 → 解码 DER → 重建公钥，
  并用 `MD5(DER) == 前16字节` 校验。
- **实测**：用上述参数构造密钥交换，手机回 `base64(RSA_enc("ok"))`，私钥解出明文 `"ok"` → 握手成功。
- **反向验证**：`ENCKEY_MODE=identity`（不加密直接发 base64）→ 手机解密失败/超时，握手不成立。
  → 确认必须用该 AES 变换。

> 反推来源：Mac `SFGenericDevice getRequest00Data` 用 `PEM_write_bio_RSAPublicKey` + `kysp_1`(密钥扩展) +
> `etcc_4`(CBC 加密)；Android `libsmartfolder.so` 的 `Java_..._parseIoBuffer` 用 `kysp_1` + `dtcc_5`(CBC 解密)
> + `dt_3`(AES 块，含标准 S-box、"AddRoundKey" 字符串)。

## 5. RSA 签名（flag=1）与握手流程（已确认）

- 签名：**SHA256withRSA**，128 字节，签**仅 protobuf 体**（不含帧头/flag/sid/签名自身）。
- 实测所有 flag=1 请求（GET_DEVICE_INFO / HEART_BEAT / GET_DIR_FILES / DOWNLOAD / UPLOAD / QUIT）均通过验签。
- **错误签名** → 手机回裸串 `"rsa verify failed"`（同样带 8B 长度前缀）。
- 握手回复 `base64(RSA/ECB/PKCS1Padding("ok"))`，私钥解出 `"ok"`。失败时回 `"failed"`/`"locked"`。

## 6. 文件操作（已确认）

- **GET_DIR_FILES**：maxdepth=1 → 176 文件/9377B；maxdepth=2 → 661 文件/40613B；响应含 `timecost`。
- **下载**：header(13) `{file, range{offset,length}, need_md5, ready=1}` → 帧流文件字节。
  - `range.length=0` → 返回全量；实际 header 回 `range{0, 文件长度}`。
- **上传**：header(15) → `SSPUploadFileResponseHeader{type=15(回显), file, ready=1}` →
  flag=3 分块（实测 1024B/块，150 块）→ `SSPUploadFileResponse{canceled=0, succeed=1}`。
  - 上传后文件与本地数据 **MD5 完全一致**。
- 手机根目录 = `/storage/emulated/0`（GET_DEVICE_INFO field 17）。

## 7. 新发现的线上行为（文档需补充）

1. **上传 `data_md5` 校验在线上未强制执行**：发送错误 data_md5（32 个 '0'）后上传仍返回
   `succeed=1`、无 `FILE_IO_MD5_CHECK_ERROR(7)`。
2. **flag=2 取消不会中断进行中的下载流**：下载中途发 flag=2，手机仍把剩余文件全部发完
   （实测 150000B 文件读 32761B 后取消，剩余 117239B 仍全部到达）。取消仅影响排队任务与上传会话。
3. **protobuf field 1（type）在等于默认值时可能被省略**：上传完成响应 `SSPUploadFileResponse` 在线上
   只出现 field 2/3/4，**没有 field 1**。解析器应以 field 1 为判别提示、但不得假定其必然存在。
   （同一响应的 `type` 字段缺失 = 默认 `GET_UPLOAD_FILE_RESPONSE`=18。）
4. **上传响应头 type 回显为 15**（请求类型），而非 proto 默认的 16/18（proto 此处注释混乱，
   以线上为准）。
5. **设备信息推送**：开启 `need_device_info_callback` 后，手机会主动推送 GET_DEVICE_INFO 消息，
   且推送使用**手机自身 sid 生成器**（首个 sid = 0x80000001，恰好与客户端首个 sid 相同）。

## 8. 复现方式

见 `tools/capture/README.md`。核心命令：

```bash
adb shell am startservice -n com.smartisanos.smartfolder.aoa/.service.ConnectionManagerService --ei ADB_PORT 10086
adb forward tcp:10086 tcp:10086
python3 tools/capture/ssp_capture.py          # 基础流程 + 全量字节日志
python3 tools/capture/ssp_multiframe.py       # 多帧分块 + 大文件下载
python3 tools/capture/ssp_upload.py           # 上传数据面
python3 tools/capture/ssp_errors.py           # MD5 异常 + 取消行为
```

环境变量：`HSPORT`（forward 端口）、`ADB_PORT`、`ENCKEY_MODE`（aes_cbc/identity）、`CAP_FILE`（日志路径）。

## 9. 对 Rust 实现的直接影响

1. `parseIoBuffer` 反转为 **AES-256-CBC(PKCS7)**：`enckey = MD5(DER) + AES-CBC-enc(base64(DER))`，
   密钥/IV 见 §4。**必须内嵌同一 48 字节密钥表**。
2. 下行解析：先读 6 字节帧头，按 `chunkLen` 收块；普通响应从首 8 字节取总长再组装；
   文件流按已声明的 range.length 收帧。
3. 上行发送：`[sid:4][flag:1][len:4]`；业务请求 flag=1、上传数据 flag=3、取消 flag=2、退出 flag=5。
4. 解析 protobuf 时勿强依赖 field 1。

---

# 10 局域网（WiFi / Bonjour）验证（2026-08-02 追加）

> 环境：Mac（macOS 27）与手机（地址已脱敏）连接同一局域网（同网段）。
> **注意**：macOS 15+ 的「本地网络（Local Network）」隐私权限会拦截非系统进程的局域网流量
> （`ping` 可用但 TCP/UDP 报 `No route to host`）。验证命令需以 **`sudo`** 运行绕过。

## 10.1 mDNS 发现（docs/02 全部确认）

`tools/capture/ssp_mdns.py`（纯 stdlib 实现，unicast 查询手机 5353 + v4/v6 多播）实测返回：

```json
{
  "instance": "handshaker_ssp_._handshaker_ssp._tcp.local",
  "port": 45656,
  "target": "fixture-phone.local",
  "ips": ["192.0.2.47", "2001:db8::47",
          "<global_ipv6>", "<global_ipv6>"],
  "txt": {}
}
```

确认要点：

- 服务类型 `_handshaker_ssp._tcp.`、实例名 `handshaker_ssp_`、已脱敏主机名 `fixture-phone.local`。
- **SRV 端口与手机实际监听端口一致**（实测 45656；期间端口从 55954 变为 45656，mDNS 记录实时跟随）。
- 手机 mdnsd（Android 7.1.1）对 **unicast 查询**（发往手机 IP:5353）会应答，也发 v4/v6 多播应答；
  本网络 WiFi 上**多播应答不可靠**（AP/IGMP），unicast 是稳定路径。
- 抓包确认（tcpdump 5353）：响应含 PTR（1 答）+ SRV/TXT/A/AAAA（6 附加）。
- 解析注意：DNS 名称压缩指针（`0xC0`）指向**整个报文**偏移，不能按 RDATA 独立解析；
  名字不带末尾点（匹配时勿加 `.`）。

## 10.2 WiFi 握手与信任（docs/04 全部确认）

### RESPONSE_01 实测字段

| field | 值 | 含义 |
|---|---|---|
| 1 | 32 | HANDSHAKE_RESPONSE_01 |
| 2 | `'201'` | apk version code |
| 3 | `'1.2.0'` | apk version name |
| 4 | 时间戳 | client_timestamp |
| 5 | `'1'` | client_smart_sync_protocol_version |
| 6 | `'2.1.0'` | client_min_host_version |
| 7 | `'<android_id>'` | device_uuid = android_id |
| 8 | `'<device_name>'` | device_name |
| 9 | `'<usb_serial>'` | usb_serial = ro.serialno |
| 10 | 1 | is_smartisan_device |
| 11 | 333 | client_min_host_version_code |

### 信任流程（三次实测）

1. **首次连接**：request02（仅 host_uuid）→ `RESPONSE_02{trust_type=1(TRUST_WAITING), 无 result}` + 手机弹窗。
2. **重置**：request02 带 `trust_type=6(TRUST_REMOVE)` → 手机清除该 hostUuid 的信任记录（同 request02
   可同时完成清除+弹窗）。
3. **信任确认**（用户点"信任"）→ `RESPONSE_02{trust_type=5(TRUST_ALWAYS), derived_key=256B(field5),
   result=base64(RSA_enc("ok"))}` → 私钥解密得 `"ok"` → 连接成立。
4. **重连（信任持久化）**：request02 携带上次收到的 derived_key → 手机 PBKDF2 重算比对通过 →
   `RESPONSE_02{TRUST_ALWAYS, result=…ok}`，**无弹窗**。
   - 手机在验证通过时会把 derivedKey **原样回显**在 field5（`g/j.java:95`）。
   - 无正确 derived_key 且信任记录已存在 → 回 `'failed'`（安全边界实测成立）。

### 其余

- 连接类型确认：WiFi 通道连接分类为 `WIFI`（ADB 为 `USB`），握手走 request01/02 protobuf 路径。
- WiFi 端口为 `ServerSocket(0)` 随机端口，**会周期性变化**（注册/注销循环），客户端必须以
  mDNS SRV 记录为准。
- 信任对话框按钮文案（zh）："不信任"/"信任一次"/"信任"（`view.a` 自定义 AlertDialog）。

## 10.3 局域网传输（与 ADB 通道完全一致）

经 WiFi 实测：GET_DEVICE_INFO（root=`/storage/emulated/0`、model=`odin`）、HEART_BEAT、
GET_DIR_FILES（178 项）、下载（210B）、上传（50000B，**MD5 与本地一致**
`04a2abbec73ae2f786397acf1909db64`）、QUIT。帧格式/签名/分块与 ADB 通道逐字节相同
（手机侧写端 `Connection$c` 对所有通道统一）。

## 10.4 复现

```bash
cd tools/capture
sudo python3 ssp_mdns.py 6 --ip <phone-ip>            # 发现（macOS 需 sudo 绕过本地网络权限）
sudo python3 ssp_wifi.py --ip <phone-ip> --port <port> --reset-trust   # 重置+信任
sudo python3 ssp_wifi.py --ip <phone-ip> --port <port>                 # derived_key 重连
sudo ./run_lan.sh <phone-ip>                           # 一键：发现+端口检测+完整验证
```

端口以 mDNS SRV 为准（`run_lan.sh` 自动从手机 /proc/net/tcp6 检测 uid 10062 的 LISTEN 端口）。
