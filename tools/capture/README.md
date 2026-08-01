# tools/capture — SSP 抓包验证工具

基于 [docs/14-capture-validation](../docs/14-capture-validation.md) 结论的可复现验证脚本。
通过 `adb forward` 与手机端 HandShaker 服务直接对话，**逐字节记录**双向流量。

## 依赖

- Python 3.10+，`cryptography` 库（`pip3 install cryptography`）
- adb 且已连接装有 HandShaker APK 的手机（`com.smartisanos.smartfolder.aoa`）

## 快速开始

```bash
# 1. 在手机端启动协议服务（监听 ADB_PORT=10086）
adb shell am startservice -n com.smartisanos.smartfolder.aoa/.service.ConnectionManagerService \
    --ei ADB_PORT 10086

# 2. 建立 adb forward
adb forward tcp:10086 tcp:10086

# 3. 运行验证
python3 ssp_capture.py            # 握手 + 设备信息 + 心跳 + 列目录 + 下载 + 取消 + 退出
python3 ssp_multiframe.py         # 多帧分块（>32761B 响应）+ 大文件下载帧流
python3 ssp_upload.py             # 上传数据面（header→flag=3→done）
python3 ssp_errors.py             # MD5 异常 + 取消行为

# 4. 清理
adb shell am force-stop com.smartisanos.smartfolder.aoa
adb forward --remove tcp:10086
```

## 环境变量

| 变量 | 默认 | 说明 |
|---|---|---|
| `HSPORT` | 10086 | adb forward 宿主端口 |
| `ADB_PORT` | 10086 | 传给手机服务的监听端口 |
| `ENCKEY_MODE` | `aes_cbc` | `aes_cbc`（正常）或 `identity`（验证无加密必失败） |
| `CAP_FILE` | `/tmp/ssp_capture.log` | 字节日志路径 |
| `UP_PATH` / `UP_SIZE` | `/storage/emulated/0/Download/ssp_upload_test.bin` / 150000 | 上传测试目标与大小 |

## 已验证结论（简版）

- 上行帧：`[sid:4 BE][flag:1][len:4 BE][payload]`
- 下行帧：`[sid:4 BE][chunkLen:2 BE][data]`，单帧数据 ≤32761B；
  普通响应数据首 8 字节为大端总长；文件流无前缀
- enckey：`MD5(DER) + AES-256-CBC(PKCS7)(base64(DER))`，密钥/IV 见 `ssp_capture.py` 内 `KEY_TABLE`
- 业务请求：`[128B SHA256withRSA 签名][protobuf]`（仅 flag=1）

## 注意

- 上传会往手机写测试文件（脚本已自动清理，但 `ssp_errors.py` 的坏 md5 用例会写
  `ssp_md5bad.bin`，如残留可手动删除）。
- 手机锁屏时握手会回 `"locked"`，需先解锁（`adb shell input keyevent 82`）。
- 仅用于协议研究与互操作验证；请不要对线上无关设备运行。
