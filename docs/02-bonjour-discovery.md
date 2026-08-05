# 02 Bonjour / mDNS 设备发现

设备发现使用 Apple Bonjour（mDNS/DNS-SD）。两端分别：

- **Android（广播方）**：`NsdManager` 注册 `_handshaker_ssp._tcp.` 服务。
- **macOS（浏览方）**：`NSNetServiceBrowser` 浏览同一服务类型，解析地址后连接。

## 2.1 Android 广播

类：`com.smartisanos.smartfolder.aoa.service.m`（WifiConnectionManager）。

启动 WiFi 服务器（`service/i.java:111-132`）：`new ServerSocket(0)` → 系统分配随机端口 →
`getLocalPort()` 记录。

注册 NsD 服务（`service/m.java:196-204`）：

```java
ServerSocket serverSocketA = mVar.e.a(0);
if (!mVar.h.get() && serverSocketA != null) {
    NsdServiceInfo nsdServiceInfo = new NsdServiceInfo();
    nsdServiceInfo.setServiceName("handshaker_ssp_");      // 服务名
    nsdServiceInfo.setServiceType("_handshaker_ssp._tcp."); // 服务类型
    nsdServiceInfo.setPort(mVar.e.d());                    // 真实 ServerSocket 端口
    mVar.h().registerService(nsdServiceInfo, 1, mVar.d);   // 1 = PROTOCOL_DNS_SD
}
```

要点：

- `NsdManager` 获取：`getSystemService("servicediscovery")`（`service/m.java:148-153`）。
- 注册回调：`b extends Handler implements NsdManager.RegistrationListener`（`m.java:75`）；
  `onServiceRegistered` 成功、`onRegistrationFailed` 报错。
- 停止：`unregisterService`（`m.java:225-228`）。

### 触发 / 停止时机

| 事件 | 行为 | 来源 |
|---|---|---|
| `StartWifiServerEvent`（未连接、WiFi 可用、非锁屏） | 启动广播 | `ConnectionManagerService.java:158-168` |
| `CONNECTIVITY_CHANGE`（WiFi 恢复 / 断开） | 重启 / 停止 | `service/e.java:20-30` |
| 弹出信任对话框 | 临时停止广播 | `m.java:155-163` |
| 信任完成 / 取消 | 恢复广播 | `m.java:165-179` |

## 2.2 macOS 浏览

类：`SFWifiDeviceManager <NSNetServiceDelegate, NSNetServiceBrowserDelegate, SFWifiSocketDelegate>`
（`SmartFinderCore.h:3674`）。方法：`startScan`、`netServiceBrowser:didFindService:`、
`netServiceDidResolveAddress:`、`tryConnectToDevice:withAddresses:rescan:`、
`probeWifiDevices`、`handleDeviceLeft:withService:notifyDelegate:`。

- 使用 `NSNetServiceBrowser`（二进制字符串 `_handshaker_ssp._tcp` 确认浏览类型）。
- 发现服务 → 解析地址（`netServiceDidResolveAddress:`）→ `tryConnectToDevice:withAddresses:`。
- 连接用 `SFWifiSocket`（原生 POSIX socket + 超时同步读写，`SmartFinderCore.h:3747`）。
- 设备去重：`connectedAddressSet`；离开用 `handleDeviceLeft:` + `lazyRemoveDeviceBySerialNumber:`。
- 周期 `probeTimer` / `rescanTimer` 探测在线设备。

### Mac 端设备对象

- `SFWifiDevice : SFGenericDevice`（`SmartFinderCore.h:3628`）：持 `NSNetService`、`SFWifiSocket`、
  `remoteIP/remotePort`、`maxRequestFailedTimes`（请求连续失败上限，用于判定设备掉线）。
- `SFGenericDevice` 是抽象基类：持有 RSA 密钥、握手状态机（`aoaHandShaking/aoaHandShakeOk/aoaHandShaking01OK`）、
  心跳超时、设备属性（uuid、型号、磁盘、电池等）。

## 2.3 二维码直连（并行的发现方式）

手机扫 Mac 屏幕上的二维码即可直连，不依赖 mDNS：

- Mac 端把本地 `IP:port` 编码进二维码（`http://t.tt/apps/handshaker?qr=1` 为引导下载页）。
- Android 端：`didScanQRCode:` 解码 → `h/d.java d(String)` 校验 `IP.P.P.P:port` 格式
  （`h/d.java:439-463`）→ `h.d.a(pcInfo, localIp, port)` 直连（`service/n.java:24-29`）。

## 2.4 Rust 实现要点

- 服务端（模拟手机）：用 `libmdns`/`mdns-sd` 发布服务：`_handshaker_ssp._tcp`，实例名
  `handshaker_ssp_<something>`，端口 = 监听端口。注意 TXT 记录可为空。
- 客户端（模拟 Mac）：浏览 `_handshaker_ssp._tcp`，解析 A/AAAA，取端口，发起 TCP。
- 跨子网不可见（mDNS 限制）；大网段部署需走二维码直连或 ADB。

> ✅ 已实现（2026-08，M2）：Rust 客户端用 `mdns-sd 0.20` 浏览并解析，见
> `src/discovery.rs` 与 `docs/18-m2-wifi-trust.md`；`handshaker device discover` 本机实测
> 成功发现真机。

## 2.5 抓包实测记录（2026-08，见 [14](14-capture-validation.md) §10）

- 手机 mdnsd 对 **unicast 查询**（发往手机 IP:5353）和 v4/v6 多播查询都会应答。
- 实测应答记录（PTR 1 答 + SRV/TXT/A/AAAA 6 附加）：

  | 类型 | 记录 |
  |---|---|
  | PTR | `handshaker_ssp_._handshaker_ssp._tcp.local.` |
  | SRV | priority=0 weight=0 **port=45656** target=`fixture-phone.local` |
  | TXT | 空 |
  | A | `fixture-phone.local` → 192.0.2.47 |
  | AAAA | fe80:: / 240e::…（3 条） |

- **SRV 端口与手机实际监听端口一致**；端口会周期性变化（WiFi 服务器注册/注销循环），
  客户端必须以 SRV 记录为准，不能缓存。
- 本网络 WiFi 上多播应答偶发丢失（AP/IGMP），unicast 查询是稳定路径；发现工具见
  `tools/capture/ssp_mdns.py`。
- 解析注意：DNS 名称压缩指针指向整个报文偏移；解析出的名字不带末尾点。
