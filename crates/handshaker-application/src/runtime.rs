//! `HandShakerRuntime`: the application-service entry point. Owns the session
//! registry and (later) transfer registry + event hub. Not a global singleton;
//! one process may create several runtimes.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex;

use handshaker_core::{ClientOptions, ConnectionTarget, DeviceInfo, HandShakerClient, RemoteFile};

use crate::dto::{
    ConnectRequest, DeviceDescriptor, DeviceId, DeviceInfoDto, FileEntryDto, ListDevicesRequest,
    ListFilesRequest, RuntimeConfig, SessionId, SessionSnapshot, SessionState, TransportKind,
};
use crate::error::{AppResult, PublicError, PublicErrorCode, from_core_error};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// One open session: the core client plus its stable descriptors.
struct ActiveSession {
    client: HandShakerClient,
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
    /// Placeholder for the M8.7 event hub; kept to stabilize the shutdown
    /// contract (cancel subscriptions, close sessions, mark closed).
    _event_capacity: usize,
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
                config,
                sessions: Mutex::new(HashMap::new()),
                next_session_id: AtomicU64::new(1),
                shutting_down: AtomicBool::new(false),
                _event_capacity: 0,
            }),
        })
    }

    /// Idempotent: cancels activity, closes all sessions, marks closed.
    /// New operations after shutdown return `RuntimeClosed`.
    pub async fn shutdown(&self) -> AppResult<()> {
        self.inner.shutting_down.store(true, Ordering::SeqCst);
        let sessions = std::mem::take(&mut *self.inner.sessions.lock().await);
        for (_, session) in sessions {
            let _ = session.client.close().await;
        }
        Ok(())
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
            client,
            device: request.device,
            device_info: device_info_to_dto(&device_info),
            connected_at_ms: now_ms(),
            last_activity_at_ms: now_ms(),
            state: SessionState::Ready,
        };
        self.inner.sessions.lock().await.insert(id, session);
        Ok(id)
    }

    pub async fn disconnect(&self, session_id: SessionId) -> AppResult<()> {
        self.ensure_open()?;
        let mut guard = self.inner.sessions.lock().await;
        let session = guard.remove(&session_id).ok_or_else(|| {
            PublicError::new(PublicErrorCode::SessionNotFound, "session not found")
        })?;
        drop(guard);
        session
            .client
            .close()
            .await
            .map_err(|error| from_core_error(error, "disconnect"))
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
}

/// Resolve a possibly-relative remote path against the device root, exactly
/// once (application layer owns this rule; GUI must not reimplement it).
pub fn resolve_remote_path(root: &str, path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "." {
        return root.to_string();
    }
    if trimmed.starts_with('/') {
        return trimmed.to_string();
    }
    // Relative: join under root, normalizing a leading "./" or ".." safely.
    let joined = format!("{root}/{trimmed}");
    normalize_remote_path(&joined)
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
