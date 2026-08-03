//! `HandShakerRuntime`: the application-service entry point. Owns the session
//! registry and (later) transfer registry + event hub. Not a global singleton;
//! one process may create several runtimes.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex;

use handshaker_core::{
    BatchTransferOptions, ClientOptions, ConnectionTarget, DeviceInfo, ErrorCode, HandShakerClient,
    RemoteFile,
};

use crate::dto::{
    ConnectRequest, CountFilesRequest, CreateDirectoryRequest, DeletePathsRequest, DeleteResultDto,
    DeviceDescriptor,
    DeviceId, DeviceInfoDto, FileEntryDto, ListDevicesRequest, ListFilesRequest, MovePathRequest,
    PingResultDto, RuntimeConfig, SessionId, SessionSnapshot, SessionState, StatFileRequest,
    TransportKind,
};
use crate::error::{AppResult, PublicError, PublicErrorCode, from_core_error};
use crate::event::{BackendEvent, EventEnvelope, EventHub};
use crate::transfer::{
    BatchTransferItemDto, BatchTransferRequest, BatchTransferResultDto, DownloadRequest,
    TransferDirectionDto, TransferFailureDto, TransferId, TransferRegistry, TransferSnapshot,
    TransferState, TreeTransferDto, UploadRequest, request_options, transfer_options,
};

use tokio::sync::broadcast;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// One open session: the core client (shared with transfer tasks) plus its
/// stable descriptors.
struct ActiveSession {
    client: Arc<HandShakerClient>,
    device: DeviceDescriptor,
    device_info: DeviceInfoDto,
    connected_at_ms: u64,
    last_activity_at_ms: u64,
    state: SessionState,
}

struct RuntimeInner {
    config: RuntimeConfig,
    sessions: Mutex<HashMap<SessionId, ActiveSession>>,
    next_session_id: AtomicU64,
    shutting_down: AtomicBool,
    event_hub: EventHub,
    transfers: Arc<TransferRegistry>,
}

/// The application service root. `create` is async by contract; callers
/// provide their own tokio runtime.
#[derive(Clone)]
pub struct HandShakerRuntime {
    inner: Arc<RuntimeInner>,
}

impl HandShakerRuntime {
    pub async fn create(config: RuntimeConfig) -> AppResult<Self> {
        if config.event_capacity == 0 {
            return Err(PublicError::new(
                PublicErrorCode::InvalidArgument,
                "event_capacity must be positive",
            ));
        }
        Ok(Self {
            inner: Arc::new(RuntimeInner {
                event_hub: EventHub::new(config.event_capacity),
                transfers: Arc::new(TransferRegistry::new()),
                config,
                sessions: Mutex::new(HashMap::new()),
                next_session_id: AtomicU64::new(1),
                shutting_down: AtomicBool::new(false),
            }),
        })
    }

