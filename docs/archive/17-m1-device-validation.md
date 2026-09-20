# M1 真机验收报告

## 环境

- 日期：2026-08-02
- 平台：macOS ARM64
- Rust：`rustc 1.97.1 (8bab26f4f 2026-07-14)`
- Cargo：`cargo 1.97.1 (c980f4866 2026-06-30)`
- Cargo package：`handshaker_rust 0.1.4`
- 手机：Smartisan U2 Pro，型号标识 OD103；设备序列号不写入仓库
- 通道：ADB，指定 OD103；测试前 `adb forward --list` 为空
- 手机兼容身份：`host_app_version=2.5.6`、`host_app_version_code=408`

## 可复现命令

将 `<OD103_SERIAL>` 替换为本机 `adb devices -l` 中 OD103 的脱敏前序列号；不要把真实序列号写入报告或提交：

```sh
adb devices -l
cargo run --quiet -- --serial <OD103_SERIAL> device info
HANDSHAKER_M1_SERIAL=<OD103_SERIAL> cargo run --quiet --example m1_real_validate
adb -s <OD103_SERIAL> forward --list
```

`examples/m1_real_validate.rs` 只在显式设置 `HANDSHAKER_M1_SERIAL` 时运行。它使用四类显式 callback，
订阅事件，写入带时间戳的唯一剪贴板标记，优先从 `ClipboardChanged` 事件取得本次条目时间戳，再删除这一个条目。
如果事件超时，则只在内存中查找本次标记以完成清理；不会清空剪贴板，也不会输出其他剪贴板正文。

## 结果

| 步骤 | 结果 |
|---|---|
| 设备状态和身份 | 通过；指定设备状态为 `device`，设备信息报告 Smartisan U2 Pro / OD103 |
| 显式 callback 连接 | 通过；ADB 握手、设备信息请求和连接关闭成功 |
| 事件订阅 | 通过；`EventSubscription` 可消费并按 `EventKind` 过滤 |
| 设备信息变更事件 | 未观察到；测试窗口内没有主动改变设备信息，未通过破坏性或设置操作强行触发 |
| 剪贴板唯一标记 | 通过；收到 `ClipboardChanged`，取得本次标记时间戳 |
| 剪贴板清理 | 通过；只删除本次写入条目，未执行 clear |
| ADB forward 清理 | 通过；测试结束后 `adb forward --list` 无本次残留 |
| 媒体/目录/同步事件 | 未执行；M1 不提供对应 CLI，触发它们需要扩大设备文件或媒体操作范围 |

## 限制

本报告确认了真实 ADB 连接、显式 callback 配置、剪贴板主动事件和清理路径。设备信息变更、媒体库、目录监控、
照片同步和同步监控没有在本次受控范围内伪造或强行触发；它们的 field 1 解码、过滤、慢消费者和取消路径由本地
假 SSP/单元测试覆盖。M1 不加入自动重连，也不改变 ADB 握手常量或手机兼容身份。
