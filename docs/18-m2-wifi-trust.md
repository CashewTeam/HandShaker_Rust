# 18 M2：WiFi 发现、连接与持久化信任（设计与实现记录）

> 状态基线：2026-08，Cargo package `handshaker_rust 0.1.5`。
> 本文记录 M2 的实现范围、协议依据、公开 API 与验证结果；行为细节以源码与自动化测试为准。

## 1. 目标与范围

M2 完成 WiFi（LAN）通道的完整连接路径：

- mDNS 发现 `_handshaker_ssp._tcp.local.`，解析 SRV/TXT/A/AAAA；
- `--wifi IP:PORT` 直连（端口以最新 mDNS SRV 为准）；
- WiFi 两轮握手 `HANDSHAKE_REQUEST_01 → RESPONSE_01`、`REQUEST_02 → RESPONSE_02`（可多轮）；
- `TRUST_ALWAYS` 的 derived_key 持久化（按 device_uuid），重连免弹窗；
- `TRUST_REMOVE` 清除手机端信任；
- 业务 API 与 ADB 通道完全复用（设备、文件、传输、剪贴板）。

不在本里程碑范围：二维码直连、USB AOA、多子网发现、主机地址变化自动恢复。

## 2. 协议依据（docs/02、04、14）

- 服务类型 `_handshaker_ssp._tcp.`，实例名 `handshaker_ssp_`，SRV 端口动态变化，
  必须以 mDNS 实时解析结果为准（docs/02 §2.5、docs/14 §10.1）。
- 握手消息 flag=0（未签名）；REQ_01 的 field8=MD5、field9=enckey（仅 AES 部分，
  不含 MD5 前缀，与 ADB 裸交换不同）；REQ_02 携带 host_uuid、derived_key（重连）、
  trust_type（TRUST_REMOVE）。
- RESPONSE_02 可返回多次，最后一次必须包含 `result`：`failed`/`locked`/`needauth`/
  `base64(RSA_enc("ok"))`；`TRUST_WAITING` 表示弹窗等待用户操作。
- 手机在 TRUST_ALWAYS 时回显 256 字节 derived_key（field5），主机保存后重连携带即可免弹窗。
- 抓包实测字段（docs/14 §10.2）：RESPONSE_01 的 apk_version=`201`、apk_version_name=`1.2.0`、
  client_min_host_version=`2.1.0`、client_min_host_version_code=`333`。

## 3. 实现

### 3.1 依赖

- `mdns-sd 0.20`：mDNS 浏览与 SRV/TXT/A/AAAA 解析（多播为主）。

> 说明：derived_key 由手机生成、主机只保存与回传，主机侧 PBKDF2 派生/校验路径暂未使用，
> 因此未引入 `pbkdf2`/`sha1` 依赖；若后续需要（如校验或生成），按需添加。

### 3.2 新增模块与公开 API

| 模块/API | 说明 |
|---|---|
| `src/discovery.rs` | `discover_wifi_devices`：浏览 `_handshaker_ssp._tcp.local.`，按 SRV 主机名去重（实例名 `handshaker_ssp_` 固定，同一主机保留最新 SRV 端口），IPv4 优先排序 |
| `src/transport/wifi.rs` | `WifiConnector`：TCP 直连 + 超时，`TransportCleanup::None` |
| `src/protocol/wifi_handshake.rs` | `WifiTrustHandshake`：REQUEST_01/02 多轮协商、derived_key 复用、TRUST_REMOVE、120s 信任等待 |
| `HandShakerClient::discover_wifi_devices` | 公开发现 API |
| `HandShakerClient::list_trusted_devices` | 公开信任记录（不含 derived_key） |
| `HandShakerClient::remove_trusted_device` | 删除本地信任记录 |
| `HandShakerClient::reset_wifi_trust` | 连接 + TRUST_REMOVE 后关闭（清手机端记录） |
| `ConnectionTarget::Wifi { address }` | 新增连接目标变体 |
| `domain::WifiDevice`、`domain::TrustRecordInfo` | 公开领域类型 |

### 3.3 信任持久化

- `state.json` 的 `trust` map 按 device_uuid 索引，`TrustRecord { device_name, derived_key, updated_at }`；
  `derived_key` 为 base64 编码的 256 字节 key，权限仍为 `0600`。
- 首次 `TRUST_ALWAYS` 握手成功后由 `connect` 自动写入；重连时从记录取 derived_key 放入 REQ_02。
- `trust reset` 使用 `WifiTrustHandshake::new_with_trust_remove` 发送 `trust_type=6`，
  收到手机确认（TRUST_WAITING）即完成，并建议随后 `trust remove` 清理本地记录。

### 3.4 CLI

```text
handshaker device discover [--browse-timeout 6s]
handshaker --wifi IP:PORT <业务命令>          # 与 --serial 互斥
handshaker trust list
handshaker --yes trust remove DEVICE_UUID
handshaker --yes --wifi IP:PORT trust reset DEVICE_UUID
```