    /// Idempotent: cancels transfers, closes all sessions, marks closed.
    /// New operations after shutdown return `RuntimeClosed`.
    pub async fn shutdown(&self) -> AppResult<()> {
        self.inner.shutting_down.store(true, Ordering::SeqCst);
        self.inner.event_hub.publish(BackendEvent::RuntimeStopping);
        // Cancel all transfers first so their tasks release session clients.
        for id in self
            .inner
            .transfers
            .list()
            .iter()
            .map(|snapshot| snapshot.id)
        {
            let _ = self.inner.transfers.cancel(id);
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let sessions = std::mem::take(&mut *self.inner.sessions.lock().await);
        for (_, session) in sessions {
            if let Ok(client) = Arc::try_unwrap(session.client) {
                let _ = client.close().await;
            }
        }
        Ok(())
    }

    /// Subscribe to backend events (broadcast; `Lagged` surfaced on recv).
    pub fn subscribe_events(&self) -> broadcast::Receiver<EventEnvelope> {
        self.event_hub().subscribe()
    }

    pub fn config(&self) -> &RuntimeConfig {
        &self.inner.config
    }

    fn ensure_open(&self) -> AppResult<()> {
        if self.inner.shutting_down.load(Ordering::SeqCst) {
            return Err(PublicError::new(
                PublicErrorCode::RuntimeClosed,
                "runtime is shut down",
            ));
        }
        Ok(())
    }

    /// List devices across the enabled transports.
    pub async fn list_devices(
        &self,
        request: ListDevicesRequest,
    ) -> AppResult<Vec<DeviceDescriptor>> {
        self.ensure_open()?;
        let mut devices = Vec::new();
        if request.include_adb {
            let adb_devices = HandShakerClient::list_adb_devices_with_timeout(
                &self.inner.config.adb_path,
                self.inner.config.default_timeout,
            )
            .await;
            match adb_devices {
                Ok(list) => {
                    for device in list {
                        devices.push(DeviceDescriptor {
                            id: DeviceId(device.serial.clone()),
                            display_name: Some(device.serial.clone()),
                            model: device.model.clone(),
                            transport: TransportKind::Adb,
                            transport_address: device.serial.clone(),
                            available: device.state == "device",
                            adb: Some(crate::dto::AdbDetailDto {
                                state: device.state.clone(),
                                product: device.product.clone(),
                                model: device.model.clone(),
                                device: device.device.clone(),
                            }),
                            usb: None,
                        });
                    }
                }
                Err(error) => {
                    // ADB missing/broken is a partial failure: surface as a
                    // non-fatal device-list entry only if no other transport
                    // can report; otherwise keep the error as detail-less.
                    let _ = error;
                }
            }
        }
        if request.include_wifi {
            match HandShakerClient::discover_wifi_devices(request.wifi_browse_timeout).await {
                Ok(list) => {
                    for device in list {
                        let address = device
                            .addresses
                            .first()
                            .cloned()
                            .unwrap_or_else(|| device.host.clone());
                        devices.push(DeviceDescriptor {
                            id: DeviceId(format!("wifi:{}:{}", address, device.port)),
                            display_name: Some(device.host.clone()),
                            model: None,
                            transport: TransportKind::Wifi,
                            transport_address: format!("{address}:{}", device.port),
                            available: true,
                            adb: None,
                            usb: None,
                        });
                    }
                }
                Err(_) => {}
            }
        }
        if request.include_usb {
            let accessories = handshaker_core::list_usb_accessories()
                .map_err(|error| from_core_error(error, "list_devices"))?;
            for accessory in accessories {
                devices.push(DeviceDescriptor {
                    id: DeviceId(accessory.location.clone()),
                    display_name: accessory
                        .serial
                        .clone()
                        .or(Some(accessory.location.clone())),
                    model: None,
                    transport: TransportKind::UsbAccessory,
                    transport_address: format!(
                        "0x{:04x}:0x{:04x}",
                        accessory.vendor_id, accessory.product_id
                    ),
                    available: true,
                    adb: None,
                    usb: Some(crate::dto::UsbDetailDto {
                        bus_number: accessory.bus_number,
                        serial: accessory.serial.clone(),
                        vendor_id: accessory.vendor_id,
                        product_id: accessory.product_id,
                        mode: format!("{:?}", accessory.mode),
                    }),
                });
            }
        }
        Ok(devices)
    }

    /// Open a session for the requested device.
    pub async fn connect(&self, request: ConnectRequest) -> AppResult<SessionId> {
        self.ensure_open()?;
        let target = connection_target_for(&request.device)?;
        let options = ClientOptions {
            timeout: self.inner.config.default_timeout,
            heartbeat_interval: self.inner.config.heartbeat_interval,
            wire_log: self.inner.config.wire_log.clone(),
            adb_path: self.inner.config.adb_path.clone(),
        };
        let client = HandShakerClient::connect(target, options)
            .await
            .map_err(|error| from_core_error(error, "connect"))?;
        let device_info = client.device_info().clone();
        let id = SessionId(self.inner.next_session_id.fetch_add(1, Ordering::SeqCst));
        let session = ActiveSession {
            client: Arc::new(client),
            device: request.device.clone(),
            device_info: device_info_to_dto(&device_info),
            connected_at_ms: now_ms(),
            last_activity_at_ms: now_ms(),
            state: SessionState::Ready,
        };
        // Insert under the session lock and re-check shutdown so a racing
        // shutdown cannot leave this session orphaned (shutdown takes the
        // same lock after flipping the flag).
        {
            let mut guard = self.inner.sessions.lock().await;
            if self.inner.shutting_down.load(Ordering::SeqCst) {
                return Err(PublicError::new(
                    PublicErrorCode::RuntimeClosed,
                    "runtime is shut down",
                ));
            }
            guard.insert(id, session);
        }
        let snapshot = self.get_session_snapshot(id).await?;
        self.inner
            .event_hub
            .publish(BackendEvent::SessionStateChanged(snapshot));
        Ok(id)
    }

    pub async fn disconnect(&self, session_id: SessionId) -> AppResult<()> {
        self.ensure_open()?;
        let mut guard = self.inner.sessions.lock().await;
        let session = guard.remove(&session_id).ok_or_else(|| {
            PublicError::new(PublicErrorCode::SessionNotFound, "session not found")
        })?;
        drop(guard);
        let result = match Arc::try_unwrap(session.client) {
            Ok(client) => client
                .close()
                .await
                .map_err(|error| from_core_error(error, "disconnect")),
            // A transfer task still borrows the client; the connection is torn
            // down when the last Arc drops (transport stream close).
            Err(_) => Ok(()),
        };
        self.inner
            .event_hub
            .publish(BackendEvent::SessionStateChanged(SessionSnapshot {
                id: session_id,
                device: session.device,
                device_info: session.device_info,
                state: SessionState::Closed,
                connected_at_ms: session.connected_at_ms,
                last_activity_at_ms: Some(session.last_activity_at_ms),
            }));
        result
    }

    pub async fn get_session_snapshot(&self, session_id: SessionId) -> AppResult<SessionSnapshot> {
        self.ensure_open()?;
        let guard = self.inner.sessions.lock().await;
        let session = guard.get(&session_id).ok_or_else(|| {
            PublicError::new(PublicErrorCode::SessionNotFound, "session not found")
        })?;
        Ok(SessionSnapshot {
            id: session_id,
            device: session.device.clone(),
            device_info: session.device_info.clone(),
            state: session.state,
            connected_at_ms: session.connected_at_ms,
            last_activity_at_ms: Some(session.last_activity_at_ms),
        })
    }

    /// Ping an open session; returns round-trip latency in milliseconds.
    pub async fn ping(&self, session_id: SessionId) -> AppResult<PingResultDto> {
        self.ensure_open()?;
        let client = self.session_client(session_id).await?;
        client
            .ping()
            .await
            .map(|result| PingResultDto {
                round_trip_ms: result.round_trip_ms as u64,
            })
            .map_err(|error| from_core_error(error, "ping"))
    }

    /// List one directory level (or `depth` levels) for an open session.
    pub async fn list_files(&self, request: ListFilesRequest) -> AppResult<Vec<FileEntryDto>> {
        self.ensure_open()?;
        let guard = self.inner.sessions.lock().await;
        let session = guard.get(&request.session_id).ok_or_else(|| {
            PublicError::new(PublicErrorCode::SessionNotFound, "session not found")
        })?;
        let root = session.client.root_path().to_string();
        let path = resolve_remote_path(&root, &request.path);
        let files = session
            .client
            .list_dir(&path, request.depth)
            .await
            .map_err(|error| from_core_error(error, "list_files"))?;
        Ok(files.into_iter().map(remote_file_to_dto).collect())
    }

    /// Count files under a remote directory (protocol exclusions passthrough).
    pub async fn count_files(&self, request: CountFilesRequest) -> AppResult<u64> {
        self.ensure_open()?;
        let guard = self.inner.sessions.lock().await;
        let session = guard.get(&request.session_id).ok_or_else(|| {
            PublicError::new(PublicErrorCode::SessionNotFound, "session not found")
        })?;
        let root = session.client.root_path().to_string();
        let path = resolve_remote_path(&root, &request.path);
        session
            .client
            .file_count(&path, request.depth, request.exclusions)
            .await
            .map_err(|error| from_core_error(error, "count_files"))
    }

    // ---- file service (M8 §5.5) ----

    /// Stat one remote path; `None` when the phone reports it missing.
    pub async fn stat_file(&self, request: StatFileRequest) -> AppResult<Option<FileEntryDto>> {
        self.ensure_open()?;
        let guard = self.inner.sessions.lock().await;
        let session = guard.get(&request.session_id).ok_or_else(|| {
            PublicError::new(PublicErrorCode::SessionNotFound, "session not found")
        })?;
        let root = session.client.root_path().to_string();
        let path = resolve_remote_path(&root, &request.path);
        session
            .client
            .stat(&path)
            .await
            .map(|file| file.map(remote_file_to_dto))
            .map_err(|error| from_core_error(error, "stat_file"))
    }

    pub async fn create_directory(&self, request: CreateDirectoryRequest) -> AppResult<()> {
        self.ensure_open()?;
        let guard = self.inner.sessions.lock().await;
        let session = guard.get(&request.session_id).ok_or_else(|| {
            PublicError::new(PublicErrorCode::SessionNotFound, "session not found")
        })?;
        let root = session.client.root_path().to_string();
        let path = resolve_remote_path(&root, &request.path);
        session
            .client
            .create_dir(&path)
            .await
            .map(|_| ())
            .map_err(|error| from_core_error(error, "create_directory"))
    }

    pub async fn move_path(&self, request: MovePathRequest) -> AppResult<()> {
        self.ensure_open()?;
        let guard = self.inner.sessions.lock().await;
        let session = guard.get(&request.session_id).ok_or_else(|| {
            PublicError::new(PublicErrorCode::SessionNotFound, "session not found")
        })?;
        let root = session.client.root_path().to_string();
        let source = resolve_remote_path(&root, &request.source);
        let target = resolve_remote_path(&root, &request.target);
        session
            .client
            .rename(&source, &target)
            .await
            .map_err(|error| from_core_error(error, "move_path"))
    }

    pub async fn delete_paths(&self, request: DeletePathsRequest) -> AppResult<DeleteResultDto> {
        self.ensure_open()?;
        let guard = self.inner.sessions.lock().await;
        let session = guard.get(&request.session_id).ok_or_else(|| {
            PublicError::new(PublicErrorCode::SessionNotFound, "session not found")
        })?;
        let root = session.client.root_path().to_string();
        let paths: Vec<String> = request
            .paths
            .iter()
            .map(|path| resolve_remote_path(&root, path))
            .collect();
        let options = handshaker_core::DeleteOptions {
            trash: request.trash,
            sync: request.sync,
        };
        let deleted = session
            .client
            .delete(&paths, options)
            .await
            .map_err(|error| from_core_error(error, "delete_paths"))?;
        Ok(DeleteResultDto {
            deleted: deleted.into_iter().map(remote_file_to_dto).collect(),
        })
    }

    // ---- transfers ----

    /// Start a background download; returns the transfer id immediately.
    pub async fn start_download(&self, request: DownloadRequest) -> AppResult<TransferId> {
        self.ensure_open()?;
        let client = self.session_client(request.session_id).await?;
        let remote = resolve_remote_path(&client.root_path(), &request.remote_path);
        let snapshot = self.inner.transfers.into_snapshot_for(
            request.session_id,
            TransferDirectionDto::Download,
            remote.clone(),
            request.local_path.display().to_string(),
        );
        let id = snapshot.id;
        let entry = self.inner.transfers.register(snapshot);
        let registry = self.inner.transfers.clone();
        let event_hub = self.event_hub();
        let options = transfer_options(registry.clone(), id, request.overwrite);
        let token = request_options(entry.cancel.clone());
        let local = request.local_path;
        let handle = tokio::spawn(async move {
            registry.transition(id, TransferState::Running);
            let result = client
                .download_with_options(&remote, &local, options, token)
                .await;
            match result {
                Ok(bytes) => {
                    registry.set_progress(id, bytes);
                    if let Some(snapshot) = registry.transition(id, TransferState::Completed) {
                        event_hub.publish(BackendEvent::TransferUpdated(snapshot));
                    }
                }
                Err(error) => {
                    if matches!(error.code(), ErrorCode::Cancelled | ErrorCode::Interrupted) {
                        registry.transition(id, TransferState::Cancelled);
                    } else {
                        registry.set_error(id, from_core_error(error, "download"));
                        registry.transition(id, TransferState::Failed);
                    }
                    if let Some(snapshot) = registry.get(id).ok() {
                        event_hub.publish(BackendEvent::TransferUpdated(snapshot));
                    }
                }
            }
        });
        *entry
            .join
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(handle);
        Ok(id)
    }

    /// Start a background upload; returns the transfer id immediately.
    pub async fn start_upload(&self, request: UploadRequest) -> AppResult<TransferId> {
        self.ensure_open()?;
        let client = self.session_client(request.session_id).await?;
        let remote = resolve_remote_path(&client.root_path(), &request.remote_path);
        let snapshot = self.inner.transfers.into_snapshot_for(
            request.session_id,
            TransferDirectionDto::Upload,
            request.local_path.display().to_string(),
            remote.clone(),
        );
        let id = snapshot.id;
        let entry = self.inner.transfers.register(snapshot);
        let registry = self.inner.transfers.clone();
        let event_hub = self.event_hub();
        let options = transfer_options(registry.clone(), id, request.overwrite);
        let token = request_options(entry.cancel.clone());
        let local = request.local_path;
        let handle = tokio::spawn(async move {
            registry.transition(id, TransferState::Running);
            let result = client
                .upload_with_options(&local, &remote, options, token)
                .await;
            match result {
                Ok(bytes) => {
                    registry.set_progress(id, bytes);
                    if let Some(snapshot) = registry.transition(id, TransferState::Completed) {
                        event_hub.publish(BackendEvent::TransferUpdated(snapshot));
                    }
                }
                Err(error) => {
                    if matches!(error.code(), ErrorCode::Cancelled | ErrorCode::Interrupted) {
                        registry.transition(id, TransferState::Cancelled);
                    } else {
                        registry.set_error(id, from_core_error(error, "upload"));
                        registry.transition(id, TransferState::Failed);
                    }
                    if let Some(snapshot) = registry.get(id).ok() {
                        event_hub.publish(BackendEvent::TransferUpdated(snapshot));
                    }
                }
            }
        });
        *entry
            .join
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(handle);
        Ok(id)
    }

    pub async fn cancel_transfer(&self, id: TransferId) -> AppResult<()> {
        self.ensure_open()?;
        self.inner.transfers.cancel(id)
    }

    /// Transfer a batch of resolved file pairs and directory trees (serial
    /// execution; per-file failures are aggregated, never aborting the
    /// remaining files). Remote sources (download) and remote targets
    /// (upload) are resolved against the device root here, so the API is
    /// consistent with the single-file methods; tree enumeration and
    /// path-escape hardening stay in the core `download_tree` implementation.
    pub async fn batch_download(
        &self,
        request: BatchTransferRequest,
    ) -> AppResult<BatchTransferResultDto> {
        self.ensure_open()?;
        let client = self.session_client(request.session_id).await?;
        let root = client.root_path().to_string();
        let files: Vec<BatchTransferItemDto> = request
            .files
            .iter()
            .map(|item| BatchTransferItemDto {
                source: crate::resolve_remote_path(&root, &item.source),
                target: item.target.clone(),
            })
            .collect();
        let trees: Vec<TreeTransferDto> = request
            .trees
            .iter()
            .map(|tree| TreeTransferDto {
                source: crate::resolve_remote_path(&root, &tree.source),
                target: tree.target.clone(),
            })
            .collect();
        let mut result = handshaker_core::BatchTransferResult::default();
        for tree in &trees {
            let partial = client
                .download_tree(
                    &tree.source,
                    std::path::Path::new(&tree.target),
                    batch_options(&request),
                )
                .await
                .map_err(|error| from_core_error(error, "batch_download"))?;
            merge_core_batch(&mut result, partial);
        }
        let partial = client
            .download_many(&core_batch_items(&files), batch_options(&request))
            .await
            .map_err(|error| from_core_error(error, "batch_download"))?;
        merge_core_batch(&mut result, partial);
        Ok(batch_result_to_dto(result))
    }

    /// Upload a batch of resolved file pairs and directory trees (serial
    /// execution; per-file failures are aggregated, never aborting the
    /// remaining files). Remote targets are resolved against the device
    /// root; tree enumeration stays in the CLI (local filesystem) and
    /// `upload_tree` handles the remote mirroring.
    pub async fn batch_upload(
        &self,
        request: BatchTransferRequest,
    ) -> AppResult<BatchTransferResultDto> {
        self.ensure_open()?;
        let client = self.session_client(request.session_id).await?;
        let root = client.root_path().to_string();
        let files: Vec<BatchTransferItemDto> = request
            .files
            .iter()
            .map(|item| BatchTransferItemDto {
                source: item.source.clone(),
                target: crate::resolve_remote_path(&root, &item.target),
            })
            .collect();
        let trees: Vec<TreeTransferDto> = request
            .trees
            .iter()
            .map(|tree| TreeTransferDto {
                source: tree.source.clone(),
                target: crate::resolve_remote_path(&root, &tree.target),
            })
            .collect();
        let mut result = handshaker_core::BatchTransferResult::default();
        for tree in &trees {
            let partial = client
                .upload_tree(
                    std::path::Path::new(&tree.source),
                    &tree.target,
                    batch_options(&request),
                )
                .await
                .map_err(|error| from_core_error(error, "batch_upload"))?;
            merge_core_batch(&mut result, partial);
        }
        let partial = client
            .upload_many(&core_batch_items(&files), batch_options(&request))
            .await
            .map_err(|error| from_core_error(error, "batch_upload"))?;
        merge_core_batch(&mut result, partial);
        Ok(batch_result_to_dto(result))
    }

    pub async fn get_transfer(&self, id: TransferId) -> AppResult<TransferSnapshot> {
        self.ensure_open()?;
        self.inner.transfers.get(id)
    }

    pub async fn list_transfers(&self) -> AppResult<Vec<TransferSnapshot>> {
        self.ensure_open()?;
        self.inner.transfers.reap();
        Ok(self.inner.transfers.list())
    }

    /// Transition API for the M8 CLI migration: hands a not-yet-migrated
    /// command (or an in-flight transfer task) the core client of an open
    /// session without opening a second connection. Leaks a core type on
    /// purpose and is removed once the CLI migration is complete (before the
    /// v1 freeze is lifted).
    pub async fn session_client(&self, session_id: SessionId) -> AppResult<Arc<HandShakerClient>> {
        let guard = self.inner.sessions.lock().await;
        guard
            .get(&session_id)
            .map(|session| session.client.clone())
            .ok_or_else(|| PublicError::new(PublicErrorCode::SessionNotFound, "session not found"))
    }

    fn event_hub(&self) -> EventHub {
        self.inner.event_hub.clone()
    }
}

/// Resolve a possibly-relative remote path against the device root, exactly
/// once (application layer owns this rule; GUI must not reimplement it).
///
/// Semantics:
/// - absolute inputs are normalized (`..` cannot escape above `/`);
/// - relative inputs are joined under `root` and **clamped to `root`**: a
///   path whose normalized form would leave the root is pinned back to root;
/// - empty / "." resolve to root.
pub fn resolve_remote_path(root: &str, path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "." {
        return root.to_string();
    }
    if trimmed.starts_with('/') {
        return normalize_remote_path(trimmed);
    }
    let joined = format!("{root}/{trimmed}");
    let normalized = normalize_remote_path(&joined);
    // Clamp: relative paths must stay inside the device root. When root is
    // the filesystem root itself ("/") there is nothing above it to escape.
    if root == "/" || root.is_empty() {
        return normalized;
    }
    let prefix = format!("{root}/");
    if normalized == root || normalized.starts_with(&prefix) {
        return normalized;
    }
    root.to_string()
}

/// Collapse "." and resolve ".." textually; never escapes above root
/// (a path escaping the root is clamped to root).
pub fn normalize_remote_path(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    let mut result = String::from("/");
    result.push_str(&parts.join("/"));
    result
}

fn connection_target_for(device: &DeviceDescriptor) -> AppResult<ConnectionTarget> {
    match device.transport {
        TransportKind::Adb => Ok(ConnectionTarget::Adb {
            serial: Some(device.id.0.clone()),
        }),
        TransportKind::Wifi => {
            let address = device.transport_address.parse().map_err(|_| {
                PublicError::new(PublicErrorCode::InvalidArgument, "invalid wifi address")
            })?;
            Ok(ConnectionTarget::Wifi { address })
        }
        TransportKind::UsbAccessory => Ok(ConnectionTarget::Usb {
            location_id: Some(device.id.0.clone()),
        }),
    }
}

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
    }
}

