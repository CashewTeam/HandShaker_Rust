//! Transfer task model (M8 §5.6): long tasks run in the background under a
//! `TransferId`; callers poll snapshots and cancel via token. State
//! transitions are one-way: Queued -> Running -> Completed|Failed|Cancelled.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use handshaker_core::{CancellationToken, RequestOptions, TransferOptions};

use crate::dto::SessionId;
use crate::error::{AppResult, PublicError, PublicErrorCode};
use crate::event::{BackendEvent, EventHub};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Application-layer transfer identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct TransferId(pub u64);

/// Fixed transfer states (one-way transitions, M8 §5.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TransferState {
    Queued = 1,
    Running = 2,
    Completed = 3,
    Failed = 4,
    Cancelled = 5,
}

/// Transfer direction for snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TransferDirectionDto {
    Download = 1,
    Upload = 2,
}

/// UI-ready transfer snapshot. Bytes are `u64`; time is Unix ms.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TransferSnapshot {
    pub id: TransferId,
    pub session_id: SessionId,
    pub direction: TransferDirectionDto,
    pub source: String,
    pub destination: String,
    pub state: TransferState,
    pub transferred_bytes: u64,
    pub total_bytes: Option<u64>,
    pub started_at_ms: Option<u64>,
    pub finished_at_ms: Option<u64>,
    pub error: Option<crate::error::PublicError>,
    /// Planned item count (files + trees) of a batch transfer; 0 for
    /// single-file transfers. Phase E field; `#[serde(default)]` keeps
    /// legacy JSON decoding.
    #[serde(default)]
    pub item_count: u64,
    /// Items completed so far (batch transfers).
    #[serde(default)]
    pub completed_items: u64,
    /// Items failed so far (batch transfers).
    #[serde(default)]
    pub failed_items: u64,
    /// Source path of the item currently being processed.
    #[serde(default)]
    pub current_item: Option<String>,
    /// Aggregated per-item result, attached before the terminal transition.
    #[serde(default)]
    pub batch_result: Option<BatchTransferResultDto>,
}

/// Start a download. `remote_path` is absolute or resolved by the caller.
#[derive(Debug, Clone)]
pub struct DownloadRequest {
    pub session_id: SessionId,
    pub remote_path: String,
    /// Local destination path (UTF-8 on FFI; PathBuf at the Rust layer).
    pub local_path: std::path::PathBuf,
    pub overwrite: bool,
}

/// Start an upload.
#[derive(Debug, Clone)]
pub struct UploadRequest {
    pub session_id: SessionId,
    pub local_path: std::path::PathBuf,
    pub remote_path: String,
    pub overwrite: bool,
}

/// One source/target pair in a batch transfer. Remote side (source for
/// download, target for upload) is resolved against the device root by the
/// application layer; the local side is a host path string.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BatchTransferItemDto {
    /// Source path (remote for download, local for upload).
    pub source: String,
    /// Target path (local for download, remote for upload).
    pub target: String,
}

/// One directory tree to mirror (source base -> target base). The remote
/// base is resolved against the device root by the application layer;
/// recursive enumeration and path-escape hardening live in the core
/// `download_tree` / `upload_tree`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TreeTransferDto {
    pub source: String,
    pub target: String,
}

/// Batch transfer request: serial execution, per-file failures aggregated
/// (a failure never aborts the remaining files).
#[derive(Debug, Clone)]
pub struct BatchTransferRequest {
    pub session_id: SessionId,
    /// Explicit file pairs.
    pub files: Vec<BatchTransferItemDto>,
    /// Directory trees to mirror recursively.
    pub trees: Vec<TreeTransferDto>,
    pub overwrite: bool,
}

/// One failed item with its error message.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TransferFailureDto {
    pub source: String,
    pub target: String,
    pub message: String,
}

/// Aggregated batch result.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BatchTransferResultDto {
    pub ok: Vec<BatchTransferItemDto>,
    pub failures: Vec<TransferFailureDto>,
}

/// Internal registry entry.
pub(crate) struct ActiveTransfer {
    pub(crate) snapshot: Mutex<TransferSnapshot>,
    pub(crate) cancel: CancellationToken,
    /// Join handle kept until the task finishes; cleaned on get/list.
    pub(crate) join: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Progress event throttling state (M8.1 Phase C / C2): emit at most
    /// ~10/s and at least every 256 KiB, so a 1 GiB transfer emits a
    /// bounded number of events while still tracking size milestones.
    progress_throttle: Mutex<ProgressThrottle>,
}

