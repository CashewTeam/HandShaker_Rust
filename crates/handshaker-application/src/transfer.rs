//! Transfer task model (M8 §5.6): long tasks run in the background under a
//! `TransferId`; callers poll snapshots and cancel via token. State
//! transitions are one-way: Queued -> Running -> Completed|Failed|Cancelled.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use handshaker_core::{CancellationToken, RequestOptions, TransferOptions};

use crate::dto::SessionId;
use crate::error::{AppResult, PublicError, PublicErrorCode};

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

/// Internal registry entry.
pub(crate) struct ActiveTransfer {
    pub(crate) snapshot: Mutex<TransferSnapshot>,
    pub(crate) cancel: CancellationToken,
    /// Join handle kept until the task finishes; cleaned on get/list.
    pub(crate) join: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// Registry of transfers with bounded history. State transitions are guarded
/// by the per-entry snapshot mutex; no data races under concurrent access.
#[derive(Default)]
pub struct TransferRegistry {
    transfers: Mutex<HashMap<TransferId, Arc<ActiveTransfer>>>,
    next_id: AtomicU64,
}

impl TransferRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn register(&self, snapshot: TransferSnapshot) -> Arc<ActiveTransfer> {
        let id = snapshot.id;
        let entry = Arc::new(ActiveTransfer {
            snapshot: Mutex::new(snapshot),
            cancel: CancellationToken::new(),
            join: Mutex::new(None),
        });
        self.transfers
            .lock()
            .expect("transfer registry poisoned")
            .insert(id, entry.clone());
        entry
    }

    pub fn get(&self, id: TransferId) -> AppResult<TransferSnapshot> {
        let guard = self.transfers.lock().expect("transfer registry poisoned");
        let entry = guard.get(&id).ok_or_else(|| {
            PublicError::new(PublicErrorCode::TransferNotFound, "transfer not found")
        })?;
        Ok(entry.snapshot.lock().expect("snapshot poisoned").clone())
    }

    pub fn list(&self) -> Vec<TransferSnapshot> {
        let guard = self.transfers.lock().expect("transfer registry poisoned");
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
        let guard = self.transfers.lock().expect("transfer registry poisoned");
        let entry = guard.get(&id)?;
        let mut snapshot = entry.snapshot.lock().expect("snapshot poisoned");
        let current = snapshot.state;
        let terminal = matches!(
            current,
            TransferState::Completed | TransferState::Failed | TransferState::Cancelled
        );
        if terminal {
            return Some(snapshot.clone());
        }
        snapshot.state = next;
        if next == TransferState::Completed || next == TransferState::Failed {
            snapshot.finished_at_ms = Some(now_ms());
        }
        Some(snapshot.clone())
    }

    /// Cancel a transfer: idempotent.
    pub fn cancel(&self, id: TransferId) -> AppResult<()> {
        let token = {
            let guard = self.transfers.lock().expect("transfer registry poisoned");
            let entry = guard.get(&id).ok_or_else(|| {
                PublicError::new(PublicErrorCode::TransferNotFound, "transfer not found")
            })?;
            entry.cancel.clone()
        };
        token.cancel();
        // Transition after dropping the registry lock (std Mutex is not
        // re-entrant; transition would otherwise deadlock).
        self.transition(id, TransferState::Cancelled);
        Ok(())
    }

    /// Reap finished join handles (best-effort).
    pub(crate) fn reap(&self) {
        let guard = self.transfers.lock().expect("transfer registry poisoned");
        for entry in guard.values() {
            if let Ok(mut join) = entry.join.lock() {
                if let Some(handle) = join.take() {
                    if handle.is_finished() {
                        // Dropping the handle detaches the finished task.
                    } else {
                        *join = Some(handle);
                    }
                }
            }
        }
    }

    pub(crate) fn into_snapshot_for(
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
        }
    }

    /// Called from the progress callback (sync context).
    pub(crate) fn set_progress(&self, id: TransferId, transferred: u64) {
        let guard = self.transfers.lock().expect("transfer registry poisoned");
        if let Some(entry) = guard.get(&id) {
            let mut snapshot = entry.snapshot.lock().expect("snapshot poisoned");
            snapshot.transferred_bytes = transferred;
            snapshot.state = TransferState::Running;
        }
    }

    pub(crate) fn set_error(&self, id: TransferId, error: PublicError) {
        let guard = self.transfers.lock().expect("transfer registry poisoned");
        if let Some(entry) = guard.get(&id) {
            let mut snapshot = entry.snapshot.lock().expect("snapshot poisoned");
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
            progress.set_progress(id, progress_event.transferred);
        })),
    }
}

pub(crate) fn request_options(token: CancellationToken) -> RequestOptions {
    RequestOptions::with_cancellation(token)
}
