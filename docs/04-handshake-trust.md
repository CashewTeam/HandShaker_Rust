# 04 握手与信任（Handshake & Trust）

握手阶段的消息使用 **flag=0**（未签名）。业务阶段使用 **flag=1**（RSA-1024 SHA256 签名）。
握手在每条连接上执行一次，成功后进入业务阶段。

## 4.1 总体流程

```
传输层建立
  │
  ▼
[USB 通道] 交换 RSA 公钥（明文裸包）→ 手机回 RSA 加密 "ok" → 直接进入业务阶段
  │
[WiFi/ADB 通道]
  HANDSHAKE_REQUEST_01(31) ──► HANDSHAKE_RESPONSE_01(32)   （交换 uuid/名称/版本/公钥）
  HANDSHAKE_REQUEST_02(33) ──► HANDSHAKE_RESPONSE_02(34)   （校验 derived_key / 信任协商，可多次）
                                 └── 最后响应必含 result 字段
  │
  ▼ 业务阶段（flag=1）：GET_DEVICE_INFO_REQUEST 等
```

## 4.2 USB 通道的公钥交换（flag=0 裸包）

USB 下握手消息体为自定义裸格式（非 protobuf），`decoder/a.java:43-55` 走
`g/j.java:59-71`（Transfer USB 路径）：

- **请求体**：`[16B MD5] [ AES-256-CBC 加密的 base64(DER 公钥) ]`
  - 前 16 字节 = 公钥字节的 **MD5 指纹**。
  - 剩余部分：`parseIoBuffer`（native，`decoder/C.java`）即 **AES-256-CBC 解密**，剥掉加密后得到
    UTF-8 的 base64 字符串 → Base64 解码 → **DER 编码的 RSA 公钥**（ASN.1 `SEQUENCE{ modulus, exponent }`，
    解析见 `decoder/b.java` + `org/a/a/a/a.java`）。
  - 校验：`MD5(DER公钥字节) == 前16字节`。
- **响应**：`base64( RSA/ECB/PKCS1Padding( "ok" ) )`（用对方公钥加密，`g/j.java:36-57`）。
  Mac 解密成功拿到明文 "ok" 才算握手成功（`SFGenericDevice checkResult:`）。

> ✅ **已抓包确认（2026-08）**：`parseIoBuffer` = **AES-256-CBC(PKCS7) 解密**，密钥表内嵌于二进制
> （Mac `SmartFinderCore`@0x229f60 / Android `libsmartfolder.so`）。完整构造见
> [14-capture-validation](14-capture-validation.md) §4：enckey 报文 =
> `MD5(DER) + AES-256-CBC( PKCS7( base64( DER ) ), key=表[16:48], iv=表[0:16] )`。
> 用该构造在真机上握手成功；不加密（identity）则失败。

## 4.3 WiFi / ADB 通道握手

### 第一轮：HANDSHAKE_REQUEST_01（type=31）→ RESPONSE_01（type=32）

Android 处理 `g/j.java:75-83`。请求字段（`SSPHandShakeRequest01`）：

| field | 含义 | Mac 端填充 |
|---|---|---|
| 1 | type=31 | - |
| 2 | `host_uuid` | Mac UUID（如 MAC 地址） |
| 3 | `host_name` | 电脑名称 |
| 4 | `host_timestamp` | 时间戳 |
| 5 | `host_smart_sync_protocol_version` | 协议版本 |
| 6 | `host_app_version` | 软件版本 |
| 7 | `host_min_client_version` | 需要的最低 APK 版本 |
| 8 | `md5` | **ENCKEY 的 MD5**（对 field 9 解码出的公钥字节取 MD5） |
| 9 | `enckey` | `parseIoBuffer( base64(DER 公钥) )` 包装 |
| 10 | `host_model` | 机型（如 iMac12,2） |
| 11 | `heartbeat_timeout_second` | 心跳超时上限（秒） |

Android 响应 `RESPONSE_01`（`g/j.java:79`）：

| field | 含义 | Android 填充 |
|---|---|---|
| 1 | type=32 | - |
| 2 | `apk_version` | versionCode 字符串 |
| 3 | `apk_version_name` | versionName |
| 4 | `client_timestamp` | 秒级时间戳 |
| 5 | `client_smart_sync_protocol_version` | 常量 `"1"` |
| 6 | `client_min_host_version` | `"2.1.0"`（host type 1） |
| 7 | `device_uuid` | android_id |
| 8 | `device_name` | 设备名 |
| 9 | `usb_serial` | `ro.serialno` |
| 10 | `is_smartisan_device` | 是否锤子 ROM |
| 11 | `client_min_host_version_code` | 333（type 1） |

RSA 公钥解析（`decoder/b.java:26-54`）：`md5` == MD5(公钥字节) 才接受，用 `RSAPublicKeySpec` 重建。

### 第二轮：HANDSHAKE_REQUEST_02（type=33）→ RESPONSE_02（type=34）

Android 处理 `g/j.java:84-108`。请求字段：

| field | 含义 |
|---|---|
| 1 | type=33 |
| 2 | `host_uuid` |
| 3 | `derived_key`（bytes） |
| 4 | `trust_type`（SSPHandShakeTrustType） |

响应字段（`SSPHandShakeResponse02`）：

| field | 含义 |
|---|---|
| 1 | type=34 |
| 2 | `trust_type` |
| 3 | `device_uuid` |
| 4 | `device_name` |
| 5 | `derived_key`（bytes） |
| 6 | `result`：`'failed'` / `'locked'` / `'needauth'` / `base64(RSA_enc('ok'))` |