#[derive(Default)]
struct ProgressThrottle {
    last_emit_ms: u64,
    last_emit_bytes: u64,
}

/// Progress event throttling (M8.1 Phase C / C2): time- and byte-based
/// thresholds keep the event stream bounded (~10–20/s max) while terminal
/// events are always published unconditionally by the task/cancel paths.
const PROGRESS_MIN_INTERVAL_MS: u64 = 100;
const PROGRESS_MIN_BYTES: u64 = 256 * 1024;

/// Registry of transfers with bounded history. State transitions are guarded
/// by the per-entry snapshot mutex; no data races under concurrent access.
pub struct TransferRegistry {
    transfers: Mutex<HashMap<TransferId, Arc<ActiveTransfer>>>,
    next_id: AtomicU64,
    event_hub: EventHub,
    history_capacity: usize,
    history_ttl: Option<Duration>,
}

impl TransferRegistry {
    pub fn new(
        event_hub: EventHub,
        history_capacity: usize,
        history_ttl: Option<Duration>,
    ) -> Self {
        Self {
            transfers: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            event_hub,
            history_capacity: history_capacity.max(1),
            history_ttl,
        }
    }

    pub(crate) fn register(&self, snapshot: TransferSnapshot) -> Arc<ActiveTransfer> {
        let id = snapshot.id;
        let entry = Arc::new(ActiveTransfer {
            snapshot: Mutex::new(snapshot),
            cancel: CancellationToken::new(),
            join: Mutex::new(None),
            progress_throttle: Mutex::new(ProgressThrottle::default()),
        });
        let mut guard = self
            .transfers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.insert(id, entry.clone());
        // Bounded history (M8.1 Phase C / C4): drop the oldest finished
        // entries (by finished_at_ms) while over capacity, then TTL-expired
        // entries.
        self.reap_locked(&mut guard);
        entry
    }

    /// Drop finished entries while over capacity (oldest first) and any
    /// finished entries older than the TTL. Caller must hold the registry
    /// lock.
    fn reap_locked(&self, guard: &mut HashMap<TransferId, Arc<ActiveTransfer>>) {
        let now = now_ms();
        if let Some(ttl) = self.history_ttl {
            let ttl_ms = ttl.as_millis() as u64;
            guard.retain(|_, entry| {
                let finished_at = entry
                    .snapshot
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .finished_at_ms;
                match finished_at {
                    Some(finished) => now.saturating_sub(finished) < ttl_ms,
                    None => true,
                }
            });
        }
        while guard.len() > self.history_capacity {
            let oldest_finished = guard
                .iter()
                .filter_map(|(id, entry)| {
                    let finished_at = entry
                        .snapshot
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .finished_at_ms?;
                    Some((*id, finished_at))
                })
                .min_by_key(|(_, finished_at)| *finished_at);
            match oldest_finished {
                Some((id, _)) => {
                    guard.remove(&id);
                }
                None => {
                    // No finished entry to evict; stop (do not drop live
                    // transfers to make room — that would break running
                    // tasks).
                    break;
                }
            }
        }
    }

    pub fn get(&self, id: TransferId) -> AppResult<TransferSnapshot> {
        let guard = self
            .transfers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = guard.get(&id).ok_or_else(|| {
            PublicError::new(PublicErrorCode::TransferNotFound, "transfer not found")
        })?;
        Ok(entry
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone())
    }

    pub fn list(&self) -> Vec<TransferSnapshot> {
        let guard = self
            .transfers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard
            .values()
            .filter_map(|entry| entry.snapshot.lock().ok().map(|snapshot| snapshot.clone()))
            .collect()
    }

    /// Transition to a new state and return the updated snapshot.
    /// State transitions are one-way; a terminal state is never overwritten.
    pub(crate) fn transition(
        &self,
        id: TransferId,
        next: TransferState,
    ) -> Option<TransferSnapshot> {
        let guard = self
            .transfers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = guard.get(&id)?;
        let mut snapshot = entry
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let current = snapshot.state;
        let terminal = matches!(
            current,
            TransferState::Completed | TransferState::Failed | TransferState::Cancelled
        );
        if terminal {
            return Some(snapshot.clone());
        }
        snapshot.state = next;
        if next == TransferState::Completed
            || next == TransferState::Failed
            || next == TransferState::Cancelled
        {
            snapshot.finished_at_ms = Some(now_ms());
        }
        Some(snapshot.clone())
    }