- 危险操作（remove/reset）遵循统一确认策略；reset 必须带 `--wifi`。
- WiFi 连接前在 stderr 打印信任提示（首次连接需要手机端操作）。

## 4. 验证

### 4.1 自动化测试

- discovery：事件→WifiDevice 转换、instance 解析、去重（3 个单元测试）。
- 握手：首次连接（WAITING→ALWAYS+derived_key）、重连携带存储 key、错误 key→failed、
  TRUST_REMOVE 确认（4 个端到端测试，loopback 假手机）。
- 抓包向量：REQ_01 携带 `host_app_version=2.5.6`/`heartbeat_timeout=30` 等；RESPONSE_01
  解码 docs/14 §10.2 实测字段（2 个向量测试）。
- 客户端集成：`FakeWifiSsp` 完整握手 + 签名业务阶段，验证 derived_key 按 device_uuid 持久化、
  `device info`/`ping`/`fs ls` 复用、QUIT 正常（1 个集成测试）。
- CLI：`device discover` JSON/human、`--wifi` 解析与 `--serial` 冲突、`trust remove` 确认策略。
- 全量：lib 66、CLI 6、localization 1，全部通过。

### 4.2 真机验证（本机实测）

- `handshaker device discover` 在本机局域网真实发现手机
  （`handshaker_ssp_` / 192.168.2.47 / SRV 动态端口，与 docs/14 同一台 OD103），
  JSON 与 human 输出均正确。

### 4.3 完整真机验收（2026-08，Smartisan OD103 / Android 7.1.1）

> 环境：Mac（本机）与手机（192.168.2.47）同一局域网；隔离 HOME（全新 host_uuid）用于
> 首次信任验收；验收后测试目录、剪贴板条目、临时环境与 adb forward 均已清理。

| 验收项 | 结果 |
|---|---|
| mDNS 发现（动态 SRV 端口） | ✅ `device discover` 返回 `handshaker_ssp_` / 192.168.2.47 / 动态端口 |
| 首次连接 + 手机端授权 | ✅ 全新 host_uuid 首次连接成功，`device info` 完整返回（apk 201/1.2.0、model odin、phone_id=`e976ce6596c81fc5`、root=`/storage/emulated/0`） |
| 信任持久化 | ✅ `state.json`(0600) 按 device_uuid 存储 derived_key（base64/256B）；`trust list` 返回且不泄露密钥 |
| 重连免弹窗 | ✅ 信任后重连 `ping` 15ms 快速返回，无信任等待（derived_key 复用） |
| WiFi 业务复用 | ✅ ping、`fs ls`、mkdir、push(204800B)、pull（**MD5 与原文件一致**）、mv、clipboard set/get/delete、rm --recursive、fs exists 全部成功 |
| trust reset | ✅ `trust reset <uuid>` 清除手机端记录（TRUST_REMOVE） |
| failed 自动清理 | ✅ reset 后重连被手机拒绝（failed），正确报错并**自动清除本地陈旧记录**（`trust list` 变空） |
| 信任重建 | ✅ 再次连接重新授权并重建信任（updated_at 更新） |
| 清理 | ✅ 测试目录已删、剪贴板测试条目已删、无残留 adb forward、临时 HOME 已删 |

**验收过程中观察**：手机端 WiFi 端口周期性变化，偶发 `early eof`（连接断开）；重新
`device discover` 获取最新端口后重试均成功——与 §5 已知限制一致。

## 5. 已知限制

- 发现依赖多播；若网络隔离多播（AP/IGMP），`mdns-sd` 无 unicast 回退，可能发现不到设备。
- 信任等待窗口固定 120 秒；超时后报错，不自动重试。
- 手机端端口周期性变化，CLI 一次 `--wifi IP:PORT` 使用同一地址；长期使用应重新 discover。
- WiFi 下 `device list` 仍只列 ADB 设备；WiFi 设备用 `device discover` 查看。

## 5.1 信任与安全边界

- **LAN 不可信**：derived_key（长期信任凭证）在首次信任时以明文 protobuf 跨局域网传输
  （协议固有行为），同网段嗅探者可截获后冒充本主机。仅在可信局域网使用，或接受该边界。
- **恶意设备可触发本地记录清除**：信任记录按手机自报的 `device_uuid` 索引；恶意设备可冒充
  该 uuid 并在握手时返回 `failed`，从而清除本地对应记录（触发时有 `tracing::warn` 日志，
  记录可在下次连接重建）。不要连接来源不明的 `--wifi` 地址。
- **握手响应长度受限**：握手阶段重组响应设有 16 MiB 上限，防止恶意设备声明超大长度前缀
  撑爆客户端内存。
- **derived_key 长度固定校验**：手机回显的 derived_key 超过 256 字节即报协议错误，
  不会写入 state.json。
- **human 输出原样打印设备名/mDNS 字段**：局域网设备可注入控制字符；JSON 输出不受影响，
  脚本应使用 `--output json`。