`result` 语义（来自 `.proto` 注释）：
- `'failed'`：握手失败。
- `'locked'`：手机锁屏。
- `'needauth'`：手机处于授权确认窗口。
- `base64(RSA_enc('ok'))`：成功，解密得明文 `'ok'` 才算握手成功。

**握手响应 02 可能返回多次**，最后一次必须包含 `result`。

### 信任校验逻辑（Android `g/j.java:84-108`）

```
解析 request02
  ├─ trust_type == TRUST_REMOVE(6)：清除该 hostUuid 的信任数据（SharedPreferences）
  ├─ 若本地已有该 hostUuid 的信任记录：
  │     re-derive = PBKDF2(hostUuid, 存储的 salt, 1500, 2048bit)
  │     derived_key 一致 → 回 RESPONSE_02(TRUST_ALWAYS, ok) → 连接成立
  │     derived_key 不一致 → 回 RESPONSE_02(TRUST_ALWAYS, 'failed')
  └─ 首次（无记录 / 宿主没发 derived_key）：
        回 RESPONSE_02(TRUST_WAITING, deviceUuid, deviceName) → 弹信任对话框
        用户选择后发 TrustResponseEvent → 重新回 RESPONSE_02
```

> ✅ **抓包实测（2026-08，WiFi 通道，见 [14](14-capture-validation.md) §10）**：
> - 首次连接：request02（仅 host_uuid）→ `RESPONSE_02{TRUST_WAITING}` + 弹窗。
> - `TRUST_REMOVE`（field4=6）可清除手机端信任记录（同一条 request02 即完成清除+触发弹窗）。
> - 用户确认后：`RESPONSE_02{TRUST_ALWAYS, derived_key=256B(field5), result=base64(RSA_enc('ok'))}`。
> - **重连**：request02 携带上次收到的 derived_key → 手机 PBKDF2 比对通过 → 直接
>   `RESPONSE_02{TRUST_ALWAYS, ok}`，**无弹窗**；且手机在 field5 原样回显 derived_key
>   （`g/j.java:95`）。无正确 derived_key 且信任记录存在 → 回 `'failed'`。

### derived_key 计算

不是 HKDF，而是 **PBKDF2**（`h/z.java:9-11`）：

```
derivedKey = PBKDF2WithHmacSHA1(
    password = hostUuid (char[]),
    salt     = 256 字节随机盐（SecureRandom）,
    iterations = 1500,
    dkLen    = 2048 bit (256 字节)
)
```

信任持久化（`g/j.java:130-137`）：SharedPreferences 以 hostUuid 为文件名，存
`hostUuid`(hex)、`salt`(hex)、`derivedKey`(hex)、`trustType`。

## 4.4 信任类型（SSPHandShakeTrustType）

| 值 | 名称 | 语义 |
|---|---|---|
| 1 | `TRUST_WAITING` | 等待用户回复信任对话框 |
| 2 | `TRUST_UNKNOW` | 信任状态不存在 |
| 3 | `TRUST_NO` | 不信任 |
| 4 | `TRUST_ONCE` | 信任一次（不持久化） |
| 5 | `TRUST_ALWAYS` | 总是信任（持久化 + 交换 derived_key） |
| 6 | `TRUST_REMOVE` | 信任失效，清理信任条目 |

用户对话框：`h/l.java` + 按钮 `h/m|n|o.java`（不信任 / 信任一次 / 始终信任）。

## 4.5 锁屏处理

Android `decoder/a.java:44-53`：**USB 通道**下若 `KeyguardManager.isKeyguardLocked()`：

1. 缓存整个握手包（`C0099a(sid, body)`）。
2. 直接回原始字符串 `"locked"`（裸字节）。
3. 订阅 `WakeLockEvent`；用户解锁（`USER_PRESENT` 广播，`h/f.java:437-453`）后重放缓存的握手包。

WiFi 通道不拦截锁屏。

## 4.6 版本兼容检查

- Android 收到 `GET_DEVICE_INFO_REQUEST`（type=2）时（`decoder/a.java:66-84`）：
  - 比较本机 APK versionName 与请求字段 `host_min_client_version`，过旧则提示更新并断开。
- UI 校验（`MainActivity.java:331-356`）：宿主 versionCode 是否达到对应类型的最低值；
  老 Mac（无 versionCode）用字符串区间 `[2.1.0, 2.5.0)` 判断。
- 对应关系（`h/d.java:338-358`）：

| host_type | min host version 字符串 | min host versionCode |
|---|---|---|
| 1 (Mac HandShaker) | `2.1.0` | 333 |
| 2 (SmartFinder) | `2.5.0` | 12 |

## 4.7 握手相关源码索引

- Android 握手入口：`decoder/a.java:43-55`；密钥解析 `decoder/b.java`；native `decoder/C.java`
- Android 握手执行：`g/j.java:36-108`（USB 59-71 / WiFi 72-108）；心跳 `g/j.java:264-266`
- PBKDF2：`h/z.java`；MD5：`h/u.java`
- Mac 侧：`SFGenericDevice getRequest00Data / getRequest01WithError: / getRequest02WithError: /
  getSignatureForData: / checkResult:`；`SFDeviceTrustStore` / `SFDeviceTrustRecord`
  （持久化 trust 记录：device_uuid, device_name, derived_key, apk_version,
  client_smart_sync_protocol_version, client_min_host_version, last_connection, connection_count, trust_type）
