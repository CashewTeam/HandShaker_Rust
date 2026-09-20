# 23 M7:USB AOA 连接(0.6.0)

## 1 目标与范围

新增 USB 传输通道,复用现有裸握手、Session 与全部业务 API。

- 传输层:libusb(`rusb`,macOS ARM64)。
- 协议:AOA accessory 模式 bulk 字节流,承载与 ADB/WiFi **完全相同**的 SSP 帧。
- 握手:复用 `AdbRawKeyExchange`(flag=0 裸公钥交换,已抓包验证)。
- CLI:`--usb [--serial <bus-ports>]`;`device list` 同时输出 ADB 与 USB 设备。
- 验收:与 ADB 相同的设备/文件/传输/剪贴板全业务;拔线立即报错、无残留。

## 2 传输层抽象(Phase 1)

原 `HandshakeStrategy`/`read_normal_direct` 硬编码 `&mut TcpStream`,USB 通道需要任意
双工字节流。本里程碑泛型化:

- `HandshakeStrategy<S>`:S = `AsyncRead + AsyncWrite + Unpin + Send`;`AdbRawKeyExchange`
  与 `WifiTrustHandshake` 均实现任意 S。
- `read_normal_direct<R>`:`R: AsyncRead + Unpin`(16 MiB 握手响应上限保留)。
- `wifi_handshake::send_unsigned<W>`:`W: AsyncWrite + Unpin`。
- `Session::establish<S>`:以 `tokio::io::split` 替代 `TcpStream::into_split`,
  `spawn_reader<S>`/`spawn_writer<S>` 泛型;Session/心跳/业务链路对传输完全透明。
- `ConnectedTransport.stream`:`Box<dyn TransportStream>`(新组合 trait
  `AsyncRead + AsyncWrite + Unpin + Send`);ADB/WiFi 连接改为 `Box::new(stream)`。
- client 侧按 target 具体化握手(去掉 `Box<dyn HandshakeStrategy>`)。

**传输无关性测试**:`handshake_and_request_round_trip_over_memory_stream` 用
`tokio::io::duplex` 内存流跑完整裸握手 + RSA 签名心跳 + QUIT——与 TCP 版本逐字节等价,
证明 USB bulk 流与 TCP 对 Session 语义一致。

## 3 AOA 传输实现(Phase 2,`src/transport/usb.rs`)

### 3.1 设备枚举与模式

| 常量 | 值 | 说明 |
|---|---|---|
| `ACCESSORY_VID` | `0x18d1` | AOA accessory 模式 VID |
| `ACCESSORY_PIDS` | `0x2d00/01/02/03` | accessory(+adb/+audio) |
| `SMARTISAN_VID` | `0x29a9` | Smartisan 普通模式 VID(OD103 实测 PID `0x7020`) |
| 接口过滤 | class 0xff / subclass 0xff | accessory bulk 接口 |

`UsbAccessory { location("bus-ports"), bus_number, serial, vendor_id, product_id, mode }`,
`mode ∈ { Accessory, Plain }`。`device list` 同时列出两种模式,Plain 设备标记待切换。

### 3.2 AOA identification 与切换(范围补充)

> **范围说明**:原计划"不做非 accessory 模式的识别与切换"。真机验收期间发现
> Smartisan 定制 ROM **锁定默认 5 功能集**(`mtp,diag,mass_storage,accessory,adb`),
> 普通模式下 accessory 接口未绑定,AOA vendor 请求(0x51)被 STALL,`svc usb`/`setprop`
> 均无法切到 `accessory,adb`(见 §6)。为恢复设备可达,补充实现了 identification
> 路径(与 Mac 端 `SFUSBDevice getAOAVersion/sendStartAccessoryControl` 一致):

1. 枚举到 Plain 设备后 `open` + `set_auto_detach_kernel_driver(true)`。
2. `GET_PROTOCOL(0x51)`:`bmRequestType=0xC0, wValue=0, wIndex=0`,读 2 字节协议版本。
3. `SEND_STRING(0x52)` ×6(**UTF-16LE**,AOA 规范):
   manufacturer=`Smartisan`、model=`HandShaker`、description=`HandShaker`、
   version=`1.0`、uri=`http://sf.smartisan.com/sf/release/apk`、serial=`host_uuid`。
   与手机 `accessory_filter.xml`(manufacturer=Smartisan model=HandShaker version=1)
   及 Mac 客户端字符串一致。
