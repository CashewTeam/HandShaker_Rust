//! `HandShakerRuntime`: the application-service entry point. Owns the session
//! registry and (later) transfer registry + event hub. Not a global singleton;
//! one process may create several runtimes.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex;

use handshaker_core::{
    BatchTransferOptions, ClientEvent, ClientOptions, ConnectionTarget, DeviceInfo, ErrorCode,
    EventCallbacks, EventFilter, EventStreamError, EventSubscription, HandShakerClient, MediaItem,
    MediaKind, MediaLibraryChange, RemoteFile,
};

use crate::dto::{
    ClipboardEntryDto, ConnectRequest, CountFilesRequest, CreateDirectoryRequest,
    DeletePathsRequest, DeleteResultDto, DeviceDescriptor, DeviceId, DeviceInfoDto, FileEntryDto,
    ListDevicesRequest, ListFilesRequest, MediaChangeDto, MediaChangeItemDto, MediaKindDto,
    MovePathRequest, PingResultDto, RemoteFileChangeDto, RemoteFileChangeKind, RuntimeConfig,
    SessionId, SessionSnapshot, SessionState, StatFileRequest, TransportKind,
};
use crate::error::{AppResult, PublicError, PublicErrorCode, from_core_error};
use crate::event::{BackendEvent, EventEnvelope, EventHub};
use crate::media::{
    AudioAlbumDto, AudioLibraryDto, ExifDataDto, ImageFileDto, PhotoLibraryDto, ThumbnailsDto,
    VideoFileDto, VideoLibraryDto, dto_to_audio_album, dto_to_image_file, dto_to_video_file,
};
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

/// Bounded wait for transfer tasks to release the session client during a
/// deterministic close (M8.1 Phase B / B2). A fixed constant on purpose:
/// adding a config knob would require real configuration plumbing.
const SESSION_CLOSE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// One open session: the core client (shared with transfer tasks) plus its
/// stable descriptors. Kept behind an `Arc` so short registry critical
/// sections can hand out the session (or its client) and drop the registry
/// lock before any network await (M8.1 Phase B / B1).
pub(crate) struct ActiveSession {
    client: Arc<HandShakerClient>,
    device: DeviceDescriptor,
    /// Device info DTO, refreshed in place by the event bridge when the
    /// phone pushes `DeviceInfoChanged` (M8.1 Phase C / C1). Shared with the
    /// bridge task via `Arc`; the bridge never holds the session `Arc`, so
    /// `close_session`'s `Arc::try_unwrap` still succeeds.
    device_info: Arc<std::sync::RwLock<DeviceInfoDto>>,
    connected_at_ms: u64,
    last_activity_at_ms: u64,
    /// `SessionState` discriminant (1=Connecting ..=5=Failed); transitions
    /// are atomic so disconnect/shutdown never race on it.
    state: AtomicU8,
    /// Bridge task forwarding core typed events to the runtime EventHub;
    /// ends when the core session's event sender drops (explicit close), or
    /// is aborted when another client owner remains (M8.1 Phase C / C1).
    event_task: tokio::task::JoinHandle<()>,
}

impl ActiveSession {
    fn snapshot(&self, id: SessionId, state: SessionState) -> SessionSnapshot {
        SessionSnapshot {
            id,
            device: self.device.clone(),
            device_info: self
                .device_info
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
            state,
            connected_at_ms: self.connected_at_ms,
            last_activity_at_ms: Some(self.last_activity_at_ms),
        }
    }
}

fn session_state_from_u8(value: u8) -> SessionState {
    match value {
        1 => SessionState::Connecting,
        2 => SessionState::Ready,
        3 => SessionState::Disconnecting,
        4 => SessionState::Closed,
        _ => SessionState::Failed,
    }
}

