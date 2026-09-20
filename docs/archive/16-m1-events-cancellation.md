# M1：事件订阅与公共取消模型

M1 在不改变 ADB 裸握手、SSP 帧格式、手机兼容身份 `2.5.6 / 408` 和原有 library 方法的前提下，
把 Session 的未匹配 sid 消息变成可消费的强类型事件流，并为请求增加可选取消。

## 1. 事件订阅

```rust,no_run
use handshaker_rust::{EventFilter, HandShakerClient};

# async fn example(client: &HandShakerClient) -> handshaker_rust::Result<()> {
let mut subscription = client.subscribe_events(EventFilter::all());
while let Ok(event) = subscription.recv().await {
    println!("{}", serde_json::to_string(&event).unwrap());
}
# Ok(())
# }
```

订阅句柄只暴露 `recv()`、`try_recv()` 和 `close()`，不暴露 Tokio channel。`EventFilter::all()` 接收所有
事件；`EventFilter::only([...])` 按 `EventKind` 筛选。

当前事件领域类型如下：

| 事件 | 数据 |
|---|---|
| `device_info_changed` | `DeviceInfo` |
| `clipboard_changed` | 解压后的 `ClipboardEntry` 列表 |
| `media_library_changed` | 统一的照片、视频或音频变更 |
| `directory_changed` | 目录监控 `FileEvent` 列表 |
| `file_changed` | 同步 `FileChange` 列表 |
| `photo_sync_changed` | 照片同步状态和文件快照 |
| `sync_monitor_changed` | 实时同步结果 |
| `request_cancelled` | 远端取消的 session id 和错误码 |
| `unknown` | sid、request type、payload 长度和安全的判定原因 |

事件解码只保留领域数据和安全元数据，不把原始 payload 放入事件、错误或普通日志。field 1 存在时按
`SSPRequestType` 解码；field 1 缺失时只有在候选结构唯一且字段具备足够特征时才推断，否则发布 `unknown`。

## 2. 广播和慢消费者契约

Session 维护一个固定容量为 64 的广播核心，每个订阅者有独立游标：

- 一个订阅者消费慢不会阻塞普通请求、读任务或其他订阅者；
- 订阅者落后时返回 `EventStreamError::Lagged { missed }`；处理该错误后可以继续读取后续事件；
- `EventSubscription::close()` 只停止当前订阅；
- Session 关闭、EOF 或连接失败后返回 `EventStreamError::Closed`；
- 不自动重连，也不会从上次事件位置恢复，调用方需要重新连接和订阅；
- 过滤发生在订阅句柄内，过滤掉的事件仍会推进该订阅者游标。

pending map 中的 sid 始终优先交给原请求。只有请求完成并移除 sid 后，后续完整消息才会进入事件解码。
下载裸流期间无法安全区分的普通消息仍按协议错误处理，绝不会写入目标文件。

## 3. Callback 开关

`HandShakerClient::connect()` 保持原行为，初始 `GET_DEVICE_INFO` 中的设备信息、照片、音频和视频 callback
全部关闭。订阅本身不会改变手机端开关。需要手机主动推送时显式使用：

```rust,no_run
use handshaker_rust::{
    ClientOptions, ConnectionTarget, EventCallbacks, HandShakerClient,
};

# async fn example() -> handshaker_rust::Result<()> {
let client = HandShakerClient::connect_with_event_callbacks(
    ConnectionTarget::Adb { serial: None },
    ClientOptions::default(),
    EventCallbacks {
        device_info: true,
        photo_library: true,
        ..EventCallbacks::default()
    },
).await?;
client.close().await
# }
```

M1 只提供事件底座；CLI 尚未增加目录、媒体或同步 watch 命令。

## 4. 请求取消

原有方法保持不变，例如 `ping()`、`download()` 和 `clipboard_set()`。带取消的版本接收
`RequestOptions`：

```rust,no_run
use handshaker_rust::{CancellationToken, RequestOptions};

# async fn example(client: &handshaker_rust::HandShakerClient) -> handshaker_rust::Result<()> {
let token = CancellationToken::new();
let request = client.ping_with_options(RequestOptions::with_cancellation(token.clone()));
token.cancel();
let _ = request.await;
# Ok(())
# }
```

取消行为按请求阶段区分：

- 普通请求和上传会移除 pending sid，发送 flag `2`，payload 为目标 sid 的大端 `u32`，连接继续可用；
- 返回的 `Error::Cancelled` 包含 `CancellationOrigin::Local { flag_sent }`；
- 手机发送 `SSPCancelRequest` 时，匹配的请求返回 `CancellationOrigin::Remote { error_code }`；没有对应
  pending 请求时，发布 `ClientEvent::RequestCancelled`；
- 下载取消不发送一个假装能中断裸流的成功状态，而是关闭当前连接，删除临时文件，保留原目标文件；
- 超时、EOF、协议错误和普通连接关闭仍保持各自原有错误分类，不伪装成取消；
- 本地取消的 CLI 退出码为 `130`，手机端取消的退出码为 `6`，稳定错误 code 为 `cancelled`；
- 取消在请求发送前发生时不会发送一个无对应手机请求的 flag `2`，而是返回 `flag_sent: false`。

`CancellationToken` 可以 clone 给多个协作任务，但每个请求仍需通过自己的 `RequestOptions` 显式绑定。
M1 不提供自动重试或自动重连。

## 5. 尚未包含的能力

M1 不实现 WiFi connector、WiFi 信任握手、USB AOA、后台 daemon、目录监控 CLI、媒体查询、递归传输或照片同步。
这些功能依赖后续里程碑，并继续使用独立的 transport/handshake 分层。