    /// Cancel a transfer: idempotent. The terminal state (with
    /// `finished_at_ms`) and a `TransferUpdated` event are published
    /// immediately; the background task's later result can never overwrite a
    /// terminal state (M8.1 Phase C / C3).
    pub fn cancel(&self, id: TransferId) -> AppResult<()> {
        let token = {
            let guard = self
                .transfers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let entry = guard.get(&id).ok_or_else(|| {
                PublicError::new(PublicErrorCode::TransferNotFound, "transfer not found")
            })?;
            entry.cancel.clone()
        };
        token.cancel();
        // Transition after dropping the registry lock (std Mutex is not
        // re-entrant; transition would otherwise deadlock).
        if let Some(snapshot) = self.transition(id, TransferState::Cancelled) {
            self.event_hub
                .publish(BackendEvent::TransferUpdated(snapshot));
        }
        Ok(())
    }

    /// Cancel every transfer belonging to a session and hand back their join
    /// handles, so the caller can wait (bounded) for the tasks to release
    /// the shared session client before closing it (M8.1 Phase B / B2).
    pub(crate) fn cancel_for_session(
        &self,
        session_id: SessionId,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        // Join-handle ownership is independent of the public transfer state.
        // A task can already be marked Cancelled while it is still unwinding
        // and holding a session client Arc (for example shutdown first calls
        // cancel()). Always collect every entry for this session.
        let entries: Vec<(TransferId, CancellationToken, bool)> = {
            let guard = self
                .transfers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard
                .values()
                .filter_map(|entry| {
                    let snapshot = entry
                        .snapshot
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let terminal = matches!(
                        snapshot.state,
                        TransferState::Completed | TransferState::Failed | TransferState::Cancelled
                    );
                    (snapshot.session_id == session_id)
                        .then(|| (snapshot.id, entry.cancel.clone(), terminal))
                })
                .collect()
        };
        let mut joins = Vec::new();
        for (id, token, terminal) in entries {
            token.cancel();
            // Transition after dropping the registry lock (std Mutex is not
            // re-entrant); publish the terminal event synchronously so the
            // transfer events precede the caller's session-level events
            // (M8.1 Phase C / C5 "terminal events first").
            if !terminal && let Some(snapshot) = self.transition(id, TransferState::Cancelled) {
                self.event_hub
                    .publish(BackendEvent::TransferUpdated(snapshot));
            }
            // Take a still-owned task handle even when its public snapshot is
            // already terminal. The close path must join or abort that task.
            let join = {
                let guard = self
                    .transfers
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                guard
                    .get(&id)
                    .and_then(|entry| entry.join.lock().ok().and_then(|mut join| join.take()))
            };
            if let Some(join) = join {
                joins.push(join);
            }
        }
        joins
    }

    /// Reap finished join handles (best-effort).
    pub(crate) fn reap(&self) {
        let guard = self
            .transfers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for entry in guard.values() {
            if let Ok(mut join) = entry.join.lock()
                && let Some(handle) = join.take()
            {
                if handle.is_finished() {
                    // Dropping the handle detaches the finished task.
                } else {
                    *join = Some(handle);
                }
            }
        }
    }

    pub(crate) fn snapshot_for(
        &self,
        session_id: SessionId,
        direction: TransferDirectionDto,
        source: String,
        destination: String,
    ) -> TransferSnapshot {
        let id = TransferId(self.next_id.fetch_add(1, Ordering::SeqCst));
        TransferSnapshot {
            id,
            session_id,
            direction,
            source,
            destination,
            state: TransferState::Queued,
            transferred_bytes: 0,
            total_bytes: None,
            started_at_ms: Some(now_ms()),
            finished_at_ms: None,
            error: None,
            item_count: 0,
            completed_items: 0,
            failed_items: 0,
            current_item: None,
            batch_result: None,
        }
    }