struct RuntimeInner {
    config: RuntimeConfig,
    sessions: Mutex<HashMap<SessionId, Arc<ActiveSession>>>,
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
        if config.transfer_history_capacity == 0 {
            return Err(PublicError::new(
                PublicErrorCode::InvalidArgument,
                "transfer_history_capacity must be positive",
            ));
        }
        let event_hub = EventHub::new(config.event_capacity);
        Ok(Self {
            inner: Arc::new(RuntimeInner {
                event_hub: event_hub.clone(),
                transfers: Arc::new(TransferRegistry::new(
                    event_hub,
                    config.transfer_history_capacity,
                    config.transfer_history_ttl,
                )),
                config,
                sessions: Mutex::new(HashMap::new()),
                next_session_id: AtomicU64::new(1),
                shutting_down: AtomicBool::new(false),
            }),
        })
    }

    /// Idempotent shutdown (M8.1 Phase B / B3): runs the deterministic close
    /// path exactly once, in order:
    /// 1. atomically mark shutting down (racing/duplicate calls return Ok);
    /// 2. publish `RuntimeStopping`;
    /// 3. cancel all transfers so their tasks release session clients;
    /// 4. close every session in parallel via the deterministic close path
    ///    (cancel session transfers, bounded join, explicit core close);
    /// 5. close the event hub so subscribers observe `Closed`;
    /// 6. no fixed sleeps — task completion is proven by joining, not by
    ///    waiting. New operations after shutdown return `RuntimeClosed`.
    pub async fn shutdown(&self) -> AppResult<()> {
        if self
            .inner
            .shutting_down
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            // Already shutting down (or shut down): idempotent no-op.
            return Ok(());
        }
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
        // Deterministically close every session in parallel (bounded joins
        // inside close_session prove the tasks finished).
        let sessions = std::mem::take(&mut *self.inner.sessions.lock().await);
        let mut close_tasks = Vec::new();
        for (id, session) in sessions {
            let this = self.clone();
            close_tasks.push(tokio::spawn(async move {
                let _ = this.close_session(session, id).await;
            }));
        }
        for task in close_tasks {
            let _ = task.await;
        }
        // Close the hub last: subscribers drain remaining events, then recv
        // returns Closed.
        self.inner.event_hub.close();
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
        if request.include_wifi
            && let Ok(list) =
                HandShakerClient::discover_wifi_devices(request.wifi_browse_timeout).await
        {
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
        // state_dir must really control where trust records and the host
        // UUID live (M8.1 Phase B / B4): explicit dir when configured,
        // otherwise the core default config directory.
        let state_store = match &self.inner.config.state_dir {
            Some(dir) => handshaker_core::StateStore::from_dir(dir),
            None => handshaker_core::StateStore::discover()
                .map_err(|error| from_core_error(error, "connect"))?,
        };
        let client = HandShakerClient::connect_with_state(
            target,
            options,
            state_store,
            EventCallbacks {
                // Enable every phone-side push so the bridge below delivers
                // device-info, clipboard and media changes (M8.1 Phase C/C1).
                device_info: true,
                photo_library: true,
                audio_library: true,
                video_library: true,
            },
        )
        .await
        .map_err(|error| from_core_error(error, "connect"))?;
        let device_info = client.device_info().clone();
        let id = SessionId(self.inner.next_session_id.fetch_add(1, Ordering::SeqCst));
        let device_info_shared = Arc::new(std::sync::RwLock::new(device_info_to_dto(&device_info)));
        // Forward core typed events to the runtime EventHub for the whole
        // session lifetime (M8.1 Phase C / C1). The task captures only the
        // descriptor + shared device-info DTO + a Weak RuntimeInner, never
        // the session or client Arc, so `close_session`'s `Arc::try_unwrap`
        // still works. The Weak runtime lets an unexpected event-stream close
        // mark an otherwise-idle session as failed.
        let event_task = tokio::spawn(run_event_bridge(
            client.subscribe_events(EventFilter::all()),
            id,
            request.device.clone(),
            device_info_shared.clone(),
            self.inner.event_hub.clone(),
            Arc::downgrade(&self.inner),
        ));
        let session = Arc::new(ActiveSession {
            client: Arc::new(client),
            device: request.device.clone(),
            device_info: device_info_shared,
            connected_at_ms: now_ms(),
            last_activity_at_ms: now_ms(),
            state: AtomicU8::new(SessionState::Ready as u8),
            event_task,
        });
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
            .publish(BackendEvent::SessionStateChanged(Box::new(snapshot)));
        Ok(id)
    }

    pub async fn disconnect(&self, session_id: SessionId) -> AppResult<()> {
        self.ensure_open()?;
        let session = {
            let mut guard = self.inner.sessions.lock().await;
            guard.remove(&session_id).ok_or_else(|| {
                PublicError::new(PublicErrorCode::SessionNotFound, "session not found")
            })?
        };
        self.close_session(session, session_id).await
    }

    /// Deterministic session close (M8.1 Phase B / B2), shared by
    /// `disconnect` and `shutdown`:
    /// 1. atomically enter `Disconnecting` (terminal states are idempotent);
    /// 2. reject new work — the session is already out of the registry, so
    ///    new requests fail with `SessionNotFound`;
    /// 3. cancel the session's transfers and wait (bounded) for their tasks
    ///    to release the shared client;
    /// 4. close the core client explicitly when this caller is the last
    ///    owner (sends QUIT); otherwise surface a `Warning` — the connection
    ///    is torn down by the last `Arc` drop (transport abort, no QUIT);
    /// 5. publish the final `Closed` event.
    async fn close_session(
        &self,
        session: Arc<ActiveSession>,
        session_id: SessionId,
    ) -> AppResult<()> {
        // Terminal states are final: no second close, no state regression.
        let mut observed = session.state.load(Ordering::SeqCst);
        loop {
            if observed == SessionState::Closed as u8 || observed == SessionState::Failed as u8 {
                return Ok(());
            }
            match session.state.compare_exchange(
                observed,
                SessionState::Disconnecting as u8,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(actual) => observed = actual,
            }
        }

        // Cancel the session's transfers; wait bounded for the tasks so they
        // release their client Arcs before the explicit close below.
        let joins = self.inner.transfers.cancel_for_session(session_id);
        let mut aborted_transfer_tasks = 0usize;
        for mut join in joins {
            if tokio::time::timeout(SESSION_CLOSE_DEADLINE, &mut join)
                .await
                .is_err()
            {
                // A deadline is not proof that a task stopped. Abort it and
                // await the cancellation so no transfer task can keep the
                // client alive after disconnect/shutdown returns.
                join.abort();
                let _ = join.await;
                aborted_transfer_tasks += 1;
            }
        }

        // Snapshot fields before consuming the session for the explicit close.
        let closed_snapshot = session.snapshot(session_id, SessionState::Closed);
        let mut warning = (aborted_transfer_tasks > 0).then(|| {
            PublicError::new(
                PublicErrorCode::Internal,
                format!("aborted {aborted_transfer_tasks} transfer task(s) after close deadline"),
            )
        });
        let mut close_error: Option<PublicError> = None;
        match Arc::try_unwrap(session) {
            Ok(active) => {
                let event_task = active.event_task;
                match Arc::try_unwrap(active.client) {
                    Ok(client) => {
                        if let Err(error) = client.close().await {
                            // The transport is already aborted by core close;
                            // QUIT delivery failure is a best-effort warning.
                            close_error = Some(from_core_error(error, "disconnect"));
                        }
                        // The core session's event sender dropped with the
                        // client, so the bridge task receives `Closed` and
                        // exits; wait bounded so shutdown never leaks it.
                        let _ = tokio::time::timeout(SESSION_CLOSE_DEADLINE, event_task).await;
                    }
                    Err(_) => {
                        // Another owner (e.g. the CLI migration's
                        // session_client handle) still borrows the client.
                        // The connection is torn down by the last Arc drop
                        // (transport abort; no QUIT). Stop the bridge now so
                        // it cannot keep publishing for a session that is
                        // already out of the registry.
                        event_task.abort();
                        // This is observable, not silent.
                        warning = Some(PublicError::new(
                            PublicErrorCode::Internal,
                            "session client still borrowed; connection closed by last owner",
                        ));
                    }
                }
            }
            Err(_) => {
                // Unreachable in practice: the session was removed from the
                // registry under the lock, so this caller is the only
                // ActiveSession owner. Kept observable instead of silent.
                warning = Some(PublicError::new(
                    PublicErrorCode::Internal,
                    "session close raced with another closer; connection closed by last owner",
                ));
            }
        }
        self.inner
            .event_hub
            .publish(BackendEvent::SessionStateChanged(Box::new(closed_snapshot)));
        if let Some(error) = close_error {
            warning = Some(error);
        }
        if let Some(warning) = warning {
            self.inner.event_hub.publish(BackendEvent::Warning(warning));
        }
        Ok(())
    }

    pub async fn get_session_snapshot(&self, session_id: SessionId) -> AppResult<SessionSnapshot> {
        self.ensure_open()?;
        let guard = self.inner.sessions.lock().await;
        let session = guard.get(&session_id).ok_or_else(|| {
            PublicError::new(PublicErrorCode::SessionNotFound, "session not found")
        })?;
        Ok(session.snapshot(
            session_id,
            session_state_from_u8(session.state.load(Ordering::SeqCst)),
        ))
    }

    /// Ping an open session; returns round-trip latency in milliseconds.
    pub async fn ping(&self, session_id: SessionId) -> AppResult<PingResultDto> {
        self.request(session_id, "ping", |client| async move {
            client.ping().await.map(|result| PingResultDto {
                round_trip_ms: result.round_trip_ms as u64,
            })
        })
        .await
    }

    /// List one directory level (or `depth` levels) for an open session.
    pub async fn list_files(&self, request: ListFilesRequest) -> AppResult<Vec<FileEntryDto>> {
        self.request(request.session_id, "list_files", |client| async move {
            let root = client.root_path().to_string();
            let path = resolve_remote_path(&root, &request.path);
            client
                .list_dir(&path, request.depth)
                .await
                .map(|files| files.into_iter().map(remote_file_to_dto).collect())
        })
        .await
    }

    /// Count files under a remote directory (protocol exclusions passthrough).
    pub async fn count_files(&self, request: CountFilesRequest) -> AppResult<u64> {
        self.request(request.session_id, "count_files", |client| async move {
            let root = client.root_path().to_string();
            let path = resolve_remote_path(&root, &request.path);
            client
                .file_count(&path, request.depth, request.exclusions)
                .await
        })
        .await
    }

    // ---- file service (M8 §5.5) ----

    /// Stat one remote path; `None` when the phone reports it missing.
    pub async fn stat_file(&self, request: StatFileRequest) -> AppResult<Option<FileEntryDto>> {
        self.request(request.session_id, "stat_file", |client| async move {
            let root = client.root_path().to_string();
            let path = resolve_remote_path(&root, &request.path);
            client
                .stat(&path)
                .await
                .map(|file| file.map(remote_file_to_dto))
        })
        .await
    }

    pub async fn create_directory(&self, request: CreateDirectoryRequest) -> AppResult<()> {
        self.request(
            request.session_id,
            "create_directory",
            |client| async move {
                let root = client.root_path().to_string();
                let path = resolve_remote_path(&root, &request.path);
                client.create_dir(&path).await.map(|_| ())
            },
        )
        .await
    }

    pub async fn move_path(&self, request: MovePathRequest) -> AppResult<()> {
        self.request(request.session_id, "move_path", |client| async move {
            let root = client.root_path().to_string();
            let source = resolve_remote_path(&root, &request.source);
            let target = resolve_remote_path(&root, &request.target);
            client.rename(&source, &target).await
        })
        .await
    }

    pub async fn delete_paths(&self, request: DeletePathsRequest) -> AppResult<DeleteResultDto> {
        self.request(request.session_id, "delete_paths", |client| async move {
            let root = client.root_path().to_string();
            let paths: Vec<String> = request
                .paths
                .iter()
                .map(|path| resolve_remote_path(&root, path))
                .collect();
            let options = handshaker_core::DeleteOptions {
                trash: request.trash,
                sync: request.sync,
            };
            client
                .delete(&paths, options)
                .await
                .map(|deleted| DeleteResultDto {
                    deleted: deleted.into_iter().map(remote_file_to_dto).collect(),
                })
        })
        .await
    }

    // ---- clipboard (M8 §5.5-adjacent service) ----

    /// List clipboard history for an open session.
    pub async fn list_clipboards(
        &self,
        session_id: SessionId,
    ) -> AppResult<Vec<ClipboardEntryDto>> {
        self.request(session_id, "list_clipboards", |client| async move {
            client.clipboard_list().await.map(|entries| {
                entries
                    .into_iter()
                    .map(|entry| ClipboardEntryDto {
                        text: entry.text,
                        timestamp_ms: entry.timestamp_ms,
                    })
                    .collect()
            })
        })
        .await
    }

    /// Write one clipboard entry.
    pub async fn set_clipboard(&self, session_id: SessionId, text: &str) -> AppResult<()> {
        let text = text.to_string();
        self.request(session_id, "set_clipboard", |client| async move {
            client.clipboard_set(&text).await
        })
        .await
    }

    /// Delete one clipboard entry by timestamp.
    pub async fn delete_clipboard(
        &self,
        session_id: SessionId,
        timestamp_ms: i64,
    ) -> AppResult<()> {
        self.request(session_id, "delete_clipboard", |client| async move {
            client.clipboard_delete(timestamp_ms).await
        })
        .await
    }

    /// Clear all clipboard history.
    pub async fn clear_clipboards(&self, session_id: SessionId) -> AppResult<()> {
        self.request(session_id, "clear_clipboards", |client| async move {
            client.clipboard_clear().await
        })
        .await
    }

    // ---- media library (mirrors core media domain, DTOs in media.rs) ----

    /// Snapshot of the phone photo library.
    pub async fn get_photo_library(&self, session_id: SessionId) -> AppResult<PhotoLibraryDto> {
        self.request(session_id, "get_photo_library", |client| async move {
            client.get_photo_library().await.map(Into::into)
        })
        .await
    }

    /// Snapshot of the phone video library.
    pub async fn get_video_library(&self, session_id: SessionId) -> AppResult<VideoLibraryDto> {
        self.request(session_id, "get_video_library", |client| async move {
            client.get_video_library().await.map(Into::into)
        })
        .await
    }

    /// Snapshot of the phone audio library.
    pub async fn get_audio_library(&self, session_id: SessionId) -> AppResult<AudioLibraryDto> {
        self.request(session_id, "get_audio_library", |client| async move {
            client.get_audio_library().await.map(Into::into)
        })
        .await
    }

    /// Fetch thumbnails for requested media entries.
    pub async fn get_thumbnails(
        &self,
        session_id: SessionId,
        images: &[ImageFileDto],
        videos: &[VideoFileDto],
        audio_albums: &[AudioAlbumDto],
    ) -> AppResult<ThumbnailsDto> {
        let images = images.iter().map(dto_to_image_file).collect::<Vec<_>>();
        let videos = videos.iter().map(dto_to_video_file).collect::<Vec<_>>();
        let audio_albums = audio_albums
            .iter()
            .map(dto_to_audio_album)
            .collect::<Vec<_>>();
        self.request(session_id, "get_thumbnails", |client| async move {
            client
                .get_thumbnails(&images, &videos, &audio_albums)
                .await
                .map(Into::into)
        })
        .await
    }

    /// Fetch EXIF metadata for one remote media path.
    pub async fn fetch_exif(&self, session_id: SessionId, path: &str) -> AppResult<ExifDataDto> {
        let path = path.to_string();
        self.request(session_id, "fetch_exif", |client| async move {
            client.fetch_exif(&path).await.map(Into::into)
        })
        .await
    }

    // ---- transfers ----

    /// Start a background download; returns the transfer id immediately.
    pub async fn start_download(&self, request: DownloadRequest) -> AppResult<TransferId> {
        self.ensure_open()?;
        let client = self.session_client(request.session_id).await?;
        let remote = resolve_remote_path(client.root_path(), &request.remote_path);
        let snapshot = self.inner.transfers.snapshot_for(
            request.session_id,
            TransferDirectionDto::Download,
            remote.clone(),
            request.local_path.display().to_string(),
        );
        let id = snapshot.id;
        let entry = self.inner.transfers.register(snapshot);
        let registry = self.inner.transfers.clone();
        let event_hub = self.event_hub();
        let inner = self.inner.clone();
        let session_id = request.session_id;
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
                    registry.set_progress(id, bytes, bytes);
                    if let Some(snapshot) = registry.transition(id, TransferState::Completed) {
                        event_hub.publish(BackendEvent::TransferUpdated(snapshot));
                    }
                }
                Err(error) => {
                    let connection_closed = transfer_closed_the_session(&error);
                    if matches!(error.code(), ErrorCode::Cancelled | ErrorCode::Interrupted) {
                        registry.transition(id, TransferState::Cancelled);
                    } else {
                        registry.set_error(id, from_core_error(error, "download"));
                        registry.transition(id, TransferState::Failed);
                    }
                    if let Ok(snapshot) = registry.get(id) {
                        event_hub.publish(BackendEvent::TransferUpdated(snapshot));
                    }
                    // Download cancellation (and transport loss) closes the
                    // core session; reflect that in the application state so
                    // GUI stops using a dead session (M8.1 Phase C / C3).
                    if connection_closed {
                        mark_connection_lost(
                            &inner.sessions,
                            &inner.transfers,
                            session_id,
                            &event_hub,
                        )
                        .await;
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
        let remote = resolve_remote_path(client.root_path(), &request.remote_path);
        let snapshot = self.inner.transfers.snapshot_for(
            request.session_id,
            TransferDirectionDto::Upload,
            request.local_path.display().to_string(),
            remote.clone(),
        );
        let id = snapshot.id;
        let entry = self.inner.transfers.register(snapshot);
        let registry = self.inner.transfers.clone();
        let event_hub = self.event_hub();
        let inner = self.inner.clone();
        let session_id = request.session_id;
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
                    registry.set_progress(id, bytes, bytes);
                    if let Some(snapshot) = registry.transition(id, TransferState::Completed) {
                        event_hub.publish(BackendEvent::TransferUpdated(snapshot));
                    }
                }
                Err(error) => {
                    let connection_closed = transfer_closed_the_session(&error);
                    if matches!(error.code(), ErrorCode::Cancelled | ErrorCode::Interrupted) {
                        registry.transition(id, TransferState::Cancelled);
                    } else {
                        registry.set_error(id, from_core_error(error, "upload"));
                        registry.transition(id, TransferState::Failed);
                    }
                    if let Ok(snapshot) = registry.get(id) {
                        event_hub.publish(BackendEvent::TransferUpdated(snapshot));
                    }
                    // A transport-level failure means the connection is gone;
                    // reflect it in the application session state.
                    if connection_closed {
                        mark_connection_lost(
                            &inner.sessions,
                            &inner.transfers,
                            session_id,
                            &event_hub,
                        )
                        .await;
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
        self.session_client_arc(session_id).await
    }

    /// Short critical section: clone the session's client Arc, then drop the
    /// registry lock before any network await (M8.1 Phase B / B1).
    async fn session_client_arc(&self, session_id: SessionId) -> AppResult<Arc<HandShakerClient>> {
        let guard = self.inner.sessions.lock().await;
        let session = guard.get(&session_id).ok_or_else(|| {
            PublicError::new(PublicErrorCode::SessionNotFound, "session not found")
        })?;
        // M8.1 Phase C / C5: once a connection loss (or explicit close) has
        // marked the session non-Ready, later requests fail fast instead of
        // waiting on a dead transport. The registry entry stays until
        // disconnect/shutdown removes it, so the stable error here is
        // SessionClosed, not SessionNotFound.
        let state = session_state_from_u8(session.state.load(Ordering::SeqCst));
        if !matches!(state, SessionState::Ready) {
            return Err(PublicError::new(
                PublicErrorCode::SessionClosed,
                "session is not ready",
            ));
        }
        Ok(session.client.clone())
    }

    /// Run one request against a session's client with connection-loss
    /// detection (M8.1 Phase C / C5): when the core call fails because the
    /// connection died, the session is marked `Failed`, its transfers are
    /// cancelled, and `ConnectionLost` + `SessionStateChanged(Failed)` events
    /// are published; later requests then fail fast with `SessionClosed`.
    async fn request<T, F, Fut>(
        &self,
        session_id: SessionId,
        operation: &'static str,
        call: F,
    ) -> AppResult<T>
    where
        F: FnOnce(Arc<HandShakerClient>) -> Fut,
        Fut: std::future::Future<Output = handshaker_core::Result<T>>,
    {
        self.ensure_open()?;
        let client = self.session_client_arc(session_id).await?;
        match call(client).await {
            Ok(value) => Ok(value),
            Err(error) => {
                let public = from_core_error(error, operation);
                if connection_lost_code(public.code) {
                    mark_connection_lost(
                        &self.inner.sessions,
                        &self.inner.transfers,
                        session_id,
                        &self.inner.event_hub,
                    )
                    .await;
                }
                Err(public)
            }
        }
    }

    fn event_hub(&self) -> EventHub {
        self.inner.event_hub.clone()
    }
}

/// Does this transfer error imply the underlying core session is dead?
/// Download cancellation explicitly closes the session (bare-stream
/// receive); transport errors mean the connection is gone (M8.1 Phase C/C3).
fn transfer_closed_the_session(error: &handshaker_core::Error) -> bool {
    use handshaker_core::Error;
    match error {
        Error::Cancelled(info) => info.connection_closed,
        other => matches!(other.code(), ErrorCode::Transport),
    }
}

/// Reflect a core session that died underneath a request or transfer in the
/// application session state (M8.1 Phase C / C5):
/// 1. `Ready` → `Failed` (terminal; no-op when already closing/closed/gone);
/// 2. cancel the session's remaining transfers (terminal events first);
/// 3. publish `ConnectionLost`, then the final `SessionStateChanged(Failed)`.
pub(crate) async fn mark_connection_lost(
    sessions: &tokio::sync::Mutex<HashMap<SessionId, Arc<ActiveSession>>>,
    transfers: &TransferRegistry,
    session_id: SessionId,
    event_hub: &EventHub,
) {
    let snapshot = {
        let guard = sessions.lock().await;
        let Some(session) = guard.get(&session_id) else {
            return;
        };
        // CAS `Ready → Failed` (same pattern as close_session) so a racing
        // disconnect can never be overwritten: if the observed state is no
        // longer Ready, someone else already transitioned the session.
        let mut observed = session.state.load(Ordering::SeqCst);
        loop {
            if observed != SessionState::Ready as u8 {
                return;
            }
            match session.state.compare_exchange(
                observed,
                SessionState::Failed as u8,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(actual) => observed = actual,
            }
        }
        // The bridge task must not keep broadcasting clipboard/media events
        // for a dead session with no owner left to stop it (security review
        // MEDIUM): abort it now; the normal close path keeps its own
        // bounded join/abort in `close_session`.
        session.event_task.abort();
        session.snapshot(session_id, SessionState::Failed)
    };
    transfers.cancel_for_session(session_id);
    event_hub.publish(BackendEvent::ConnectionLost { session_id });
    event_hub.publish(BackendEvent::SessionStateChanged(Box::new(snapshot)));
}

/// Does a public error mean the underlying connection is gone? Only the
/// core `Transport` family (mapped to `ConnectFailed`) proves the session
/// can no longer serve requests. `ConnectionLost` also appears for core
/// `Timeout` — a request may time out while the connection stays usable
/// (slow phone, large response), so a timeout alone must NOT kill the
/// session or cancel its transfers (M8.1 Phase C / C5).
pub(crate) fn connection_lost_code(code: PublicErrorCode) -> bool {
    matches!(code, PublicErrorCode::ConnectFailed)
}

/// Forward core typed events to the runtime EventHub until the core session
/// closes: the subscription ends with `Closed` once the session's event
/// sender drops (explicit close or transport teardown). Lag and unknown
/// events never panic the hub — they map to safe `Warning`s.
async fn run_event_bridge(
    mut subscription: EventSubscription,
    session_id: SessionId,
    device: DeviceDescriptor,
    device_info: Arc<std::sync::RwLock<DeviceInfoDto>>,
    event_hub: EventHub,
    runtime: std::sync::Weak<RuntimeInner>,
) {
    loop {
        match subscription.recv().await {
            Ok(event) => event_hub.publish(bridge_client_event(
                event,
                session_id,
                &device,
                &device_info,
            )),
            Err(EventStreamError::Lagged { missed }) => {
                event_hub.publish(BackendEvent::Warning(PublicError::new(
                    PublicErrorCode::InvalidState,
                    format!("core event stream lagged; missed {missed} events"),
                )));
            }
            Err(EventStreamError::Closed) => {
                // During explicit disconnect/shutdown the session has already
                // been removed from the registry, so this is a no-op. If the
                // phone/transport disappears while idle, the entry is still
                // present and must transition Ready -> Failed immediately.
                if let Some(runtime) = runtime.upgrade() {
                    mark_connection_lost(
                        &runtime.sessions,
                        &runtime.transfers,
                        session_id,
                        &runtime.event_hub,
                    )
                    .await;
                }
                return;
            }
        }
    }
}

/// Map one core `ClientEvent` to a stable backend event (M8.1 Phase C / C1).
/// Device-info changes also refresh the session's cached DTO in place so
/// `SessionSnapshot.device_info` stays current. Never leaks protobuf types
/// or wire payloads across the application boundary.
pub(crate) fn bridge_client_event(
    event: ClientEvent,
    session_id: SessionId,
    device: &DeviceDescriptor,
    device_info: &std::sync::RwLock<DeviceInfoDto>,
) -> BackendEvent {
    match event {
        ClientEvent::DeviceInfoChanged(info) => {
            let dto = device_info_to_dto(&info);
            if let Ok(mut guard) = device_info.write() {
                *guard = dto;
            }
            BackendEvent::DeviceUpdated {
                session_id,
                device: device.clone(),
            }
        }
        ClientEvent::ClipboardChanged(entries) => BackendEvent::ClipboardChanged {
            session_id,
            entries: entries
                .into_iter()
                .map(|entry| ClipboardEntryDto {
                    text: entry.text,
                    timestamp_ms: entry.timestamp_ms,
                })
                .collect(),
        },
        ClientEvent::MediaLibraryChanged(change) => BackendEvent::MediaChanged {
            session_id,
            change: media_change_to_dto(change),
        },
        ClientEvent::DirectoryChanged(events) => BackendEvent::RemoteFileChanged {
            session_id,
            change: RemoteFileChangeDto {
                change_kind: RemoteFileChangeKind::DirectoryChanged,
                paths: events
                    .into_iter()
                    .filter_map(|event| event.file.map(|file| file.path))
                    .collect(),
            },
        },
        ClientEvent::FileChanged(changes) => BackendEvent::RemoteFileChanged {
            session_id,
            change: RemoteFileChangeDto {
                change_kind: RemoteFileChangeKind::FileChanged,
                paths: changes
                    .into_iter()
                    .filter_map(|change| change.file.map(|file| file.path))
                    .collect(),
            },
        },
        ClientEvent::PhotoSyncChanged(change) => BackendEvent::RemoteFileChanged {
            session_id,
            change: RemoteFileChangeDto {
                change_kind: RemoteFileChangeKind::PhotoSyncChanged,
                paths: change.files.into_iter().map(|file| file.path).collect(),
            },
        },
        ClientEvent::SyncMonitorChanged(_) => BackendEvent::RemoteFileChanged {
            session_id,
            change: RemoteFileChangeDto {
                change_kind: RemoteFileChangeKind::SyncMonitorChanged,
                paths: Vec::new(),
            },
        },
        ClientEvent::RequestCancelled(_) => BackendEvent::Warning(PublicError::new(
            PublicErrorCode::RemoteCancelled,
            "phone requested cancellation outside an active request",
        )),
        ClientEvent::Unknown(unknown) => BackendEvent::Warning(PublicError::new(
            PublicErrorCode::ProtocolError,
            format!(
                "unclassified phone event: sid={} reason={:?}",
                unknown.sid, unknown.reason
            ),
        )),
    }
}

/// Normalize a core media library change into the stable change DTO.
fn media_change_to_dto(change: MediaLibraryChange) -> MediaChangeDto {
    let map_item = |item: MediaItem| MediaChangeItemDto {
        media_id: item.media_id,
        path: item.path,
        size: item.size,
        created_at: item.created_at,
        modified_at: item.modified_at,
        mime_type: item.mime_type,
        title: item.title,
        album_name: item.album_name,
    };
    MediaChangeDto {
        media_kind: match change.kind {
            MediaKind::Photo => MediaKindDto::Photo,
            MediaKind::Video => MediaKindDto::Video,
            MediaKind::Audio => MediaKindDto::Audio,
        },
        added: change.added.into_iter().map(map_item).collect(),
        deleted: change.deleted.into_iter().map(map_item).collect(),
        updated: change.updated.into_iter().map(map_item).collect(),
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
