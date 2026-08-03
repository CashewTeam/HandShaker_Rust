# HandShaker_Rust Phase D：Application 业务闭环执行计划

> 基线提交：`555367dd1e0a14977a68f8b6c67658cad344e5ae`  
> Workspace：`handshaker-core` / `handshaker-application` / `handshaker-cli` / `handshaker-ffi`  
> Application API：`1.0.0-preview.1`  
> 目标：完成设备发现诊断、设备信息、稳定身份、TrustService、文件预检/执行计划，以及可选的 SyncService，为 Swift/GTK/.NET 提供完整且不依赖 CLI/Core 的业务入口。

---

## 1. 范围与原则

Phase D 只修改 Application 业务层及必要的 Core 公共入口，不扩展 SSP 协议，不直接扩展 FFI。

依赖方向保持：

```text
handshaker-core
        ↑
handshaker-application
        ↑
   ┌────┴────┐
 CLI         FFI
```

所有新增公共模型必须：

- 不泄露 `HandShakerClient`、Prost、Session、Transport；
- 使用稳定英文 JSON token；
- 错误使用 `PublicErrorCode`，不能依赖字符串；
- 多 Session 情况下携带 `SessionId` 或稳定 `DeviceId`；
- 配置必须使用 Runtime 的 `state_dir`、timeout、wire log；
- 新增 DTO 必须有 serde fixture；
- Application API 继续保持 preview，Phase D 完成后再评估正式冻结。

---

# 2. 模块 D1：设备发现诊断

## 2.1 问题

当前 `list_devices()` 返回 `Vec<DeviceDescriptor>`，并静默吞掉 ADB 和 Wi-Fi 错误。Swift 无法区分：

- 确实没有设备；
- ADB 未安装；
- ADB server 启动失败；
- Wi-Fi mDNS 发现失败；
- USB 枚举或权限失败；
- 某通道失败但其他通道成功。

## 2.2 新模型

建议新增 `crates/handshaker-application/src/discovery.rs`：

```rust
use serde::{Deserialize, Serialize};

use crate::dto::{DeviceDescriptor, TransportKind};
use crate::error::PublicError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceDiscoveryResult {
    pub devices: Vec<DeviceDescriptor>,
    pub warnings: Vec<DeviceDiscoveryWarning>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceDiscoveryWarning {
    pub transport: TransportKind,
    pub error: PublicError,
}
```

保持兼容的迁移方式：

```rust
impl HandShakerRuntime {
    /// 新的权威接口。
    pub async fn discover_devices(
        &self,
        request: ListDevicesRequest,
    ) -> AppResult<DeviceDiscoveryResult>;

    /// Preview 期间保留的兼容包装；正式冻结前决定是否删除。
    pub async fn list_devices(
        &self,
        request: ListDevicesRequest,
    ) -> AppResult<Vec<DeviceDescriptor>> {
        Ok(self.discover_devices(request).await?.devices)
    }
}
```

## 2.3 核心实现

```rust
pub async fn discover_devices(
    &self,
    request: ListDevicesRequest,
) -> AppResult<DeviceDiscoveryResult> {
    self.ensure_open()?;

    let mut devices = Vec::new();
    let mut warnings = Vec::new();

    if request.include_adb {
        match HandShakerClient::list_adb_devices_with_timeout(
            &self.inner.config.adb_path,
            self.inner.config.default_timeout,
        )
        .await
        {
            Ok(list) => devices.extend(list.into_iter().map(adb_device_to_descriptor)),
            Err(error) => warnings.push(DeviceDiscoveryWarning {
                transport: TransportKind::Adb,
                error: from_core_error(error, "discover_devices.adb"),
            }),
        }
    }

    if request.include_wifi {
        match HandShakerClient::discover_wifi_devices(request.wifi_browse_timeout).await {
            Ok(list) => devices.extend(list.into_iter().map(wifi_device_to_descriptor)),
            Err(error) => warnings.push(DeviceDiscoveryWarning {
                transport: TransportKind::Wifi,
                error: from_core_error(error, "discover_devices.wifi"),
            }),
        }
    }

    if request.include_usb {
        match handshaker_core::list_usb_accessories() {
            Ok(list) => devices.extend(list.into_iter().map(usb_device_to_descriptor)),
            Err(error) => warnings.push(DeviceDiscoveryWarning {
                transport: TransportKind::UsbAccessory,
                error: from_core_error(error, "discover_devices.usb"),
            }),
        }
    }

    deduplicate_discovered_devices(&mut devices);
    devices.sort_by(device_sort_key);

    Ok(DeviceDiscoveryResult { devices, warnings })
}
```

