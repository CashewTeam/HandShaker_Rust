# ADB v0.1 基线验收报告

> 验收日期：2026-08-02  
> CLI 版本：0.1.3  
> 手机兼容身份：host_app_version 2.5.6 / host_app_version_code 408  
> 设备：Smartisan U2 Pro（OD103 / odin，设备序列号已脱敏）  
> Android：7.1.1  
> HandShaker APK：version code 201，version name 1.2.0

## 验收环境

- macOS Apple Silicon
- Rust stable
- ADB Platform-Tools
- 手机已授权 USB 调试
- 手机端组件：`com.smartisanos.smartfolder.aoa/.service.ConnectionManagerService`
- 手机根目录：`/storage/emulated/0`

本次执行使用指定设备序列号连接。报告中的命令使用 `<SERIAL>` 占位，复现时替换为目标设备序列号。

## 验收步骤

### 只读连接检查

```sh
handshaker --serial <SERIAL> --output json device info
handshaker --serial <SERIAL> --output json device ping
handshaker --serial <SERIAL> --output json fs ls /storage/emulated/0 --depth 1
```

结果：

- `device.info` 成功，设备名称为 Smartisan U2 Pro，root path 为 `/storage/emulated/0`。
- `device.ping` 成功，实测往返约 7 ms。
- 根目录读取成功。

### 文件 CRUD 和传输

本次使用唯一临时目录：

```
/storage/emulated/0/HandShaker_Rust_M0_20260802_161714
```

执行流程：

```sh
handshaker --serial <SERIAL> fs mkdir /storage/emulated/0/HandShaker_Rust_M0_20260802_161714
handshaker --serial <SERIAL> --output json --yes fs push \
  ./source.txt \
  /storage/emulated/0/HandShaker_Rust_M0_20260802_161714/upload.txt \
  --overwrite
handshaker --serial <SERIAL> --output json fs exists \
  /storage/emulated/0/HandShaker_Rust_M0_20260802_161714/upload.txt
handshaker --serial <SERIAL> --output json fs stat \
  /storage/emulated/0/HandShaker_Rust_M0_20260802_161714/upload.txt
handshaker --serial <SERIAL> --output json --yes fs pull \
  /storage/emulated/0/HandShaker_Rust_M0_20260802_161714/upload.txt \
  ./download.txt \
  --overwrite
handshaker --serial <SERIAL> --output json fs mv \
  /storage/emulated/0/HandShaker_Rust_M0_20260802_161714/upload.txt \
  /storage/emulated/0/HandShaker_Rust_M0_20260802_161714/renamed.txt
handshaker --serial <SERIAL> --output json --yes fs rm \
  /storage/emulated/0/HandShaker_Rust_M0_20260802_161714/renamed.txt
handshaker --serial <SERIAL> --output json --yes fs rm \
  /storage/emulated/0/HandShaker_Rust_M0_20260802_161714 \
  --recursive
```

结果：

- 上传成功，字节数 78。
- `fs.exists` 返回 `true`。
- `fs.stat` 返回普通文件，大小 78 字节。
- 下载成功，字节数 78。
- 本地源文件与下载文件 MD5 均为 `fcc9729c7231b47278abcec19788869d`。
- `cmp` 字节比较一致。
- 重命名、文件删除和递归目录删除成功。
- 删除后再次 `fs.exists` 返回 `false`。

### 剪贴板

```sh
handshaker --serial <SERIAL> --output json clipboard get
handshaker --serial <SERIAL> --output json clipboard set <unique-test-marker>
handshaker --serial <SERIAL> --output json clipboard get
handshaker --serial <SERIAL> --output json --yes clipboard delete <marker-timestamp>
```

结果：

- 剪贴板读取成功。
- 唯一测试标记写入并可再次读取。
- 只删除本次写入的测试条目。
- 未执行 `clipboard clear`，未清空既有剪贴板历史。
- 剪贴板正文和时间戳不写入本报告。

### 资源清理

验收前后均执行：

```sh
adb -s <SERIAL> forward --list
```

结果：

- 本次创建的 ADB forward 已清理。
- 验收结束时目标设备没有残留 forward。
- 手机测试目录已删除。
- 本地测试文件已删除。
- 每个 CLI 命令均完成 QUIT 和连接清理。

## 自动化基线

通过的检查：

```sh
cargo fmt -- --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo build --release
cargo doc --no-deps
cargo test --doc
git diff --check
```

测试覆盖包括假 ADB、真实握手流程的假 SSP 服务、设备字段缺失、手机错误码、上传/下载失败、MD5
错误、临时文件清理、JSON/JSONL schema、CLI 参数解析和中文文案扫描。本次共通过 54 个测试：库
36、CLI 单元 13、CLI 集成 3、本地化约束 1、rustdoc 示例 1。

## 已知限制

- 真机验收只覆盖 ADB；WiFi、USB AOA、媒体、目录监控和照片同步仍未实现。
- `fs push` 的公开命令顺序固定为 `LOCAL REMOTE`；`fs pull` 固定为 `REMOTE LOCAL`。
- 本报告不代表递归传输、断点续传或后台 daemon 已实现。
