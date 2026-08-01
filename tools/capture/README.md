# tools/capture — SSP 抓包验证工具

基于 [docs/14-capture-validation](../docs/14-capture-validation.md) 结论的可复现验证脚本。
覆盖 **ADB 通道**与 **WiFi/局域网通道**（Bonjour 发现 + 握手 + 传输），逐字节记录双向流量。

## 依赖

- Python 3.10+，`cryptography`（`pip3 install cryptography`）
- adb 且已连接装有 HandShaker APK 的手机（`com.smartisanos.smartfolder.aoa`）
- 局域网验证需手机与电脑同网段（WiFi）

> **macOS 注意**：macOS 15+「本地网络（Local Network）」权限会拦截非系统进程的局域网流量
> （ping 正常但 TCP/UDP 报 `No route to host`）。局域网验证请用 **`sudo`** 运行
> （`ping` 是系统二进制豁免；sudo 进程不受门控）。

## 1) ADB 通道验证

```bash
# 手机端启动协议服务（监听 ADB_PORT=10086）
adb shell am startservice -n com.smartisanos.smartfolder.aoa/.service.ConnectionManagerService \
    --ei ADB_PORT 10086
adb forward tcp:10086 tcp:10086

python3 ssp_capture.py            # 握手 + 设备信息 + 心跳 + 列目录 + 下载 + 取消 + 退出
python3 ssp_multiframe.py         # 多帧分块（>32761B 响应）+ 大文件下载帧流
python3 ssp_upload.py             # 上传数据面（header→flag=3→done）
python3 ssp_errors.py             # MD5 异常 + 取消行为

# 清理
adb shell am force-stop com.smartisanos.smartfolder.aoa
adb forward --remove tcp:10086
```

## 2) WiFi / 局域网验证

```bash
# 一键（sudo）：mDNS 发现 → 自动检测端口 → 完整验证（首次会弹信任框）
sudo ./run_lan.sh 192.168.2.47

# 或分步：
sudo python3 ssp_mdns.py 6 --ip 192.168.2.47                     # mDNS 发现（unicast+多播）
sudo python3 ssp_wifi.py --ip 192.168.2.47 --port 45656 --reset-trust  # 重置信任+重新信任
sudo python3 ssp_wifi.py --ip 192.168.2.47 --port 45656                  # derived_key 重连（无弹窗）
```

- 端口以 mDNS SRV 记录为准（WiFi 服务器端口会周期性变化；`run_lan.sh` 从手机
  `/proc/net/tcp6` 检测 uid 10062 的 LISTEN 端口兜底）。
- 首次信任需在手机上点"信任"（脚本尝试 adb uiautomator 自动点按；失败时手动点）。
- 信任后 `ssp_wifi.py` 会把 `derived_key` 存到 `/tmp/ssp_wifi_trust.json`，下次重连免弹窗。

## 环境变量

| 变量 | 默认 | 说明 |
|---|---|---|
| `HSPORT` | 10086 | adb forward 宿主端口 |
| `ADB_PORT` | 10086 | 传给手机服务的监听端口 |
| `ENCKEY_MODE` | `aes_cbc` | `aes_cbc`（正常）或 `identity`（验证无加密必失败） |
| `CAP_FILE` | `/tmp/ssp_capture.log` | 字节日志路径 |

## 已验证结论（简版）

- 上行帧：`[sid:4 BE][flag:1][len:4 BE][payload]`
- 下行帧：`[sid:4 BE][chunkLen:2 BE][data]`，单帧数据 ≤32761B；
  普通响应数据首 8 字节为大端总长；文件流无前缀
- enckey：`MD5(DER) + AES-256-CBC(PKCS7)(base64(DER))`，密钥/IV 见 `ssp_capture.py` 内 `KEY_TABLE`
- 业务请求：`[128B SHA256withRSA 签名][protobuf]`（仅 flag=1）
- Bonjour：`_handshaker_ssp._tcp.`，实例 `handshaker_ssp_`，SRV 端口实时准确
- 信任：`TRUST_REMOVE` 清除记录；重连带 `derived_key` 免弹窗

## 注意

- 上传会往手机写测试文件（`ssp_wifi_test.bin` / `ssp_upload_test.bin` / `ssp_md5bad.bin`），
  结束后请清理（`adb shell rm -f /storage/emulated/0/Download/ssp_*`）。
- 手机锁屏时握手会回 `"locked"`，需先解锁（`adb shell input keyevent 82`）。
- 仅用于协议研究与互操作验证；请不要对无关设备运行。