映射函数必须独立、可测试：

```rust
fn adb_device_to_descriptor(device: handshaker_core::AdbDevice) -> DeviceDescriptor { /* ... */ }
fn wifi_device_to_descriptor(device: handshaker_core::WifiDevice) -> DeviceDescriptor { /* ... */ }
fn usb_device_to_descriptor(device: handshaker_core::UsbAccessoryInfo) -> DeviceDescriptor { /* ... */ }
```

## 2.4 错误策略

- 单个 transport 失败不使整个发现请求失败；
- Runtime 已关闭、请求参数非法等整体错误仍返回 `Err`；
- USB 错误不再终止 ADB/Wi-Fi 已获得的结果；
- 所有 partial failure 必须进入 `warnings`；
- GUI 可以显示“发现 1 台设备，同时 Wi-Fi 发现失败”。

## 2.5 测试

- 三个 transport 全关闭，返回空 devices/空 warnings；
- ADB 不存在，返回 ADB warning；
- 一个 transport 失败、另一个成功，保留成功设备；
- warning JSON token 稳定；
- devices 排序稳定；
- 不再存在 `let _ = error`。

---

# 3. 模块 D2：完整设备信息与稳定身份

## 3.1 补全 `DeviceInfoDto`

当前 Core 已有以下字段，但 Application 丢失：

```rust
pub external_storage_path: Option<String>,
pub disk_size: Option<u64>,
pub used_disk_size: Option<u64>,
pub battery_percentage: Option<u32>,
pub phone_locked: Option<bool>,
```

修改 `DeviceInfoDto`：

```rust
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DeviceInfoDto {
    pub serial: String,
    pub phone_id: Option<String>,
    pub name: Option<String>,
    pub model: Option<String>,
    pub brand: Option<String>,
    pub manufacturer: Option<String>,
    pub smartisan_version: Option<String>,
    pub apk_version: Option<String>,
    pub apk_version_name: Option<String>,
    pub root_path: String,
    pub external_storage_path: Option<String>,
    pub disk_size: Option<u64>,
    pub used_disk_size: Option<u64>,
    pub battery_percentage: Option<u32>,
    pub phone_locked: Option<bool>,
}
```

映射：

```rust
pub(crate) fn device_info_to_dto(info: &DeviceInfo) -> DeviceInfoDto {
    DeviceInfoDto {
        serial: info.serial.clone(),
        phone_id: info.phone_id.clone(),
        name: info.name.clone(),
        model: info.model.clone(),
        brand: info.brand.clone(),
        manufacturer: info.manufacturer.clone(),
        smartisan_version: info.smartisan_version.clone(),
        apk_version: info.apk_version.clone(),
        apk_version_name: info.apk_version_name.clone(),
        root_path: info.root_path.clone(),
        external_storage_path: info.external_storage_path.clone(),
        disk_size: info.disk_size,
        used_disk_size: info.used_disk_size,
        battery_percentage: info.battery_percentage,
        phone_locked: info.phone_locked,
    }
}
```

## 3.2 区分发现端点和稳定设备身份

Wi-Fi mDNS 端口是动态值，不能作为长期 `DeviceId`。

