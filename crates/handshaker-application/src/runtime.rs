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
    EventCallbacks, EventFilter, EventKind, EventStreamError, EventSubscription, FileChange,
    FileChangeStatus, HandShakerClient, MediaItem, MediaKind, MediaLibraryChange, RemoteFile,
};

use crate::discovery::{
    DeviceDiscoveryResult, DeviceDiscoveryWarning, adb_device_to_descriptor,
    deduplicate_discovered_devices, sort_discovered_devices, usb_device_to_descriptor,
    wifi_device_to_descriptor,
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
use crate::file_plan::{
    ExecuteFilePlanRequest, FileConflictKind, FileOperationPlan, FilePlanConflict,
    FilePlanDirection, FilePlanItem, PlanDownloadRequest, PlanUploadRequest,
};
use crate::media::{
    AudioAlbumDto, AudioLibraryDto, ExifDataDto, ImageFileDto, PhotoLibraryDto, ThumbnailsDto,
    VideoFileDto, VideoLibraryDto, dto_to_audio_album, dto_to_image_file, dto_to_video_file,
};
use crate::sync::{
    SyncJob, SyncLedgerStatusDto, SyncPlanDto, SyncProfileDto, SyncRunResultDto, SyncStatusDto,
    one_entry_diff, snapshot_to_remote_files, sync_plan_to_dto, sync_run_result_to_dto,
};
use crate::transfer::{
    BatchTransferItemDto, BatchTransferRequest, BatchTransferResultDto, DownloadRequest,
    TransferDirectionDto, TransferFailureDto, TransferId, TransferRegistry, TransferSnapshot,
    TransferState, TreeTransferDto, UploadRequest, request_options, transfer_options,
};
use crate::trust::{
    RemoveTrustRequest, RemoveTrustResult, ResetWifiTrustRequest, TrustRecordDto,
    parse_phone_device_id,
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

/// Bounded wait for sync run/watch tasks during stop and shutdown (Phase D /
/// D6). Core `execute_plan` has no cooperative cancellation mid-download, so
/// a deadline plus abort is the only guarantee; fixed on purpose, mirroring
/// `SESSION_CLOSE_DEADLINE`.
const SYNC_STOP_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// Watch debounce window (Phase D / D6): a debounced batch of file-change
/// events is applied at most once per window after the last event.
const SYNC_WATCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(500);

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
    /// Completion time of the most recent request on this session (Phase D /
    /// D5 §6.2): stored after every request finishes, so the snapshot value
    /// reflects real activity instead of the connection time.
    last_activity_at_ms: AtomicU64,
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
            last_activity_at_ms: Some(self.last_activity_at_ms.load(Ordering::Relaxed)),
        }
    }

    /// Current device-info DTO (shared with the event bridge).
    pub(crate) fn device_info(&self) -> DeviceInfoDto {
        self.device_info
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Record real activity on this session (Phase D / D5 §6.2): called
    /// after every request finishes (success or failure) and when long
    /// transfers start, so `last_activity_at_ms` reflects actual use.
    pub(crate) fn record_activity(&self) {
        self.last_activity_at_ms.store(now_ms(), Ordering::Relaxed);
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
    /// Photo-sync jobs keyed by profile id (Phase D / D6). Jobs are created
    /// by `start_sync`/`register_sync_job`, queried by `get_sync_status`,
    /// and removed by `stop_sync` / shutdown.
    sync_jobs: Mutex<HashMap<String, Arc<SyncJob>>>,
}

/// Everything the plan mapping and the executor need, in core types. Never
/// crosses the application boundary (Phase D / D6).
struct SyncPipeline {
    client: Arc<HandShakerClient>,
    config: handshaker_core::SyncConfig,
    snapshot: handshaker_core::SyncSnapshot,
    phone_files: Vec<RemoteFile>,
    diff: handshaker_core::SyncDiff,
    conflicts: Vec<String>,
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
                sync_jobs: Mutex::new(HashMap::new()),
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
        // Cancel all sync jobs (run + watch) so their tasks release session
        // clients too (Phase D / D6). Bounded joins; a task that ignores
        // cancellation is aborted, mirroring the transfer handling above.
        let sync_jobs: Vec<Arc<SyncJob>> = {
            let guard = self.inner.sync_jobs.lock().await;
            guard.values().cloned().collect()
        };
        let mut sync_joins = Vec::new();
        for job in sync_jobs {
            job.cancel.cancel();
            if let Some(task) = job.task.lock().await.take() {
                sync_joins.push(task);
            }
            if let Some(task) = job.watch_task.lock().await.take() {
                sync_joins.push(task);
            }
        }
        for mut join in sync_joins {
            if tokio::time::timeout(SYNC_STOP_DEADLINE, &mut join)
                .await
                .is_err()
            {
                join.abort();
                let _ = join.await;
            }
        }
        self.inner.sync_jobs.lock().await.clear();
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

    /// Core options derived from this runtime's configuration (Phase D / D5
    /// helper): every connection, reset and future sync uses the configured
    /// timeout, heartbeat, wire log and adb path.
    fn client_options(&self) -> ClientOptions {
        ClientOptions {
            timeout: self.inner.config.default_timeout,
            heartbeat_interval: self.inner.config.heartbeat_interval,
            wire_log: self.inner.config.wire_log.clone(),
            adb_path: self.inner.config.adb_path.clone(),
        }
    }

    /// State store rooted at the runtime's configured `state_dir` (Phase D /
    /// D3): trust, host UUID and future sync ledgers must all live where the
    /// caller configured them. Without a configured dir the core default
    /// config directory is used.
    fn state_store(&self) -> AppResult<handshaker_core::StateStore> {
        match &self.inner.config.state_dir {
            Some(dir) => Ok(handshaker_core::StateStore::from_dir(dir)),
            None => handshaker_core::StateStore::discover()
                .map_err(|error| from_core_error(error, "state_store")),
        }
    }

    // ---- trust service (Phase D / D3) ----

    /// List locally persisted WiFi trust records. Derived keys never cross
    /// the application boundary.
    pub async fn list_trust_records(&self) -> AppResult<Vec<TrustRecordDto>> {
        self.ensure_open()?;
        let records = HandShakerClient::list_trusted_devices_with_store(self.state_store()?)
            .await
            .map_err(|error| from_core_error(error, "trust.list"))?;
        Ok(records
            .into_iter()
            .map(|record| TrustRecordDto {
                device_id: DeviceId(format!("phone:{}", record.device_uuid)),
                device_name: record.device_name,
                updated_at_ms: record.updated_at.saturating_mul(1000),
            })
            .collect())
    }

    /// Remove the local trust record for one device. Only the local record is
    /// touched; the phone-side trust is cleared with [`Self::reset_wifi_trust`].
    pub async fn remove_trust_record(
        &self,
        request: RemoveTrustRequest,
    ) -> AppResult<RemoveTrustResult> {
        self.ensure_open()?;
        let uuid = parse_phone_device_id(&request.device_id)?;
        let removed = HandShakerClient::remove_trusted_device_with_store(self.state_store()?, uuid)
            .await
            .map_err(|error| from_core_error(error, "trust.remove"))?;
        Ok(RemoveTrustResult { removed })
    }

    /// Clear the phone-side WiFi trust for a device, then delete the local
    /// record. The connected phone must report the expected UUID; a mismatch
    /// aborts the reset instead of touching a different device.
    pub async fn reset_wifi_trust(&self, request: ResetWifiTrustRequest) -> AppResult<()> {
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

    /// Discover devices across the enabled transports with per-transport
    /// diagnostics (Phase D / D1). A single broken transport never fails the
    /// whole sweep: its failure is reported as a structured warning next to
    /// the devices other transports found. Whole-request errors (runtime
    /// closed) still return `Err`.
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
        sort_discovered_devices(&mut devices);

        Ok(DeviceDiscoveryResult { devices, warnings })
    }

    /// Preview-period compatibility wrapper over [`Self::discover_devices`]:
    /// returns only the devices, dropping per-transport warnings. Removed
    /// before the v1 freeze once callers have migrated to `discover_devices`.
    pub async fn list_devices(
        &self,
        request: ListDevicesRequest,
    ) -> AppResult<Vec<DeviceDescriptor>> {
        Ok(self.discover_devices(request).await?.devices)
    }

    /// Open a session for the requested device.
    pub async fn connect(&self, request: ConnectRequest) -> AppResult<SessionId> {
        self.ensure_open()?;
        let target = connection_target_for(&request.device)?;
        let options = self.client_options();
        // state_dir must really control where trust records and the host
        // UUID live (M8.1 Phase B / B4 + Phase D / D3): explicit dir when
        // configured, otherwise the core default config directory.
        let state_store = self.state_store()?;
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
        // Reconcile the discovery entry with the connected phone's identity
        // (Phase D / D2): phone_id becomes the stable id, name/model are
        // backfilled. The session and every event it publishes carry the
        // reconciled descriptor, so `stable_id` is present from the start.
        let device_info_dto = device_info_to_dto(&device_info);
        let device_info_shared = Arc::new(std::sync::RwLock::new(device_info_dto.clone()));
        let reconciled_device = reconcile_device_identity(&request.device, &device_info_dto);
        // Forward core typed events to the runtime EventHub for the whole
        // session lifetime (M8.1 Phase C / C1). The task captures only the
        // descriptor + shared device-info DTO + a Weak RuntimeInner, never
        // the session or client Arc, so `close_session`'s `Arc::try_unwrap`
        // still works. The Weak runtime lets an unexpected event-stream close
        // mark an otherwise-idle session as failed.
        let event_task = tokio::spawn(run_event_bridge(
            client.subscribe_events(EventFilter::all()),
            id,
            reconciled_device.clone(),
            device_info_shared.clone(),
            self.inner.event_hub.clone(),
            Arc::downgrade(&self.inner),
        ));
        let session = Arc::new(ActiveSession {
            client: Arc::new(client),
            device: reconciled_device,
            device_info: device_info_shared,
            connected_at_ms: now_ms(),
            last_activity_at_ms: AtomicU64::new(now_ms()),
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

    /// Register or unregister a phone-side directory monitor (Phase D /
    /// D6-adjacent): the phone pushes `FileChanged` events for the path
    /// while the monitor is on; the events arrive over the runtime event
    /// hub as `BackendEvent::RemoteFileChanged`.
    pub async fn monitor_folder(
        &self,
        session_id: SessionId,
        path: String,
        enabled: bool,
    ) -> AppResult<()> {
        self.request(session_id, "monitor_folder", |client| async move {
            let root = client.root_path().to_string();
            let path = resolve_remote_path(&root, &path);
            client.monitor_folder(&path, enabled).await
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

    // ---- file plans (Phase D / D4) ----

    /// Preflight a download batch: source type, recursive requirement,
    /// destination resolution and overwrite/type conflicts are all decided
    /// here; GUI only renders the plan and the user's choices.
    pub async fn plan_download(
        &self,
        request: PlanDownloadRequest,
    ) -> AppResult<FileOperationPlan> {
        self.ensure_open()?;
        let session = self.session_handle(request.session_id).await?;
        let root = session.device_info().root_path;

        let mut items = Vec::new();
        let mut conflicts = Vec::new();
        let mut requires_recursive = false;

        for raw_source in &request.remote_sources {
            let source = resolve_remote_path(&root, raw_source);
            let remote = self
                .stat_file(StatFileRequest {
                    session_id: request.session_id,
                    path: source.clone(),
                })
                .await?;

            let Some(remote) = remote else {
                conflicts.push(FilePlanConflict {
                    kind: FileConflictKind::SourceMissing,
                    source: source.clone(),
                    destination: request.local_destination.clone(),
                    message: "remote source does not exist".into(),
                    overridable: false,
                });
                continue;
            };

            if remote.is_directory && !request.recursive {
                requires_recursive = true;
                conflicts.push(FilePlanConflict {
                    kind: FileConflictKind::RecursiveRequired,
                    source: source.clone(),
                    destination: request.local_destination.clone(),
                    message: "directory download requires recursive mode".into(),
                    overridable: true,
                });
            }

            let Some(destination) = resolve_local_download_destination(
                &request.local_destination,
                &remote,
                request.remote_sources.len(),
                &mut conflicts,
            ) else {
                continue;
            };

            inspect_local_destination(
                &destination,
                remote.is_directory,
                request.overwrite,
                &mut conflicts,
            );

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

    /// Preflight an upload batch (mirror of [`Self::plan_download`]).
    pub async fn plan_upload(&self, request: PlanUploadRequest) -> AppResult<FileOperationPlan> {
        self.ensure_open()?;
        let session = self.session_handle(request.session_id).await?;
        let root = session.device_info().root_path;
        let remote_destination = resolve_remote_path(&root, &request.remote_destination);
        let remote_destination_stat = self
            .stat_optional(request.session_id, &remote_destination)
            .await?;

        let mut items = Vec::new();
        let mut conflicts = Vec::new();
        let mut requires_recursive = false;

        for raw_source in &request.local_sources {
            let source = std::path::PathBuf::from(raw_source);
            let metadata = match tokio::fs::metadata(&source).await {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    conflicts.push(FilePlanConflict {
                        kind: FileConflictKind::SourceMissing,
                        source: raw_source.clone(),
                        destination: remote_destination.clone(),
                        message: "local source does not exist".into(),
                        overridable: false,
                    });
                    continue;
                }
                Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                    conflicts.push(FilePlanConflict {
                        kind: FileConflictKind::LocalPermissionDenied,
                        source: raw_source.clone(),
                        destination: remote_destination.clone(),
                        message: "local source is not readable".into(),
                        overridable: false,
                    });
                    continue;
                }
                Err(error) => {
                    return Err(map_local_plan_error(error, raw_source));
                }
            };

            let is_directory = metadata.is_dir();
            if is_directory && !request.recursive {
                requires_recursive = true;
                conflicts.push(FilePlanConflict {
                    kind: FileConflictKind::RecursiveRequired,
                    source: raw_source.clone(),
                    destination: remote_destination.clone(),
                    message: "directory upload requires recursive mode".into(),
                    overridable: true,
                });
            }

            let Some(destination) = resolve_remote_upload_destination(
                &remote_destination,
                &source,
                request.local_sources.len(),
                remote_destination_stat.as_ref(),
                &mut conflicts,
            ) else {
                continue;
            };

            if let Some(existing) = self.stat_optional(request.session_id, &destination).await? {
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

    /// Stat one remote path as optional: `RemotePathNotFound` becomes
    /// `None`, everything else propagates (Phase D / D4; never parse error
    /// text).
    async fn stat_optional(
        &self,
        session_id: SessionId,
        path: &str,
    ) -> AppResult<Option<FileEntryDto>> {
        match self
            .stat_file(StatFileRequest {
                session_id,
                path: path.to_string(),
            })
            .await
        {
            Ok(file) => Ok(file),
            Err(error) if error.code == PublicErrorCode::RemotePathNotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Execute a preflighted plan as a background transfer; returns the
    /// unified transfer id immediately. The plan's session must still be
    /// open, and the caller's `overwrite` must match the plan's conflicts.
    pub async fn execute_file_plan(
        &self,
        request: ExecuteFilePlanRequest,
    ) -> AppResult<TransferId> {
        self.ensure_open()?;
        if !request.plan.is_executable_with(request.overwrite) {
            return Err(PublicError::new(
                PublicErrorCode::InvalidState,
                "plan contains unresolved conflicts for the given options",
            )
            .operation("execute_file_plan"));
        }
        self.session_handle(request.plan.session_id).await?;
        self.start_batch_plan(request).await
    }

    /// Convert an executable plan into a background batch transfer task with
    /// a unified transfer id (files -> `download_many`/`upload_many`,
    /// directories -> `download_tree`/`upload_tree`, serial with the given
    /// concurrency; failures are aggregated, never aborting the rest).
    async fn start_batch_plan(&self, request: ExecuteFilePlanRequest) -> AppResult<TransferId> {
        let session = self.session_handle(request.plan.session_id).await?;
        session.record_activity();
        let client = session.client.clone();
        let direction = match request.plan.direction {
            FilePlanDirection::Download => TransferDirectionDto::Download,
            FilePlanDirection::Upload => TransferDirectionDto::Upload,
        };
        let mut snapshot = self.inner.transfers.snapshot_for(
            request.plan.session_id,
            direction,
            request
                .plan
                .items
                .iter()
                .map(|item| item.source.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            request
                .plan
                .items
                .iter()
                .map(|item| item.destination.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
        // Carry the planned total so GUI can render progress against it.
        snapshot.total_bytes = request.plan.total_bytes;
        let id = snapshot.id;
        let entry = self.inner.transfers.register(snapshot);
        let registry = self.inner.transfers.clone();
        let event_hub = self.event_hub();
        let inner = self.inner.clone();
        let session_id = request.plan.session_id;
        let overwrite = request.overwrite;
        let concurrency = request.concurrency.max(1);
        let total_bytes = request.plan.total_bytes;
        let item_count = request.plan.items.len();
        // Review fix: cancel_transfer(id) must stop the background plan, not
        // just flip the snapshot (the core batch checks this token between
        // items and returns Error::Interrupted).
        let cancel_token = entry.cancel.clone();

        // Split the plan into file pairs and directory trees (both sides are
        // already resolved absolute paths).
        let trees: Vec<TreeTransferDto> = request
            .plan
            .items
            .iter()
            .filter(|item| item.is_directory)
            .map(|item| TreeTransferDto {
                source: item.source.clone(),
                target: item.destination.clone(),
            })
            .collect();
        let files: Vec<BatchTransferItemDto> = request
            .plan
            .items
            .iter()
            .filter(|item| !item.is_directory)
            .map(|item| BatchTransferItemDto {
                source: item.source.clone(),
                target: item.destination.clone(),
            })
            .collect();

        let handle = tokio::spawn(async move {
            registry.transition(id, TransferState::Running);
            // BatchTransferOptions is not Clone (progress callback Arc); build
            // a fresh copy per call, each carrying the cancellation token.
            let batch = || handshaker_core::BatchTransferOptions {
                overwrite,
                progress: None,
                offset: 0,
                concurrency,
                cancel: Some(cancel_token.clone()),
            };
            let mut result = handshaker_core::BatchTransferResult::default();
            let outcome: Result<(), PublicError> = async {
                for tree in &trees {
                    let partial = match direction {
                        TransferDirectionDto::Download => {
                            client
                                .download_tree(
                                    &tree.source,
                                    std::path::Path::new(&tree.target),
                                    batch(),
                                )
                                .await
                        }
                        TransferDirectionDto::Upload => {
                            client
                                .upload_tree(
                                    std::path::Path::new(&tree.source),
                                    &tree.target,
                                    batch(),
                                )
                                .await
                        }
                    }
                    .map_err(|error| from_core_error(error, "execute_file_plan"))?;
                    merge_core_batch(&mut result, partial);
                }
                let core_items: Vec<handshaker_core::BatchTransferItem> = files
                    .iter()
                    .map(|file| handshaker_core::BatchTransferItem {
                        source: file.source.clone(),
                        target: file.target.clone(),
                    })
                    .collect();
                if !core_items.is_empty() {
                    let partial = match direction {
                        TransferDirectionDto::Download => {
                            client.download_many(&core_items, batch()).await
                        }
                        TransferDirectionDto::Upload => {
                            client.upload_many(&core_items, batch()).await
                        }
                    }
                    .map_err(|error| from_core_error(error, "execute_file_plan"))?;
                    merge_core_batch(&mut result, partial);
                }
                Ok(())
            }
            .await;

            match outcome {
                Ok(()) if result.failures.is_empty() => {
                    if let Some(total) = total_bytes {
                        registry.set_progress(id, total, total);
                    }
                    if let Some(snapshot) = registry.transition(id, TransferState::Completed) {
                        event_hub.publish(BackendEvent::TransferUpdated(snapshot));
                    }
                }
                Ok(()) => {
                    // Review fix: transport death inside a batch is aggregated
                    // as per-file failures by core; surface it as a connection
                    // loss instead of a silent partial success.
                    //
                    // A `Transport` failure here is never a transient USB
                    // hiccup: the USB transport wraps bulk errors in
                    // `io::Error`, but frame.rs converts those back to
                    // `Error::Transport` and the reader/writer task calls
                    // `fail_connection()` (session.rs), which fails every
                    // in-flight request — so a Transport-coded failure means
                    // the core session already declared the connection dead.
                    // `mark_connection_lost` only mirrors that verdict.
                    let connection_closed = result
                        .failures
                        .iter()
                        .any(|failure| failure.code == Some(handshaker_core::ErrorCode::Transport));
                    // Some items failed but the batch itself completed
                    // (partial success): observable, never silent.
                    registry.set_error(
                        id,
                        PublicError::new(
                            if connection_closed {
                                PublicErrorCode::ConnectionLost
                            } else {
                                PublicErrorCode::RemoteIo
                            },
                            format!(
                                "{} of {} plan items failed",
                                result.failures.len(),
                                item_count
                            ),
                        )
                        .operation("execute_file_plan"),
                    );
                    if let Some(snapshot) = registry.transition(id, TransferState::Failed) {
                        event_hub.publish(BackendEvent::TransferUpdated(snapshot));
                    }
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
                Err(error) => {
                    if error.code == PublicErrorCode::TransferCancelled {
                        // cancel_transfer() already flipped the snapshot to
                        // Cancelled (terminal); the task must not overwrite it
                        // with Failed.
                        if let Ok(snapshot) = registry.get(id) {
                            event_hub.publish(BackendEvent::TransferUpdated(snapshot));
                        }
                        return;
                    }
                    let connection_closed = connection_lost_code(error.code);
                    registry.set_error(id, error);
                    if let Some(snapshot) = registry.transition(id, TransferState::Failed) {
                        event_hub.publish(BackendEvent::TransferUpdated(snapshot));
                    }
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
        let session = self.session_handle(request.session_id).await?;
        session.record_activity();
        let client = session.client.clone();
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
        let session = self.session_handle(request.session_id).await?;
        session.record_activity();
        let client = session.client.clone();
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
        let session = self.session_handle(request.session_id).await?;
        session.record_activity();
        let client = session.client.clone();
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
        let session = self.session_handle(request.session_id).await?;
        session.record_activity();
        let client = session.client.clone();
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

    /// Short critical section: clone the session's client Arc, then drop the
    /// registry lock before any network await (M8.1 Phase B / B1).
    async fn session_client_arc(&self, session_id: SessionId) -> AppResult<Arc<HandShakerClient>> {
        Ok(self.session_handle(session_id).await?.client.clone())
    }

    /// Short critical section: clone the session Arc (Phase D / D5 helper),
    /// then drop the registry lock before any network await. Fails fast with
    /// `SessionClosed` once the session left `Ready`.
    async fn session_handle(&self, session_id: SessionId) -> AppResult<Arc<ActiveSession>> {
        let session = {
            let guard = self.inner.sessions.lock().await;
            guard.get(&session_id).cloned()
        }
        .ok_or_else(|| PublicError::new(PublicErrorCode::SessionNotFound, "session not found"))?;
        let state = session_state_from_u8(session.state.load(Ordering::SeqCst));
        if !matches!(state, SessionState::Ready) {
            return Err(PublicError::new(
                PublicErrorCode::SessionClosed,
                "session is not ready",
            ));
        }
        Ok(session)
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
        let outcome = call(client).await;
        // Phase D / D5 §6.2: completion time is recorded for success and
        // failure alike — a failed request is still session activity.
        if let Some(session) = {
            let guard = self.inner.sessions.lock().await;
            guard.get(&session_id).cloned()
        } {
            session.record_activity();
        }
        match outcome {
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

    // ---- Phase D / D6: photo-sync service ----

    /// Sync ledger store rooted at the runtime's configured `state_dir`
    /// (Phase D / D6). `SyncStore::discover` joins "sync" itself and
    /// sanitizes the device_uuid, so the ledger lands at
    /// `<state_dir>/sync/<device_uuid>.json` — the same layout the CLI used
    /// with the core default config dir, so existing ledgers keep working.
    /// `pub(crate)` only so tests can assert the resolved path.
    /// Directory that roots sync ledgers (and every other state file):
    /// the configured `state_dir`, or the core default config directory.
    fn sync_config_dir(&self) -> AppResult<std::path::PathBuf> {
        match &self.inner.config.state_dir {
            Some(dir) => Ok(dir.clone()),
            None => handshaker_core::default_config_dir()
                .map_err(|error| from_core_error(error, "sync.store")),
        }
    }

    pub(crate) fn sync_store_for(
        &self,
        profile: &SyncProfileDto,
    ) -> AppResult<handshaker_core::SyncStore> {
        Ok(handshaker_core::SyncStore::discover(
            &self.sync_config_dir()?,
            &profile.device_uuid,
        ))
    }

    /// Ledger summary for `sync status` (Phase D / D6): files/bytes the
    /// local ledger tracks for one device. No session required — the ledger
    /// is local state rooted at the configured `state_dir`.
    pub async fn sync_ledger_status(&self, device_uuid: &str) -> AppResult<SyncLedgerStatusDto> {
        self.ensure_open()?;
        if device_uuid.trim().is_empty() {
            return Err(PublicError::new(
                PublicErrorCode::InvalidArgument,
                "device_uuid must not be empty",
            )
            .operation("sync.status"));
        }
        let store = handshaker_core::SyncStore::discover(&self.sync_config_dir()?, device_uuid);
        let snapshot = store
            .load()
            .map_err(|error| from_core_error(error, "sync.status"))?
            .unwrap_or_default();
        let files = snapshot.files.len() as u64;
        let bytes: u64 = snapshot.files.values().map(|record| record.size).sum();
        Ok(SyncLedgerStatusDto {
            device_uuid: device_uuid.to_string(),
            files,
            bytes,
        })
    }

    /// Short critical section: clone the job Arc, then drop the registry
    /// lock before any await on the job's task handles.
    async fn sync_job_for(&self, profile_id: &str) -> Option<Arc<SyncJob>> {
        let guard = self.inner.sync_jobs.lock().await;
        guard.get(profile_id).cloned()
    }

    fn sync_job_not_found(operation: &'static str) -> PublicError {
        PublicError::new(PublicErrorCode::NotFound, "sync job not found").operation(operation)
    }

    /// Shared core pipeline for plan and run: resolve the session, load the
    /// ledger, ask the phone for its current state (PHOTO_SYNC_REQUEST), then
    /// diff against the ledger and check local conflicts. Phone files outside
    /// the configured root are dropped (the phone answers with its whole
    /// library; `Path::strip_prefix` is segment-wise, so a sibling like
    /// DCIM/Camera2 never matches phone_root DCIM/Camera).
    async fn sync_pipeline(&self, profile: &SyncProfileDto) -> AppResult<SyncPipeline> {
        let session = self.session_handle(profile.session_id).await?;
        let store = self.sync_store_for(profile)?;
        let snapshot = store
            .load()
            .map_err(|error| from_core_error(error, "sync.plan"))?
            .unwrap_or_default();
        let state = self
            .state_store()?
            .load_or_create()
            .map_err(|error| from_core_error(error, "sync.plan"))?;
        let host_uuid = state.host_uuid.to_string();
        let config = handshaker_core::sync_config(
            &profile.device_uuid,
            &profile.remote_root,
            &profile.local_root,
            &host_uuid,
        );
        let pc_id = handshaker_core::pc_id_from_host_uuid(&host_uuid);
        // The phone rejects a PHOTO_SYNC_REQUEST while it is still in SYNCING
        // state (a plan followed by an immediate run hits this). The state is
        // transient, so retry with a short backoff instead of failing the
        // run (the legacy CLI hit the same wall).
        const PHOTO_SYNC_REJECT_RETRIES: u32 = 3;
        const PHOTO_SYNC_REJECT_BACKOFF_MS: u64 = 1_500;
        let mut phone = None;
        for attempt in 0..=PHOTO_SYNC_REJECT_RETRIES {
            let result = session
                .client
                .photo_sync(&pc_id, &snapshot_to_remote_files(&snapshot))
                .await;
            session.record_activity();
            let result = result.map_err(|error| from_core_error(error, "sync.plan"))?;
            if result.is_success != Some(false) {
                phone = Some(result);
                break;
            }
            if attempt == PHOTO_SYNC_REJECT_RETRIES {
                return Err(PublicError::new(
                    PublicErrorCode::SyncError,
                    "phone rejected photo sync request",
                )
                .operation("sync.plan"));
            }
            tokio::time::sleep(std::time::Duration::from_millis(
                PHOTO_SYNC_REJECT_BACKOFF_MS,
            ))
            .await;
        }
        let phone = phone.expect("loop sets phone on success or returns");
        let phone_files: Vec<RemoteFile> = phone
            .files
            .into_iter()
            .filter(|file| {
                std::path::Path::new(&file.path)
                    .strip_prefix(std::path::Path::new(&profile.remote_root))
                    .is_ok()
            })
            .collect();
        let diff = handshaker_core::plan_diff(&phone_files, &snapshot);
        let conflicts = handshaker_core::check_conflicts(&diff, &snapshot);
        Ok(SyncPipeline {
            client: session.client.clone(),
            config,
            snapshot,
            phone_files,
            diff,
            conflicts,
        })
    }

    /// Preview one sync run: diff the phone state against the ledger and map
    /// the plan onto the public DTO. No files are touched.
    pub async fn plan_sync(&self, profile: SyncProfileDto) -> AppResult<SyncPlanDto> {
        self.ensure_open()?;
        if !profile.enabled {
            return Err(PublicError::new(
                PublicErrorCode::InvalidState,
                "sync profile is disabled",
            )
            .operation("sync.plan"));
        }
        let pipeline = self.sync_pipeline(&profile).await?;
        sync_plan_to_dto(
            &profile.id,
            &pipeline.config,
            &pipeline.diff,
            &pipeline.conflicts,
            &pipeline.phone_files,
            &pipeline.snapshot,
        )
    }

    /// Register a fresh job for a profile under the sync_jobs lock. A job
    /// whose run is still live is refused with `InvalidState` (the caller
    /// must `stop_sync` first); a finished job is replaced with a clean
    /// cancellation token and status. Marking `running` inside the critical
    /// section closes the race between two concurrent `start_sync` calls for
    /// the same profile. No await while the registry lock is held.
    pub(crate) async fn register_sync_job(
        &self,
        profile: SyncProfileDto,
    ) -> AppResult<Arc<SyncJob>> {
        let mut guard = self.inner.sync_jobs.lock().await;
        if let Some(existing) = guard.get(&profile.id) {
            let status = existing.status();
            if status.running {
                return Err(PublicError::new(
                    PublicErrorCode::InvalidState,
                    "sync job already running for this profile",
                )
                .operation("sync.start"));
            }
            if status.monitoring {
                return Err(PublicError::new(
                    PublicErrorCode::InvalidState,
                    "sync watch is active for this profile; stop it before running",
                )
                .operation("sync.start"));
            }
        }
        let job = Arc::new(SyncJob::new(profile));
        job.set_status(|status| status.running = true);
        guard.insert(job.profile.id.clone(), job.clone());
        Ok(job)
    }

    /// Start a full sync run for a profile and return its job id (the
    /// profile id). The plan is computed inside the job with exactly one
    /// `photo_sync` request (the phone rejects a second one while in SYNCING
    /// state — the legacy CLI behaved the same way); conflicted entries are
    /// skipped and reported in `SyncRunResultDto.conflicts`. The ledger is
    /// committed atomically after a completed run; per-file failures are
    /// aggregated by `execute_plan` and the ledger still commits (failed
    /// entries keep their old records, `SyncRunResult.failures` reports
    /// them), while a transport-level failure leaves the old ledger
    /// untouched and reports through [`Self::get_sync_status`].
    pub async fn start_sync(&self, profile: SyncProfileDto) -> AppResult<String> {
        self.ensure_open()?;
        // No pre-flight `plan_sync` here: the phone rejects a second
        // PHOTO_SYNC_REQUEST while it is still in SYNCING state, so a
        // start_sync that planned first would fail on every real device
        // (the legacy CLI ran exactly one photo_sync per run). The plan is
        // computed once inside the job; conflicted entries are skipped and
        // reported in the result, matching the legacy behavior.
        let job = self.register_sync_job(profile).await?;
        let this = self.clone();
        let task_job = job.clone();
        let task = tokio::spawn(async move {
            this.run_sync_job(task_job).await;
        });
        *job.task.lock().await = Some(task);
        Ok(job.profile.id.clone())
    }

    /// Execute one full sync run to completion (spawned by `start_sync`).
    /// The ledger is committed atomically only after a fully successful run;
    /// a failed run leaves the old ledger untouched (re-run is idempotent).
    ///
    /// Panics are caught by the caller's `JoinHandle` (start_sync spawns
    /// this task; stop_sync observes the `JoinError` and marks the job
    /// failed). There must be no nested `tokio::spawn` in here: aborting
    /// the outer task would orphan the inner one, which then keeps the
    /// session/client alive past disconnect (device finding: no QUIT, no
    /// cleanup until process exit).
    async fn run_sync_job(&self, job: Arc<SyncJob>) {
        let outcome = self.run_sync_once(&job).await;
        match outcome {
            Ok(Some((result, updated))) => {
                let commit = self.sync_store_for(&job.profile).and_then(|store| {
                    store
                        .save(&updated)
                        .map_err(|error| from_core_error(error, "sync.run.commit"))
                });
                match commit {
                    Ok(()) => {
                        job.set_last_result(sync_run_result_to_dto(result));
                        job.set_status(|status| {
                            status.running = false;
                            status.last_run_at_ms = Some(now_ms());
                            status.last_error = None;
                        });
                    }
                    Err(error) => {
                        job.set_status(|status| {
                            status.running = false;
                            status.last_error = Some(error);
                        });
                    }
                }
            }
            Ok(None) => {
                // Cancelled before any work started: not an error, and it
                // must not keep a stale last_error from a prior failed run.
                job.set_status(|status| {
                    status.running = false;
                    status.last_error = None;
                });
            }
            Err(error) => {
                job.set_status(|status| {
                    status.running = false;
                    status.last_error = Some(error);
                });
            }
        }
        // The run task is done: clear the slot so the profile can start
        // again (the job itself stays registered for status queries).
        *job.task.lock().await = None;
    }

    /// The full core pipeline for one run: session + ledger + photo_sync +
    /// diff + conflict check, then `execute_plan` (which skips conflicted
    /// entries). `Ok(None)` means the run was cancelled before any network
    /// work. `execute_plan` cannot be interrupted mid-download; `stop_sync`
    /// aborts the task after a bounded wait instead.
    async fn run_sync_once(
        &self,
        job: &SyncJob,
    ) -> AppResult<
        Option<(
            handshaker_core::SyncRunResult,
            handshaker_core::SyncSnapshot,
        )>,
    > {
        if job.cancel.is_cancelled() {
            return Ok(None);
        }
        let pipeline = self.sync_pipeline(&job.profile).await?;
        if job.cancel.is_cancelled() {
            return Ok(None);
        }
        let (result, updated) = handshaker_core::execute_plan(
            &pipeline.client,
            &pipeline.config,
            &pipeline.phone_files,
            &pipeline.diff,
            &pipeline.snapshot,
            &pipeline.conflicts,
        )
        .await
        .map_err(|error| from_core_error(error, "sync.run"))?;
        Ok(Some((result, updated)))
    }

    /// Live status of a registered sync job.
    pub async fn get_sync_status(&self, profile_id: &str) -> AppResult<SyncStatusDto> {
        self.ensure_open()?;
        let job = self
            .sync_job_for(profile_id)
            .await
            .ok_or_else(|| Self::sync_job_not_found("sync.status"))?;
        Ok(job.status())
    }

    /// Result of the most recent completed run (or watch batch); `None` for
    /// a job that never ran. Field names match the CLI `sync run` JSON
    /// contract.
    pub async fn last_sync_result(&self, profile_id: &str) -> AppResult<Option<SyncRunResultDto>> {
        self.ensure_open()?;
        let job = self
            .sync_job_for(profile_id)
            .await
            .ok_or_else(|| Self::sync_job_not_found("sync.result"))?;
        Ok(job.last_result())
    }

    /// Cancel a running sync run and drop the job. Also stops an active
    /// watch on the same profile so nothing keeps running after the job is
    /// gone. Per profile the second call reports `NotFound`.
    pub async fn stop_sync(&self, profile_id: &str) -> AppResult<()> {
        self.ensure_open()?;
        let job = self
            .sync_job_for(profile_id)
            .await
            .ok_or_else(|| Self::sync_job_not_found("sync.stop"))?;
        job.cancel.cancel();
        // Bounded join: core `execute_plan` has no cooperative cancellation,
        // so a run that ignores the token is aborted after the deadline
        // (same pattern as `close_session` — a deadline is not proof of
        // completion; abort + await is).
        let task = job.task.lock().await.take();
        if let Some(mut task) = task {
            match tokio::time::timeout(SYNC_STOP_DEADLINE, &mut task).await {
                Ok(Ok(())) => {}
                Ok(Err(join_error)) => {
                    // The run task panicked: mark the job failed so the
                    // panic is observable (running can never stay true).
                    let message = join_error.to_string();
                    job.set_status(|status| {
                        status.running = false;
                        status.last_error = Some(
                            PublicError::new(
                                PublicErrorCode::Internal,
                                format!("sync pipeline panicked: {message}"),
                            )
                            .operation("sync.run"),
                        );
                    });
                }
                Err(_) => {
                    // A deadline is not proof of completion; abort + await so
                    // no sync task can outlive stop_sync (same pattern as
                    // close_session).
                    task.abort();
                    let _ = task.await;
                }
            }
        }
        // Defensive: a watch can only be active here if it was started after
        // this run job was registered and never stopped; never leave a task
        // running against a job that is about to be removed from the map.
        let watch_task = job.watch_task.lock().await.take();
        if let Some(task) = watch_task {
            task.abort();
            let _ = task.await;
        }
        // Only remove when the map still holds exactly this job: a
        // concurrent re-registration during the bounded join must survive.
        self.remove_sync_job_if_current(profile_id, &job).await;
        Ok(())
    }

    /// Remove a job from the registry only when the map still holds exactly
    /// this job (review fix): a `stop_sync` whose bounded join overlapped a
    /// concurrent `start_sync` re-registration must not delete the fresh job.
    pub(crate) async fn remove_sync_job_if_current(
        &self,
        profile_id: &str,
        job: &Arc<SyncJob>,
    ) -> bool {
        let mut guard = self.inner.sync_jobs.lock().await;
        match guard.get(profile_id) {
            Some(current) if Arc::ptr_eq(current, job) => {
                guard.remove(profile_id);
                true
            }
            _ => false,
        }
    }

    /// Reserve the watch slot for a job inside the registry critical
    /// section: the run/watch mutual exclusion and the `monitoring` flag
    /// flip must be atomic with respect to `register_sync_job` (review fix:
    /// the previous check-then-await-then-flag had a TOCTOU window).
    pub(crate) async fn reserve_sync_watch(&self, job: &Arc<SyncJob>) -> AppResult<()> {
        let guard = self.inner.sync_jobs.lock().await;
        match guard.get(&job.profile.id) {
            Some(current) if !Arc::ptr_eq(current, job) => {
                return Err(PublicError::new(
                    PublicErrorCode::InvalidState,
                    "sync job was replaced; retry",
                )
                .operation("sync.watch.start"));
            }
            None => return Err(Self::sync_job_not_found("sync.watch.start")),
            _ => {}
        }
        let status = job.status();
        if status.running {
            return Err(PublicError::new(
                PublicErrorCode::InvalidState,
                "sync job is running; stop it before watching",
            )
            .operation("sync.watch.start"));
        }
        if status.monitoring {
            return Err(PublicError::new(
                PublicErrorCode::InvalidState,
                "sync watch already active for this profile",
            )
            .operation("sync.watch.start"));
        }
        job.set_status(|status| status.monitoring = true);
        Ok(())
    }

    /// Roll the watch reservation back after a failed activation.
    pub(crate) async fn release_sync_watch(&self, job: &Arc<SyncJob>) {
        let guard = self.inner.sync_jobs.lock().await;
        if guard
            .get(&job.profile.id)
            .is_some_and(|current| Arc::ptr_eq(current, job))
        {
            job.set_status(|status| status.monitoring = false);
        }
    }

    /// Start watching a profile for phone file changes and apply them
    /// incrementally. Requires a registered job (created by `start_sync`) —
    /// the job carries the profile configuration and the live status.
    /// Rejected with `InvalidState` while a run is in progress or a watch is
    /// already active for this profile.
    pub async fn start_sync_watch(&self, profile_id: &str) -> AppResult<()> {
        self.ensure_open()?;
        let job = self
            .sync_job_for(profile_id)
            .await
            .ok_or_else(|| Self::sync_job_not_found("sync.watch.start"))?;
        if !job.profile.enabled {
            return Err(PublicError::new(
                PublicErrorCode::InvalidState,
                "sync profile is disabled",
            )
            .operation("sync.watch.start"));
        }
        // Check + flag flip happen inside the registry lock (TOCTOU fix).
        self.reserve_sync_watch(&job).await?;
        let session = self.session_handle(job.profile.session_id).await?;
        let client = session.client.clone();
        let accepted = match client.sync_monitor(true).await {
            Ok(accepted) => accepted,
            Err(error) => {
                self.release_sync_watch(&job).await;
                return Err(from_core_error(error, "sync.watch.start"));
            }
        };
        if !accepted {
            self.release_sync_watch(&job).await;
            return Err(PublicError::new(
                PublicErrorCode::SyncError,
                "phone rejected sync monitor request",
            )
            .operation("sync.watch.start"));
        }
        session.record_activity();
        let subscription = client.subscribe_events(EventFilter::only([EventKind::FileChanged]));
        let this = self.clone();
        let watch_job = job.clone();
        let watch_task = tokio::spawn(async move {
            this.run_sync_watch(watch_job, subscription).await;
        });
        *job.watch_task.lock().await = Some(watch_task);
        Ok(())
    }

    /// Stop an active watch: cancel the watch task, disable the phone-side
    /// monitor (when the session is still usable), and clear the monitoring
    /// flag. The job stays registered so the profile can be watched again
    /// or run again. The event-stream `Closed` observed by the watch task is
    /// not an error, but a `sync_monitor(false)` failure on a live session
    /// is returned as an error (it leaves the phone-side monitor on).
    pub async fn stop_sync_watch(&self, profile_id: &str) -> AppResult<()> {
        self.ensure_open()?;
        let job = self
            .sync_job_for(profile_id)
            .await
            .ok_or_else(|| Self::sync_job_not_found("sync.watch.stop"))?;
        let watch_task = job.watch_task.lock().await.take();
        if let Some(task) = watch_task {
            // The watch task blocks on the event stream with no cooperative
            // cancellation point; abort it and await the cancellation.
            task.abort();
            let _ = task.await;
        }
        job.set_status(|status| status.monitoring = false);
        // Turn the phone-side monitor off when the session is still usable;
        // a session that already died is not an error here (the watch task
        // already observed the closed event stream).
        if let Ok(session) = self.session_handle(job.profile.session_id).await {
            session
                .client
                .sync_monitor(false)
                .await
                .map_err(|error| from_core_error(error, "sync.watch.stop"))?;
            session.record_activity();
        }
        Ok(())
    }

    /// Surface a lagged file-change event: the missed changes are not
    /// applied, so the incremental ledger would silently diverge — record it
    /// in the job status and publish a warning telling the user to run a
    /// full sync to reconcile (review fix: never swallow the gap).
    async fn report_sync_watch_lag(&self, job: &Arc<SyncJob>, missed: u64) {
        let error = PublicError::new(
            PublicErrorCode::SyncError,
            format!("sync watch lagged; missed {missed} file-change event(s) — run a full sync to reconcile"),
        )
        .operation("sync.watch");
        job.set_status(|status| status.last_error = Some(error.clone()));
        self.inner.event_hub.publish(BackendEvent::Warning(error));
    }

    /// Watch loop: wait for a file-change event, keep collecting during the
    /// debounce window, apply the batch incrementally, repeat. Exits when
    /// the session's event stream closes (monitoring=false,
    /// last_error=ConnectionLost) or the task is aborted.
    async fn run_sync_watch(&self, job: Arc<SyncJob>, mut subscription: EventSubscription) {
        'watch: loop {
            // Wait for the first event of a batch.
            let mut batch: Vec<FileChange> = Vec::new();
            loop {
                match subscription.recv().await {
                    Ok(ClientEvent::FileChanged(changes)) => {
                        batch.extend(changes);
                        break;
                    }
                    Ok(_) => continue,
                    Err(EventStreamError::Lagged { missed }) => {
                        // Review fix: lagged events are never applied; the
                        // incremental ledger would silently diverge. Surface
                        // it and ask for a full sync instead of hiding it.
                        self.report_sync_watch_lag(&job, missed).await;
                    }
                    Err(EventStreamError::Closed) => break 'watch,
                }
            }
            // Debounce: keep collecting for one window after the last event.
            loop {
                match tokio::time::timeout(SYNC_WATCH_DEBOUNCE, subscription.recv()).await {
                    Ok(Ok(ClientEvent::FileChanged(changes))) => batch.extend(changes),
                    Ok(Err(EventStreamError::Lagged { missed })) => {
                        self.report_sync_watch_lag(&job, missed).await;
                    }
                    Ok(Err(EventStreamError::Closed)) => break 'watch,
                    Ok(Ok(_)) => {}
                    Err(_elapsed) => break,
                }
            }
            match self.apply_watch_batch(&job.profile, &batch).await {
                Ok(result) => {
                    job.set_last_result(result.clone());
                    job.set_status(|status| {
                        status.last_run_at_ms = Some(now_ms());
                        status.last_error = None;
                    });
                    // Let GUI/CLI render progress without polling (Phase D /
                    // D6): the batch result travels as a backend event.
                    self.inner
                        .event_hub
                        .publish(BackendEvent::SyncWatchApplied(result));
                }
                Err(error) => {
                    job.set_status(|status| status.last_error = Some(error));
                }
            }
        }
        // The session's event stream closed: connection lost, watch over.
        job.set_status(|status| {
            status.monitoring = false;
            status.last_error = Some(
                PublicError::new(
                    PublicErrorCode::ConnectionLost,
                    "sync watch stopped: session event stream closed",
                )
                .operation("sync.watch"),
            );
        });
        *job.watch_task.lock().await = None;
    }

    /// Apply one debounced batch incrementally with the core
    /// `apply_file_change` instead of a full re-plan. Rationale: the phone
    /// rejects a second PHOTO_SYNC_REQUEST(37) while it is in the SYNCING
    /// state (observed by the CLI), and the change events already carry full
    /// file metadata — a re-plan would only re-fetch what the events contain.
    /// Because `apply_file_change` has no conflict protection, every
    /// download/delete is pre-checked with the core `check_conflicts` on a
    /// one-entry diff, so a user-modified local file is preserved and
    /// reported instead of being overwritten. The ledger is committed
    /// atomically after the batch.
    async fn apply_watch_batch(
        &self,
        profile: &SyncProfileDto,
        changes: &[FileChange],
    ) -> AppResult<SyncRunResultDto> {
        let session = self.session_handle(profile.session_id).await?;
        session.record_activity();
        let store = self.sync_store_for(profile)?;
        let mut snapshot = store
            .load()
            .map_err(|error| from_core_error(error, "sync.watch"))?
            .unwrap_or_default();
        let state = self
            .state_store()?
            .load_or_create()
            .map_err(|error| from_core_error(error, "sync.watch"))?;
        let host_uuid = state.host_uuid.to_string();
        let config = handshaker_core::sync_config(
            &profile.device_uuid,
            &profile.remote_root,
            &profile.local_root,
            &host_uuid,
        );
        let mut run = handshaker_core::SyncRunResult::default();
        let mut touched = false;
        for change in changes {
            let Some(file) = change.file.as_ref() else {
                continue;
            };
            if file.is_directory {
                continue;
            }
            // Conflict pre-check: skip downloads/deletes that would clobber
            // a user-modified local file (core check_conflicts, one-entry
            // diff — the algorithm is never copied here).
            let guard = one_entry_diff(change);
            if (!guard.added.is_empty() || !guard.deleted.is_empty())
                && !handshaker_core::check_conflicts(&guard, &snapshot).is_empty()
            {
                run.conflicts.push(file.path.clone());
                continue;
            }
            let result =
                handshaker_core::apply_file_change(&session.client, &config, change, &mut snapshot)
                    .await
                    .map_err(|error| from_core_error(error, "sync.watch"))?;
            touched = true;
            run.downloaded.extend(result.downloaded);
            run.deleted.extend(result.deleted);
            run.failures.extend(result.failures);
        }
        if touched {
            store
                .save(&snapshot)
                .map_err(|error| from_core_error(error, "sync.watch"))?;
        }
        Ok(sync_run_result_to_dto(run))
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

/// Stable snake_case token for a core `FileChangeStatus` (mirrors the core
/// serde token, so watch/sync consumers can match on it without touching
/// core types).
fn file_change_status_token(status: FileChangeStatus) -> String {
    match status {
        FileChangeStatus::None => "none",
        FileChangeStatus::Added => "added",
        FileChangeStatus::Deleted => "deleted",
        FileChangeStatus::Modified => "modified",
        FileChangeStatus::InfoModified => "info_modified",
        FileChangeStatus::FileAndInfoModified => "file_and_info_modified",
        FileChangeStatus::Unknown => "unknown",
    }
    .to_string()
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
                *guard = dto.clone();
            }
            // Reconcile the descriptor so `DeviceUpdated` carries the stable
            // identity and any name/model the push reported (Phase D / D2).
            let updated = reconcile_device_identity(device, &dto);
            BackendEvent::DeviceUpdated {
                session_id,
                device: updated,
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
                    .iter()
                    .filter_map(|event| event.file.as_ref().map(|file| file.path.clone()))
                    .collect(),
                files: events
                    .iter()
                    .filter_map(|event| event.file.clone().map(remote_file_to_dto))
                    .collect(),
                // Directory-monitor events carry no per-path status.
                statuses: Vec::new(),
            },
        },
        ClientEvent::FileChanged(changes) => BackendEvent::RemoteFileChanged {
            session_id,
            change: RemoteFileChangeDto {
                change_kind: RemoteFileChangeKind::FileChanged,
                paths: changes
                    .iter()
                    .filter_map(|change| change.file.as_ref().map(|file| file.path.clone()))
                    .collect(),
                files: changes
                    .iter()
                    .filter_map(|change| change.file.clone().map(remote_file_to_dto))
                    .collect(),
                // Parallel to `paths`: one status token per changed file.
                statuses: changes
                    .iter()
                    .filter_map(|change| {
                        change
                            .file
                            .as_ref()
                            .map(|_| file_change_status_token(change.status))
                    })
                    .collect(),
            },
        },
        ClientEvent::PhotoSyncChanged(change) => {
            let files: Vec<FileEntryDto> = change
                .files
                .iter()
                .cloned()
                .map(remote_file_to_dto)
                .collect();
            let paths: Vec<String> = change.files.into_iter().map(|file| file.path).collect();
            BackendEvent::RemoteFileChanged {
                session_id,
                change: RemoteFileChangeDto {
                    change_kind: RemoteFileChangeKind::PhotoSyncChanged,
                    paths,
                    files,
                    statuses: Vec::new(),
                },
            }
        }
        ClientEvent::SyncMonitorChanged(_) => BackendEvent::RemoteFileChanged {
            session_id,
            change: RemoteFileChangeDto {
                change_kind: RemoteFileChangeKind::SyncMonitorChanged,
                paths: Vec::new(),
                files: Vec::new(),
                statuses: Vec::new(),
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
        external_storage_path: info.external_storage_path.clone(),
        disk_size: info.disk_size,
        used_disk_size: info.used_disk_size,
        battery_percentage: info.battery_percentage,
        phone_locked: info.phone_locked,
    }
}

/// Reconcile a discovery entry with the connected phone's device info
/// (Phase D / D2): the phone's `phone_id` becomes the stable identity
/// (`phone:<uuid>`), and the phone-reported name/model win over whatever
/// the discovery entry carried. Without a `phone_id` the descriptor
/// keeps its discovery identity and `stable_id` stays `None` — ADB/USB
/// connections remain fully usable that way.
pub(crate) fn reconcile_device_identity(
    discovered: &DeviceDescriptor,
    info: &DeviceInfoDto,
) -> DeviceDescriptor {
    let mut device = discovered.clone();
    device.stable_id = info
        .phone_id
        .as_ref()
        .filter(|id| !id.trim().is_empty())
        .map(|id| DeviceId(format!("phone:{id}")));
    if let Some(name) = info.name.clone() {
        device.display_name = Some(name);
    }
    if let Some(model) = info.model.clone() {
        device.model = Some(model);
    }
    device
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
        cancel: None,
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

// ---- Phase D / D4: plan helpers ----

/// Resolve the local download destination for one remote entry. With a
/// single source the destination is used as-is unless it is an existing
/// directory (then the remote basename is appended); with multiple sources
/// the destination must be an existing directory. Returns `None` when the
/// destination cannot be resolved (a conflict was appended).
pub(crate) fn resolve_local_download_destination(
    destination: &str,
    remote: &FileEntryDto,
    source_count: usize,
    conflicts: &mut Vec<FilePlanConflict>,
) -> Option<std::path::PathBuf> {
    // Defense in depth (security review): `resolve_remote_path` already
    // normalizes the remote path, but never let a "." / ".." / empty
    // basename escape the intended destination directory.
    let base = match std::path::Path::new(&remote.path)
        .file_name()
        .and_then(|name| name.to_str())
    {
        Some(name) if !matches!(name, "." | "..") && !name.is_empty() => name.to_string(),
        _ => {
            conflicts.push(FilePlanConflict {
                kind: FileConflictKind::DestinationTypeMismatch,
                source: remote.path.clone(),
                destination: destination.to_string(),
                message: "remote path has no usable file name".into(),
                overridable: false,
            });
            return None;
        }
    };
    let destination_path = std::path::PathBuf::from(destination);
    let existing = std::fs::metadata(&destination_path).ok();

    if source_count > 1 {
        match existing {
            Some(metadata) if metadata.is_dir() => {
                return Some(destination_path.join(base));
            }
            _ => {
                conflicts.push(FilePlanConflict {
                    kind: FileConflictKind::DestinationTypeMismatch,
                    source: remote.path.clone(),
                    destination: destination.to_string(),
                    message: "multi-source download requires an existing destination directory"
                        .into(),
                    overridable: false,
                });
                return None;
            }
        }
    }
    match existing {
        Some(metadata) if metadata.is_dir() => Some(destination_path.join(base)),
        _ => Some(destination_path),
    }
}

/// Inspect an already-resolved local download destination: an existing file
/// is a `DestinationExists` conflict (overridable by `overwrite`); a
/// file/directory shape mismatch is never overridable.
pub(crate) fn inspect_local_destination(
    destination: &std::path::Path,
    remote_is_directory: bool,
    overwrite: bool,
    conflicts: &mut Vec<FilePlanConflict>,
) {
    let Some(metadata) = std::fs::metadata(destination).ok() else {
        return;
    };
    if metadata.is_dir() != remote_is_directory {
        conflicts.push(FilePlanConflict {
            kind: FileConflictKind::DestinationTypeMismatch,
            source: destination.display().to_string(),
            destination: destination.display().to_string(),
            message: "destination type does not match the source".into(),
            overridable: false,
        });
    } else if metadata.is_file() {
        conflicts.push(FilePlanConflict {
            kind: FileConflictKind::DestinationExists,
            source: destination.display().to_string(),
            destination: destination.display().to_string(),
            message: "destination already exists".into(),
            overridable: overwrite,
        });
    }
}

/// Resolve the remote upload destination for one local source. With a
/// single source the destination is used as-is unless it is an existing
/// remote directory (then the local basename is appended); with multiple
/// sources the destination must be an existing remote directory.
pub(crate) fn resolve_remote_upload_destination(
    destination: &str,
    source: &std::path::Path,
    source_count: usize,
    destination_stat: Option<&FileEntryDto>,
    conflicts: &mut Vec<FilePlanConflict>,
) -> Option<String> {
    // Defense in depth (security review): reject "." / ".." / empty local
    // basenames instead of letting them escape the intended remote
    // directory.
    let base = match source.file_name().and_then(|name| name.to_str()) {
        Some(name) if !matches!(name, "." | "..") && !name.is_empty() => name.to_string(),
        _ => {
            conflicts.push(FilePlanConflict {
                kind: FileConflictKind::DestinationTypeMismatch,
                source: source.display().to_string(),
                destination: destination.to_string(),
                message: "local source has no usable file name".into(),
                overridable: false,
            });
            return None;
        }
    };
    if source_count > 1 {
        match destination_stat {
            Some(existing) if existing.is_directory => {
                return Some(format!("{destination}/{base}"));
            }
            _ => {
                conflicts.push(FilePlanConflict {
                    kind: FileConflictKind::DestinationTypeMismatch,
                    source: source.display().to_string(),
                    destination: destination.to_string(),
                    message: "multi-source upload requires an existing remote directory".into(),
                    overridable: false,
                });
                return None;
            }
        }
    }
    match destination_stat {
        Some(existing) if existing.is_directory => Some(format!("{destination}/{base}")),
        _ => Some(destination.to_string()),
    }
}

/// Compare one planned upload target with the remote entry that exists
/// there: a shape mismatch is never overridable; an existing file of the
/// same shape is overridable by `overwrite`.
pub(crate) fn append_remote_destination_conflict(
    source: &str,
    destination: &str,
    is_directory: bool,
    existing: &FileEntryDto,
    overwrite: bool,
    conflicts: &mut Vec<FilePlanConflict>,
) {
    if existing.is_directory != is_directory {
        conflicts.push(FilePlanConflict {
            kind: FileConflictKind::DestinationTypeMismatch,
            source: source.to_string(),
            destination: destination.to_string(),
            message: "remote destination type does not match the source".into(),
            overridable: false,
        });
    } else if !existing.is_directory {
        conflicts.push(FilePlanConflict {
            kind: FileConflictKind::DestinationExists,
            source: source.to_string(),
            destination: destination.to_string(),
            message: "remote destination already exists".into(),
            overridable: overwrite,
        });
    }
}

/// Two sources mapping onto the same destination are never overridable by
/// overwrite — one of them would silently be skipped.
pub(crate) fn append_duplicate_destination_conflicts(
    items: &[FilePlanItem],
    conflicts: &mut Vec<FilePlanConflict>,
) {
    let mut seen = std::collections::HashMap::<&str, &str>::new();
    for item in items {
        if let Some(previous) = seen.insert(item.destination.as_str(), item.source.as_str()) {
            conflicts.push(FilePlanConflict {
                kind: FileConflictKind::DuplicateDestination,
                source: item.source.clone(),
                destination: item.destination.clone(),
                message: format!(
                    "multiple sources map to the same destination ({previous} and {})",
                    item.source
                ),
                overridable: false,
            });
        }
    }
}

/// Compute the aggregate counters and executability of a plan. `executable`
/// is false while any non-overridable conflict remains; overridable
/// conflicts are resolved by the caller's options at execution time.
pub(crate) fn finalize_file_plan(
    direction: FilePlanDirection,
    session_id: SessionId,
    items: Vec<FilePlanItem>,
    conflicts: Vec<FilePlanConflict>,
    requires_recursive: bool,
) -> FileOperationPlan {
    let file_count = items.iter().filter(|item| !item.is_directory).count() as u64;
    let directory_count = items.iter().filter(|item| item.is_directory).count() as u64;
    let total_bytes = {
        let sum: u64 = items
            .iter()
            .filter_map(|item| item.size)
            .fold(0, u64::saturating_add);
        (file_count > 0).then_some(sum)
    };
    let executable = conflicts.iter().all(|conflict| conflict.overridable);
    FileOperationPlan {
        direction,
        session_id,
        items,
        conflicts,
        file_count,
        directory_count,
        total_bytes,
        requires_recursive,
        executable,
    }
}

/// Map a local filesystem error that is not a missing/permission case onto
/// the stable public error.
fn map_local_plan_error(error: std::io::Error, source: &str) -> PublicError {
    PublicError::new(
        PublicErrorCode::LocalPermissionDenied,
        format!("cannot inspect local source {source}"),
    )
    .with_detail(error.to_string())
    .operation("plan_upload")
}