4. `ACCESSORY_START(0x53)` 后设备重新枚举为 0x18d1;`wait_for_accessory` 轮询
   同 location 的 accessory 设备(6s 超时)。

### 3.3 端点与流

- `accessory_endpoints`:读取 active config,在 0xff/0xff 接口找一对 bulk IN/OUT 端点。
- `UsbStream`:`AsyncRead + AsyncWrite` 桥接——
  - 读:专用线程 `spawn_blocking` 循环 `read_bulk`(16 KiB 块,5s 超时续读),经
    unbounded channel 投递;`poll_read` 只 poll channel,不阻塞 reactor。
  - 写:`spawn_blocking` 的 `write_bulk` + oneshot 状态机,`poll_write`/`poll_flush`
    等待 in-flight 写,不阻塞 reactor。
  - 短读/EOF:`read_bulk == 0` → 投递 EOF;错误(NoDevice/NotFound/Busy/Pipe)→
    映射为 `usb.device_gone/busy/pipe_error` 并结束读线程。
- 热插拔:读侧错误结束 → Session `fail_connection` → 统一关闭,业务请求报
  `usb.device_gone`。

## 4 CLI

- 全局 `--usb`(与 `--wifi` 冲突;`--serial <bus-ports>` 指定 location,与 ADB 风格一致)。
- `connection_target()` 按 usb/wifi/adb 分派。
- `device list`:JSON `data = { adb: [...], usb: [...] }`(**schema 变更**:
  原 `data` 为 ADB 设备数组,现为对象;human 输出 ADB 表 + USB 表)。