建议新增：

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DiscoveryEndpointId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StableDeviceId(pub String);
```

为了减少 preview 阶段破坏，也可先扩展现有 `DeviceDescriptor`：

```rust
pub struct DeviceDescriptor {
    /// 当前发现条目的临时身份，用于连接前列表 diff。
    pub id: DeviceId,
    /// 连接成功后由 phone_id/android_id 提供的稳定身份。
    pub stable_id: Option<DeviceId>,
    pub display_name: Option<String>,
    pub model: Option<String>,
    pub transport: TransportKind,
    pub transport_address: String,
    pub available: bool,
    pub adb: Option<AdbDetailDto>,
    pub usb: Option<UsbDetailDto>,
}
```

Wi-Fi 发现 ID 只表示 endpoint：

```rust
fn wifi_endpoint_id(device: &WifiDevice) -> DeviceId {
    DeviceId(format!(
        "wifi-endpoint:{}:{}:{}",
        device.instance,
        device.host,
        device.port
    ))
}
```

连接成功后，使用 `DeviceInfo.phone_id`：

```rust
fn reconcile_device_identity(
    discovered: &DeviceDescriptor,
    info: &DeviceInfoDto,
) -> DeviceDescriptor {
    let mut device = discovered.clone();
    device.stable_id = info
        .phone_id
        .as_ref()
        .filter(|id| !id.is_empty())
        .map(|id| DeviceId(format!("phone:{id}")));
    device.display_name = info.name.clone().or(device.display_name);
    device.model = info.model.clone().or(device.model);
    device
}
```

`ActiveSession` 应保存 reconcile 后的 descriptor，而不是原始发现对象。

## 3.3 身份更新事件

连接完成时或收到 `DeviceInfoChanged` 时发布：

```rust
BackendEvent::DeviceIdentityResolved {
    session_id: SessionId,
    endpoint_id: DeviceId,
    stable_id: DeviceId,
}
```

或复用已有 `DeviceUpdated`，但必须确保事件中的 `device.stable_id` 已更新。

## 3.4 测试

- DeviceInfo 全字段映射；
- Wi-Fi endpoint ID 随端口变化，不被错误当作 stable ID；
- phone_id 生成稳定 ID；
- DeviceUpdated JSON 带 stable_id；
- ADB/USB 没有 phone_id 时仍正常工作；
- Swift 列表可按 `stable_id ?? id` 做 identity。

---

# 4. 模块 D3：TrustService

## 4.1 Core 入口修正

Core 已有：

```rust
HandShakerClient::list_trusted_devices()
HandShakerClient::remove_trusted_device(device_uuid)
HandShakerClient::reset_wifi_trust(address, expected_device_uuid, options)
```

但前两项使用 `StateStore::discover()`，Application 不能保证使用 Runtime 的 `state_dir`。

建议在 Core 增加显式 StateStore 版本，并保持旧 API 兼容：

```rust
impl HandShakerClient {
    pub async fn list_trusted_devices_with_store(
        state_store: StateStore,
    ) -> Result<Vec<TrustRecordInfo>> {
        let state = state_store.load_or_create()?;
        Ok(state.trust.iter().map(|(device_uuid, record)| TrustRecordInfo {
            device_uuid: device_uuid.clone(),
            device_name: record.device_name.clone(),
            updated_at: record.updated_at,
        }).collect())
    }

    pub async fn remove_trusted_device_with_store(
        state_store: StateStore,
        device_uuid: &str,
    ) -> Result<bool> {
        state_store.remove_trust(device_uuid)
    }

    pub async fn reset_wifi_trust_with_state_store(
        address: SocketAddr,
        expected_device_uuid: &str,
        options: ClientOptions,
        state_store: StateStore,
    ) -> Result<()> {
        Self::reset_wifi_trust_with_store(
            address,
            expected_device_uuid,
            options,
            state_store,
        ).await
    }
}
```

## 4.2 Application DTO

新建 `crates/handshaker-application/src/trust.rs`：

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustRecordDto {
    pub device_id: DeviceId,
    pub device_name: Option<String>,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoveTrustRequest {
    pub device_id: DeviceId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoveTrustResult {
    pub removed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResetWifiTrustRequest {
    pub endpoint: String,
    pub expected_device_id: DeviceId,
}
```

## 4.3 Runtime 的 StateStore helper

```rust
impl HandShakerRuntime {
    fn state_store(&self) -> AppResult<handshaker_core::StateStore> {
        match &self.inner.config.state_dir {
            Some(dir) => Ok(handshaker_core::StateStore::from_dir(dir)),
            None => handshaker_core::StateStore::discover()
                .map_err(|error| from_core_error(error, "state_store")),
        }
    }
}
```

连接、trust、sync 统一使用这个 helper，避免每处复制。

## 4.4 Application 方法