    /// Shared progress-throttle decision (M8.1 Phase C / C2): a progress
    /// update may emit when enough time passed since the last emit or the
    /// byte counter advanced past the byte threshold. Item-level batch
    /// updates carry no byte delta (`bytes: None`) and use the time
    /// threshold only. Updates the throttle state when the update may
    /// emit; a throttled update does not refresh the timestamps. The
    /// caller must hold the entry's snapshot lock (lock order: snapshot,
    /// then throttle — the same order `set_progress` uses).
    fn throttle_allows(&self, entry: &ActiveTransfer, now: u64, bytes: Option<u64>) -> bool {
        let mut throttle = entry
            .progress_throttle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let enough_time = now.saturating_sub(throttle.last_emit_ms) >= PROGRESS_MIN_INTERVAL_MS;
        let enough_bytes = bytes
            .map(|bytes| bytes.saturating_sub(throttle.last_emit_bytes) >= PROGRESS_MIN_BYTES)
            .unwrap_or(false);
        if enough_time || enough_bytes {
            throttle.last_emit_ms = now;
            if let Some(bytes) = bytes {
                throttle.last_emit_bytes = bytes;
            }
            true
        } else {
            false
        }
    }

    /// Called from the progress callback (sync context). Updates both
    /// `transferred_bytes` and `total_bytes`, then publishes a throttled
    /// `TransferUpdated` event (time/byte thresholds; M8.1 Phase C / C2).
    pub(crate) fn set_progress(&self, id: TransferId, transferred: u64, total: u64) {
        let emit = {
            let guard = self
                .transfers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(entry) = guard.get(&id) else {
                return;
            };
            let mut snapshot = entry
                .snapshot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            // Terminal states are never rolled back to Running by a late
            // in-flight progress callback (M8.1 Phase C / C3): after
            // cancel() the core may still deliver one progress chunk; the
            // bytes may advance but the state stays terminal.
            let terminal = matches!(
                snapshot.state,
                TransferState::Completed | TransferState::Failed | TransferState::Cancelled
            );
            snapshot.transferred_bytes = transferred;
            snapshot.total_bytes = Some(total);
            if !terminal {
                snapshot.state = TransferState::Running;
            }
            self.throttle_allows(entry, now_ms(), Some(transferred))
        };
        if emit && let Ok(snapshot) = self.get(id) {
            self.event_hub
                .publish(BackendEvent::TransferUpdated(snapshot));
        }
    }

    /// Called from the background batch task (sync context) after each
    /// plan item: updates the item counters and the current item, then
    /// publishes a time-throttled `TransferUpdated` event. Terminal states
    /// are never rolled back to Running (same rule as `set_progress`); the
    /// counters may still advance after a cancel.
    pub(crate) fn set_batch_items(
        &self,
        id: TransferId,
        completed: u64,
        failed: u64,
        current: Option<String>,
    ) {
        let emit = {
            let guard = self
                .transfers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(entry) = guard.get(&id) else {
                return;
            };
            let mut snapshot = entry
                .snapshot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let terminal = matches!(
                snapshot.state,
                TransferState::Completed | TransferState::Failed | TransferState::Cancelled
            );
            snapshot.completed_items = completed;
            snapshot.failed_items = failed;
            snapshot.current_item = current;
            if !terminal {
                snapshot.state = TransferState::Running;
            }
            // Item counters carry no byte delta; only the 100 ms time
            // threshold applies.
            self.throttle_allows(entry, now_ms(), None)
        };
        if emit && let Ok(snapshot) = self.get(id) {
            self.event_hub
                .publish(BackendEvent::TransferUpdated(snapshot));
        }
    }

    /// Attach the aggregated batch result before the terminal transition
    /// (the terminal `TransferUpdated` event then carries it). A terminal
    /// state is never overwritten: the late result of a cancelled transfer
    /// is dropped.
    pub(crate) fn set_batch_result(&self, id: TransferId, result: BatchTransferResultDto) {
        let guard = self
            .transfers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(entry) = guard.get(&id) else {
            return;
        };
        let mut snapshot = entry
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let terminal = matches!(
            snapshot.state,
            TransferState::Completed | TransferState::Failed | TransferState::Cancelled
        );
        if !terminal {
            snapshot.batch_result = Some(result);
        }
    }

    pub(crate) fn set_error(&self, id: TransferId, error: PublicError) {
        let guard = self
            .transfers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(entry) = guard.get(&id) {
            let mut snapshot = entry
                .snapshot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            snapshot.error = Some(error);
        }
    }
}

/// Build `TransferOptions` for a core call with a progress callback wired to
/// the registry.
pub(crate) fn transfer_options(
    registry: Arc<TransferRegistry>,
    id: TransferId,
    overwrite: bool,
) -> TransferOptions {
    let progress = registry.clone();
    TransferOptions {
        overwrite,
        offset: 0,
        progress: Some(Arc::new(move |progress_event| {
            progress.set_progress(id, progress_event.transferred, progress_event.total);
        })),
    }
}