pub(crate) fn remote_file_to_dto(file: RemoteFile) -> FileEntryDto {
    FileEntryDto {
        path: file.path,
        size: file.size,
        created_at_ms: file.created_at,
        modified_at_ms: file.modified_at,
        is_directory: file.is_directory,
        checksum: file.checksum,
        is_trash: file.is_trash,
        media_id: file.id,
    }
}

/// Convert application batch items to core batch items.
fn core_batch_items(items: &[BatchTransferItemDto]) -> Vec<handshaker_core::BatchTransferItem> {
    items
        .iter()
        .map(|item| handshaker_core::BatchTransferItem {
            source: item.source.clone(),
            target: item.target.clone(),
        })
        .collect()
}

/// Serial, overwrite-permitting core batch options (no progress callback; the
/// CLI renders the aggregated result after completion).
fn batch_options(request: &BatchTransferRequest) -> BatchTransferOptions {
    BatchTransferOptions {
        overwrite: request.overwrite,
        progress: None,
        offset: 0,
        concurrency: 1,
    }
}

/// Merge one partial core batch result into an accumulator.
fn merge_core_batch(
    target: &mut handshaker_core::BatchTransferResult,
    partial: handshaker_core::BatchTransferResult,
) {
    target.ok.extend(partial.ok);
    target.failures.extend(partial.failures);
}

/// Convert a core batch result onto the application DTO.
pub(crate) fn batch_result_to_dto(
    result: handshaker_core::BatchTransferResult,
) -> BatchTransferResultDto {
    BatchTransferResultDto {
        ok: result
            .ok
            .iter()
            .map(|item| BatchTransferItemDto {
                source: item.source.clone(),
                target: item.target.clone(),
            })
            .collect(),
        failures: result
            .failures
            .iter()
            .map(|failure| TransferFailureDto {
                source: failure.source.clone(),
                target: failure.target.clone(),
                message: failure.message.clone(),
            })
            .collect(),
    }
}