```rust
impl HandShakerRuntime {
    pub async fn list_trust_records(&self) -> AppResult<Vec<TrustRecordDto>> {
        self.ensure_open()?;
        let records = HandShakerClient::list_trusted_devices_with_store(
            self.state_store()?,
        )
        .await
        .map_err(|error| from_core_error(error, "trust.list"))?;

        Ok(records.into_iter().map(|record| TrustRecordDto {
            device_id: DeviceId(format!("phone:{}", record.device_uuid)),
            device_name: record.device_name,
            updated_at_ms: record.updated_at,
        }).collect())
    }

    pub async fn remove_trust_record(
        &self,
        request: RemoveTrustRequest,
    ) -> AppResult<RemoveTrustResult> {
        self.ensure_open()?;
        let uuid = parse_phone_device_id(&request.device_id)?;
        let removed = HandShakerClient::remove_trusted_device_with_store(
            self.state_store()?,
            uuid,
        )
        .await
        .map_err(|error| from_core_error(error, "trust.remove"))?;
        Ok(RemoveTrustResult { removed })
    }

    pub async fn reset_wifi_trust(
        &self,
        request: ResetWifiTrustRequest,
    ) -> AppResult<()> {
        self.ensure_open()?;
        let address = request.endpoint.parse().map_err(|_| {
            PublicError::new(PublicErrorCode::InvalidArgument, "invalid wifi endpoint")
                .operation("trust.reset")
        })?;
        let uuid = parse_phone_device_id(&request.expected_device_id)?;

        HandShakerClient::reset_wifi_trust_with_state_store(
            address,
            uuid,
            self.client_options(),
            self.state_store()?,
        )
        .await
        .map_err(|error| from_core_error(error, "trust.reset"))
    }
}
```

## 4.5 安全要求

- DTO 不包含 derived key；
- 日志不记录 key 或状态文件内容；
- reset 必须校验手机返回 UUID；
- remove 只删除本地记录；
- reset 清除手机端信任，成功后同步删除本地记录；
- 所有操作使用 Runtime 的 `state_dir`。

## 4.6 测试

- tempdir 状态存储 list/remove；
- derived key 不出现在序列化结果；
- malformed `DeviceId` 返回 InvalidArgument；
- reset UUID mismatch 保持错误；
- Runtime shutdown 后 trust API 返回 RuntimeClosed。

---

# 5. 模块 D4：文件预检与执行计划

## 5.1 目标

CLI 目前仍承担部分：

- 源路径是文件还是目录；
- 是否要求 recursive；
- 目标是否存在；
- 覆盖冲突；
- 多个源映射到相同目标；
- 批量文件和目录统计。

这些规则必须由 Application 统一输出计划，Swift 只负责展示冲突和用户选择。

## 5.2 公共模型