pub(crate) fn request_options(token: CancellationToken) -> RequestOptions {
    RequestOptions::with_cancellation(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_registry() -> TransferRegistry {
        TransferRegistry::new(EventHub::new(8), 64, None)
    }

    #[test]
    fn cancel_for_session_only_cancels_that_sessions_transfers() {
        let registry = test_registry();
        let mut receiver = registry.event_hub.clone().subscribe();
        let entry1 = registry.register(registry.snapshot_for(
            SessionId(1),
            TransferDirectionDto::Download,
            "/remote/a.bin".to_string(),
            "/local/a.bin".to_string(),
        ));
        let entry2 = registry.register(registry.snapshot_for(
            SessionId(2),
            TransferDirectionDto::Upload,
            "/local/c.bin".to_string(),
            "/remote/c.bin".to_string(),
        ));
        let id1 = entry1.snapshot.lock().unwrap().id;
        let id2 = entry2.snapshot.lock().unwrap().id;

        // No tasks were spawned, so there are no join handles to wait on.
        let joins = registry.cancel_for_session(SessionId(1));
        assert!(joins.is_empty());

        let snapshot1 = registry.get(id1).expect("transfer 1");
        assert_eq!(snapshot1.state, TransferState::Cancelled);
        let snapshot2 = registry.get(id2).expect("transfer 2");
        assert_eq!(
            snapshot2.state,
            TransferState::Queued,
            "other session untouched"
        );

        // M8.1 Phase C / C5: the cancelled transfer's terminal event is
        // published synchronously (before any caller-level session event).
        let envelope = receiver.try_recv().expect("terminal event published");
        match envelope.event {
            BackendEvent::TransferUpdated(snapshot) => {
                assert_eq!(snapshot.id, id1);
                assert_eq!(snapshot.state, TransferState::Cancelled);
            }
            other => panic!("expected TransferUpdated(Cancelled), got {other:?}"),
        }
        assert!(matches!(
            receiver.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn cancel_is_idempotent() {
        let registry = test_registry();
        let entry = registry.register(registry.snapshot_for(
            SessionId(1),
            TransferDirectionDto::Download,
            "/a".to_string(),
            "/b".to_string(),
        ));
        let id = entry.snapshot.lock().unwrap().id;
        assert!(registry.cancel(id).is_ok());
        assert!(registry.cancel(id).is_ok(), "second cancel is a no-op");
        let snapshot = registry.get(id).unwrap();
        assert_eq!(snapshot.state, TransferState::Cancelled);
    }

    #[test]
    fn progress_does_not_roll_back_terminal_state() {
        // M8.1 Phase C / C3: a late in-flight progress callback after
        // cancel() must not flip the snapshot back to Running (bytes may
        // advance, state stays terminal).
        let registry = test_registry();
        let entry = registry.register(registry.snapshot_for(
            SessionId(1),
            TransferDirectionDto::Download,
            "/a".to_string(),
            "/b".to_string(),
        ));
        let id = entry.snapshot.lock().unwrap().id;
        registry.cancel(id).expect("cancel");
        registry.set_progress(id, 5000, 1_000_000);
        let snapshot = registry.get(id).unwrap();
        assert_eq!(snapshot.state, TransferState::Cancelled);
        assert_eq!(snapshot.transferred_bytes, 5000);
        assert!(snapshot.finished_at_ms.is_some());
    }

    #[tokio::test]
    async fn cancel_for_session_takes_join_after_public_terminal_state() {
        // Regression: shutdown may call cancel() before close_session(). The
        // snapshot is already Cancelled, but the background task can still
        // own the client and therefore its JoinHandle must still be returned.
        let registry = test_registry();
        let entry = registry.register(registry.snapshot_for(
            SessionId(1),
            TransferDirectionDto::Download,
            "/a".to_string(),
            "/b".to_string(),
        ));
        let id = entry.snapshot.lock().unwrap().id;
        let handle = tokio::spawn(async { std::future::pending::<()>().await });
        *entry
            .join
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(handle);

        registry.cancel(id).expect("pre-cancel");
        assert_eq!(registry.get(id).unwrap().state, TransferState::Cancelled);

        let mut joins = registry.cancel_for_session(SessionId(1));
        assert_eq!(joins.len(), 1, "terminal snapshot must not hide live task");
        let join = joins.pop().unwrap();
        join.abort();
        let _ = join.await;
        assert!(registry.cancel_for_session(SessionId(1)).is_empty());
    }
}