- 参数预检:sync `plan/run/watch` 缺 `--output-dir` 在连接前报 usage(exit 2)。
- `sync run` 无 `--output-dir` 不再先连设备。
- **`batch`(0.6.1,长连接批量会话)**:`handshaker --usb batch < script.txt`
  从 stdin 逐行读取命令,在单个持久连接上顺序执行。Session 心跳在命令间隙
  持续保活,手机端 accessory 会话保持到 `exit`/`quit`/EOF(随后 QUIT),规避
  每命令一次连接的"会话单次性"成本。输出遵循 `--output`;命令级失败打印到
  stderr 并继续,结束时若有失败行则非零退出;传输/握手/超时/协议错误以及
  stdin/输出写入失败**中止**并优先返回该错误(脚本可区分"中止"与"完成但有
  失败")。内置命令(`cd`/`lcd`/`pwd`/`lpwd`/`help`/`exit`/`quit`)与 REPL 一致;
  `shell`/`batch` 嵌套被拒绝。
- batch 行内**连接目标类全局参数(`--serial`/`--wifi`/`--usb`/`--timeout`/
  `--wire-log` 等)被忽略**(仅 `--yes` 与子命令生效),命令运行在 batch 启动
  时的连接上;破坏性命令(`fs rm`/`fs mv` 等)在 batch(非 TTY)中必须带
  `--yes`,否则按 ConfirmationRequired 记为该行失败。

## 5 i18n

`usb.*` 12 个 key:enumerate_failed/config_failed/no_bulk_endpoints/device_not_found/
ambiguous_devices/open_failed/claim_failed/device_gone/busy/pipe_error/transfer_failed/
aoa_protocol_failed/aoa_unsupported/aoa_string_failed/aoa_start_failed/aoa_switch_failed;
`device.usb_list_header`;`cli.command.batch`/`batch.read_failed`/`batch.failures`。

## 6 真机发现与验收记录(2026-08-03,OD103)

### 6.1 设备初始状态

- 初始 USB 处于 accessory 模式(VID `0x18d1`/PID `0x2d01`,functions=`accessory,ffs`,
  mCurrentAccessory 存在且 mSerial=host_uuid)——说明 Mac 端官方 HandShaker 曾成功
  identification。
- `device list` 实测:ADB(3f13d4b4)+ USB accessory(1-1,18d1:2d01)同时列出。

### 6.2 验收期间发现(重要)

1. **Smartisan ROM 锁定默认 5 功能集**:`svc usb setFunction`/`setprop sys.usb.config`
   均被系统忽略,funtions 恒为 `mtp,diag,mass_storage,accessory,adb`(adb 位置常为 ffs)。
   唯一成功的是 `setFunction none`(全断)与 `setFunction mtp`(回到默认集)。
2. **AOA identification 三个关键修复(从 Mac 版 `sendAOAStartupRequest` 反汇编获得)**:
   - SEND_STRING 请求码为 **十进制 52(`0x34`)**,初版误写为 `0x52` → 全部 STALL;
   - 字符串 **index 为 0 基 0..=4**(f_accessory 期望),初版误用 1..=6;
   - 字符串编码为 **UTF-8**(Mac 端 `cStringUsingEncoding:NSUTF8StringEncoding`);
     初版 UTF-16LE 导致 `mManufacturer="S"` 截断,filter 不匹配、配件弹窗发给其他 App。
   - `GET_PROTOCOL` 失败必须**容忍**(该 ROM 复合模式必 STALL),初版直接报错。
   修复后 identification + START 成功:设备切 `0x18d1/0x2d01`,`entering USB accessory
   mode: UsbAccessory[mManufacturer=Smartisan, mModel=HandShaker, mDescription=HandShaker,
   mVersion=1.0, mUri=<host_uuid>]`,系统 ATTACHED 发给 `smartfolder.aoa/.MainActivity`,
   用户授权后 openAccessory 成功。
3. **业务会话单次性**:QUIT 后手机端关闭 accessory fd,读线程退出;设备保持 0x18D1
   配件模式但 App 不再监听,后续连接需拔插重识别(或手机端重新授权)。
4. **accessory 模式退出仅由物理拔线触发**:`libusb_reset_device`(Mac 版 close 的
   做法)实测不会使 Android 退出 accessory(设备重枚举仍是 0x18D1,无新 ATTACHED);
   UsbDeviceManager 日志 `exited USB accessory mode` 只在 USB_STATE 变化(拔线)时出现。
   host 侧无 AOA "exit" control,协议上无法软件退出。
5. **USB 模式遍历实测**:仅充电=0x05c6/0xf006(无接口)、PTP=0x05c6/0x904d(单 PTP
   接口)、MTP/复合=0x29a9/0x7020(5 接口,accessory 接口写可通);均需先 AOA
   identification 才能建立配件会话。

### 6.3 结论与验证等级

- 自动化:150 测试通过(lib 119 + bin 21 + cli 10 + localization 1);传输无关性
  (duplex 全握手)、枚举过滤/错误映射/identification 常量均有单测。
- 真机**完整业务验收通过**(单连接,`examples/usb_accept`):connect/ping/list_dir/
  mkdir/upload/download/**verify_md5(8192B 完全一致)**/rename/clipboard_set/get/
  cleanup/quit 全 PASS;`--usb device info` 返回完整设备信息(apk 1.2.0/201、
  odin、e976ce6596c81fc5)。多命令场景需单连接(REPL/library),因手机端业务会话单次。
- 验证等级:「自动化测试 + 真机完整业务互通(设备/文件/传输/剪贴板)」。

## 7 已知限制

- **USB 配件模式生命周期由 Android 管理**:仅在物理拔线时退出;host 断开(QUIT)只结束
  业务会话,设备保持 0x18D1。CLI `--usb` 单命令场景每次需拔插重识别;多命令请用
  单连接(REPL/library)。
- 未实现 Smartisan 普通模式识别失败的降级提示(当前直接报 STALL 错误)。
- 多台 accessory 设备需 `--serial` 指定;ambiguous 报错。
- Linux udev 规则未评估(本里程碑仅 macOS ARM64)。
- USB 通道的 `watch`/媒体库推送未单独验收(依赖手机端 openAccessory,同 §6 阻塞)。