新建 `crates/handshaker-application/src/file_plan.rs`：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilePlanDirection {
    Upload,
    Download,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileConflictKind {
    DestinationExists,
    DestinationTypeMismatch,
    RecursiveRequired,
    DuplicateDestination,
    SourceMissing,
    LocalPermissionDenied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilePlanConflict {
    pub kind: FileConflictKind,
    pub source: String,
    pub destination: String,
    pub message: String,
    pub overridable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilePlanItem {
    pub source: String,
    pub destination: String,
    pub is_directory: bool,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileOperationPlan {
    pub direction: FilePlanDirection,
    pub session_id: SessionId,
    pub items: Vec<FilePlanItem>,
    pub conflicts: Vec<FilePlanConflict>,
    pub file_count: u64,
    pub directory_count: u64,
    pub total_bytes: Option<u64>,
    pub requires_recursive: bool,
    pub executable: bool,
}
```

请求：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanDownloadRequest {
    pub session_id: SessionId,
    pub remote_sources: Vec<String>,
    pub local_destination: String,
    pub recursive: bool,
    pub overwrite: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanUploadRequest {
    pub session_id: SessionId,
    pub local_sources: Vec<String>,
    pub remote_destination: String,
    pub recursive: bool,
    pub overwrite: bool,
}
```

## 5.3 下载预检核心流程

```rust
pub async fn plan_download(
    &self,
    request: PlanDownloadRequest,
) -> AppResult<FileOperationPlan> {
    self.ensure_open()?;
    let session = self.session_handle(request.session_id).await?;
    let root = session.device_info().root_path.clone();

    let mut items = Vec::new();
    let mut conflicts = Vec::new();
    let mut requires_recursive = false;

    for raw_source in &request.remote_sources {
        let source = resolve_remote_path(&root, raw_source);
        let remote = self.stat_file(StatFileRequest {
            session_id: request.session_id,
            path: source.clone(),
        }).await?;

        let destination = resolve_local_download_destination(
            &request.local_destination,
            &remote,
            request.remote_sources.len(),
        )?;

        if remote.is_directory && !request.recursive {
            requires_recursive = true;
            conflicts.push(FilePlanConflict {
                kind: FileConflictKind::RecursiveRequired,
                source: source.clone(),
                destination: destination.display().to_string(),
                message: "directory download requires recursive mode".into(),
                overridable: true,
            });
        }

        inspect_local_destination(
            &destination,
            remote.is_directory,
            request.overwrite,
            &mut conflicts,
        )?;

        items.push(FilePlanItem {
            source,
            destination: destination.display().to_string(),
            is_directory: remote.is_directory,
            size: (!remote.is_directory).then_some(remote.size),
        });
    }

    append_duplicate_destination_conflicts(&items, &mut conflicts);
    Ok(finalize_file_plan(
        FilePlanDirection::Download,
        request.session_id,
        items,
        conflicts,
        requires_recursive,
    ))
}
```

## 5.4 上传预检核心流程

```rust
pub async fn plan_upload(
    &self,
    request: PlanUploadRequest,
) -> AppResult<FileOperationPlan> {
    self.ensure_open()?;
    let session = self.session_handle(request.session_id).await?;
    let root = session.device_info().root_path.clone();
    let remote_destination = resolve_remote_path(&root, &request.remote_destination);

    let remote_destination_stat = self.stat_optional(
        request.session_id,
        &remote_destination,
    ).await?;

    let mut items = Vec::new();
    let mut conflicts = Vec::new();
    let mut requires_recursive = false;

    for raw_source in &request.local_sources {
        let source = std::path::PathBuf::from(raw_source);
        let metadata = tokio::fs::metadata(&source)
            .await
            .map_err(|error| map_local_plan_error(error, raw_source))?;

        let is_directory = metadata.is_dir();
        if is_directory && !request.recursive {
            requires_recursive = true;
            conflicts.push(recursive_conflict(raw_source));
        }

        let destination = resolve_remote_upload_destination(
            &remote_destination,
            &source,
            request.local_sources.len(),
            remote_destination_stat.as_ref(),
        )?;

        if let Some(existing) = self.stat_optional(
            request.session_id,
            &destination,
        ).await? {
            append_remote_destination_conflict(
                raw_source,
                &destination,
                is_directory,
                &existing,
                request.overwrite,
                &mut conflicts,
            );
        }

        items.push(FilePlanItem {
            source: source.display().to_string(),
            destination,
            is_directory,
            size: (!is_directory).then_some(metadata.len()),
        });
    }

    append_duplicate_destination_conflicts(&items, &mut conflicts);
    Ok(finalize_file_plan(
        FilePlanDirection::Upload,
        request.session_id,
        items,
        conflicts,
        requires_recursive,
    ))
}
```

## 5.5 可选 stat helper

不要通过错误文本判断 NotFound：

```rust
async fn stat_optional(
    &self,
    session_id: SessionId,
    path: &str,
) -> AppResult<Option<FileEntryDto>> {
    match self.stat_file(StatFileRequest {
        session_id,
        path: path.to_string(),
    }).await {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.code == PublicErrorCode::RemotePathNotFound => Ok(None),
        Err(error) => Err(error),
    }
}
```

## 5.6 执行计划

不要让 Swift 自己把 plan 转回 BatchTransferRequest。

新增：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteFilePlanRequest {
    pub plan: FileOperationPlan,
    pub overwrite: bool,
    pub concurrency: usize,
}

impl HandShakerRuntime {
    pub async fn execute_file_plan(
        &self,
        request: ExecuteFilePlanRequest,
    ) -> AppResult<TransferId> {
        validate_plan_is_executable(&request.plan, request.overwrite)?;
        validate_plan_session(&request.plan, &self.inner.sessions).await?;
        // 转换为现有 batch transfer 后台任务，返回统一 TransferId。
        self.start_batch_plan(request).await
    }
}
```

建议把现有同步返回 `BatchTransferResultDto` 的方法保留给 CLI 兼容；GUI 使用 task ID。

## 5.7 测试矩阵

- 单文件下载到新路径；
- 多文件下载到目录；
- 目录未开启 recursive；
- 本地目标存在且 overwrite=false；
- 文件/目录类型冲突；
- 两个源映射到同一目标；
- 上传本地路径不存在；
- 上传本地权限不足；
- 远端目标存在；
- 多源上传目标不是目录；
- 相对远端路径不能逃逸 root；
- plan JSON round-trip；
- executable 只由不可覆盖冲突和请求选项决定。

---

# 6. 模块 D5：统一 Runtime helpers 与 CLI 迁移

Phase D 完成时应顺带减少 CLI 继续依赖 Core 的理由，但不要一次性重写 shell/watch/sync。

## 6.1 Runtime helper

```rust
impl HandShakerRuntime {
    fn client_options(&self) -> ClientOptions {
        ClientOptions {
            timeout: self.inner.config.default_timeout,
            heartbeat_interval: self.inner.config.heartbeat_interval,
            wire_log: self.inner.config.wire_log.clone(),
            adb_path: self.inner.config.adb_path.clone(),
        }
    }

    fn state_store(&self) -> AppResult<StateStore> { /* D3 */ }

    async fn session_handle(
        &self,
        session_id: SessionId,
    ) -> AppResult<Arc<ActiveSession>> {
        self.ensure_open()?;
        let session = {
            let sessions = self.inner.sessions.lock().await;
            sessions.get(&session_id).cloned()
        }.ok_or_else(session_not_found)?;
        ensure_session_ready(&session)?;
        Ok(session)
    }
}
```

避免继续复制 connect/options/state-store 逻辑。

## 6.2 last activity

当前 `last_activity_at_ms` 是普通 `u64`，请求后没有更新。改为：

```rust
last_activity_at_ms: AtomicU64,
```

请求成功或失败后都记录请求完成时间：

```rust
let result = operation(client).await;
session.last_activity_at_ms.store(now_ms(), Ordering::Relaxed);
```

snapshot：

```rust
last_activity_at_ms: Some(
    self.last_activity_at_ms.load(Ordering::Relaxed)
),
```

## 6.3 CLI 最小迁移

本阶段只迁移容易完成的调用：

- `device info` → SessionSnapshot/DeviceInfoDto；
- `device ping` → Application ping；
- `trust list/remove/reset` → TrustService；
- pull/push 预检 → `plan_download/plan_upload`；
- CLI 仍负责 TTY 确认和 human 输出。

不要在同一阶段迁移：

- watch loop；
- sync watch；
- shell/batch 循环框架。

---

# 7. 模块 D6：SyncService（独立可选子阶段）

SyncService 的风险和测试面明显大于 D1–D5。建议独立提交，Phase D 主体可以在 D1–D5 完成后先合并。

## 7.1 目标

Application 对外提供：

```text
sync status
sync plan
sync run
sync watch start/stop
```

不得让 Swift：

- 直接使用 `SyncStore`；
- 自己生成 pc_id；
- 自己维护 ledger；
- 自己 diff RemoteFile；
- 直接消费 Core FileChange。

## 7.2 DTO

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncProfileDto {
    pub id: String,
    pub session_id: SessionId,
    pub remote_root: String,
    pub local_root: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncPlanDto {
    pub profile_id: String,
    pub downloads: Vec<SyncActionDto>,
    pub metadata_updates: Vec<SyncActionDto>,
    pub deletions: Vec<SyncActionDto>,
    pub conflicts: Vec<SyncConflictDto>,
    pub total_bytes: u64,
    pub executable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncActionDto {
    pub remote_path: String,
    pub local_path: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConflictDto {
    pub remote_path: String,
    pub local_path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncStatusDto {
    pub profile_id: String,
    pub running: bool,
    pub monitoring: bool,
    pub last_run_at_ms: Option<u64>,
    pub last_error: Option<PublicError>,
}
```

## 7.3 Runtime Registry

```rust
struct RuntimeInner {
    // existing fields...
    sync_jobs: Mutex<HashMap<String, Arc<SyncJob>>>,
}

struct SyncJob {
    profile: SyncProfileDto,
    cancel: CancellationToken,
    task: Mutex<Option<JoinHandle<()>>>,
    status: RwLock<SyncStatusDto>,
}
```

## 7.4 状态目录

Sync ledger 必须使用 Runtime `state_dir`：

```text
<state_dir>/sync/<profile-id>/ledger.json
```

Application helper：

```rust
fn sync_store_for(&self, profile_id: &str) -> AppResult<SyncStore> {
    let state_dir = self.resolved_state_dir()?;
    let dir = state_dir.join("sync").join(sanitize_profile_id(profile_id)?);
    Ok(SyncStore::new(dir))
}
```

## 7.5 Plan 核心流程

```rust
pub async fn plan_sync(
    &self,
    profile: SyncProfileDto,
) -> AppResult<SyncPlanDto> {
    let session = self.session_handle(profile.session_id).await?;
    let store = self.sync_store_for(&profile.id)?;
    let ledger = store.load_or_default()?;
    let pc_id = pc_id_from_host_uuid(&self.state_store()?.load_or_create()?.host_uuid);

    let phone = session.client.photo_sync(
        &pc_id,
        &ledger.remote_files,
    ).await.map_err(|e| from_core_error(e, "sync.plan.photo_sync"))?;

    let core_plan = handshaker_core::build_sync_plan(
        &sync_config(&profile.remote_root, &profile.local_root),
        &ledger,
        &phone.files,
    )?;

    Ok(sync_plan_to_dto(core_plan, &profile))
}
```

具体函数名应根据 Core 已有 `sync.rs`/`sync_store.rs` API 调整，不要复制 diff 算法到 Application。

## 7.6 Run 核心流程

```rust
pub async fn start_sync(
    &self,
    request: StartSyncRequest,
) -> AppResult<String> {
    self.ensure_open()?;
    let plan = self.plan_sync(request.profile.clone()).await?;
    if !plan.executable {
        return Err(PublicError::new(
            PublicErrorCode::InvalidState,
            "sync plan contains unresolved conflicts",
        ).operation("sync.start"));
    }

    let job = self.register_sync_job(request.profile.clone()).await?;
    let runtime = self.clone();
    let job_id = request.profile.id.clone();
    let task = tokio::spawn(async move {
        runtime.run_sync_job(job.clone(), plan).await;
    });
    *job.task.lock().await = Some(task);
    Ok(job_id)
}
```

执行成功后原子保存 ledger；失败时旧 ledger 不变。

## 7.7 Watch

```rust
pub async fn start_sync_watch(&self, profile_id: &str) -> AppResult<()> {
    // 1. 确认 sync job 不在运行
    // 2. 对 Session 调 sync_monitor(true)
    // 3. 注册 remote folder monitor
    // 4. Application EventHub 的 RemoteFileChanged 驱动 debounce
    // 5. debounce 后触发 plan/run
}
```

需要：

- debounce；
- 同一 profile 单实例；
- disconnect/shutdown 自动停止；
- Session 失效时 SyncStatus 进入 error；
- 不在事件 bridge 内直接执行长同步任务。

## 7.8 测试

- first sync；
- empty ledger；
- changed file；
- metadata-only update；
- local conflict；
- partial failure 不覆盖 ledger；
- atomic ledger；
- watch debounce；
- duplicate start；
- shutdown cancel/join；
- state_dir 隔离。

---

# 8. 公共导出与文件结构

完成 D1–D5 后建议结构：

```text
crates/handshaker-application/src/
├── lib.rs
├── dto.rs
├── discovery.rs
├── trust.rs
├── file_plan.rs
├── runtime.rs
├── event.rs
├── error.rs
├── transfer.rs
├── media.rs
└── tests.rs
```

可选 D6：

```text
├── sync.rs
```

`lib.rs`：

```rust
mod discovery;
mod file_plan;
mod trust;
// mod sync;

pub use discovery::{DeviceDiscoveryResult, DeviceDiscoveryWarning};
pub use file_plan::{
    ExecuteFilePlanRequest, FileConflictKind, FileOperationPlan,
    FilePlanConflict, FilePlanDirection, FilePlanItem,
    PlanDownloadRequest, PlanUploadRequest,
};
pub use trust::{
    RemoveTrustRequest, RemoveTrustResult, ResetWifiTrustRequest,
    TrustRecordDto,
};
```

---

# 9. 提交拆分

建议 Agent 不要一次提交整个 Phase D。

## Commit D1

```text
feat(application): return per-transport device discovery diagnostics
```

内容：

- discovery DTO；
- `discover_devices()`；
- 兼容 `list_devices()`；
- warning tests。

## Commit D2

```text
feat(application): complete device info and stable identity reconciliation
```

内容：

- DeviceInfoDto 全字段；
- stable_id；
- identity event/update；
- fixtures。

## Commit D3

```text
feat(core,application): add state-store-aware trust service
```

内容：

- Core explicit StateStore APIs；
- Application TrustService；
- tempdir tests；
- CLI trust 最小迁移。

## Commit D4

```text
feat(application): add reusable file preflight and execution plans
```

内容：

- upload/download plan；
- conflict model；
- execution entry；
- plan fixtures/tests。

## Commit D5

```text
refactor(cli,application): route device and transfer preflight through application
```

内容：

- device info/ping；
- trust；
- pull/push preflight；
- 旧 CLI 输出兼容测试。

## Commit D6（可选）

```text
feat(application): add photo sync service and lifecycle
```

单独审查、单独真机验收。

---

# 10. 验证命令

每个 commit：

```bash
cargo fmt -- --check
cargo test -p handshaker-application
cargo clippy -p handshaker-application --all-targets -- -D warnings
```

涉及 Core：

```bash
cargo test -p handshaker-core
cargo clippy -p handshaker-core --all-targets -- -D warnings
```

完成 D1–D5：

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo build --release
scripts/generate-ffi-header.sh
scripts/run-ffi-smoke-tests.sh
git diff --check
```

Phase D 本身不增加 FFI symbol，因此 FFI ABI 仍应保持 `1.2.0`。Application 继续为 `1.0.0-preview.1`，直到 CLI 过渡入口和最终 DTO 完成收口。

---

# 11. 真机验收

D1–D5 最小真机验收：

- ADB/Wi-Fi/USB discovery 部分失败诊断；
- DeviceInfo 存储、电池、锁屏字段；
- Wi-Fi 首次连接后 stable_id；
- trust list/remove/reset；
- 单文件上传/下载 plan；
- 目录 recursive 冲突；
- overwrite 冲突；
- plan 执行与 MD5；
- disconnect/shutdown 无残留任务。

D6 追加：

- 首次照片同步；
- 增量同步；
- watch；
- 冲突；
- 中断恢复；
- ledger 原子性。

---

# 12. Phase D Definition of Done

## D1–D5 主体

- [ ] 设备发现不再吞错；
- [ ] 每个 transport 都能返回 warning；
- [ ] DeviceInfoDto 覆盖 Core 全字段；
- [ ] Wi-Fi endpoint 与 stable device identity 分离；
- [ ] TrustService 使用 Runtime state_dir；
- [ ] derived key 不跨 Application；
- [ ] 文件预检由 Application 统一完成；
- [ ] plan 能表达 recursive/overwrite/type/duplicate 冲突；
- [ ] plan 可直接进入 Application 执行；
- [ ] CLI 不再复制已迁移的预检；
- [ ] last_activity 真实更新；
- [ ] 新 DTO 有 JSON fixture；
- [ ] Workspace tests/clippy/release 全绿；
- [ ] FFI ABI 未意外变化。

## D6 SyncService

- [ ] ledger 使用 Runtime state_dir；
- [ ] plan/run/status/watch 都在 Application；
- [ ] Swift 不需要接触 Core SyncStore；
- [ ] shutdown 能取消并 join sync task；
- [ ] 真机首次/增量/watch 验收完成。

---

# 13. Agent 最终报告模板

```markdown
## 完成范围
- D1 / D2 / D3 / D4 / D5 / D6

## 公共契约变化
- Application API：
- FFI ABI：未变化 / 变化原因
- CLI JSON schema：未变化 / 变化原因

## 关键实现
- 设备发现诊断：
- 稳定身份：
- TrustService：
- 文件计划：
- SyncService：

## 验证
- cargo fmt：
- cargo test：
- cargo clippy：
- cargo build --release：
- C/Swift smoke：

## 真机
- ADB：
- Wi-Fi：
- USB：
- 文件预检/执行：
- Trust：
- Sync：

## 未完成或风险
- ...
```
