//! Backend event hub (M8 §5.7): a broadcast of `EventEnvelope` with a
//! monotonic sequence per Runtime. Subscribers may lag (broadcast `Lagged`
//! surfaced verbatim); closed subscriptions are identifiable; unknown core
//! events never panic the hub.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::broadcast;

use crate::dto::{
    ClipboardEntryDto, DeviceDescriptor, DeviceId, MediaChangeDto, RemoteFileChangeDto, SessionId,
    SessionSnapshot,
};
use crate::transfer::TransferSnapshot;

/// Backend event kinds for v1 (Session and Transfer are fully implemented;
/// Clipboard/Media/RemoteFile carry phone-initiated change payloads bridged
/// from core (M8.1 Phase C / C1)).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum BackendEvent {
    RuntimeStarted,
    RuntimeStopping,
    DeviceAdded(DeviceDescriptor),
    DeviceUpdated {
        session_id: SessionId,
        device: DeviceDescriptor,
    },
    DeviceRemoved {
        device_id: DeviceId,
    },
    SessionStateChanged(Box<SessionSnapshot>),
    TransferUpdated(TransferSnapshot),
    /// The core session died under a request or transfer (M8.1 Phase C / C5):
    /// the session is marked `Failed`, its transfers cancelled, and a
    /// `SessionStateChanged(Failed)` event follows.
    ConnectionLost {
        session_id: SessionId,
    },
    /// Clipboard history pushed by the phone.
    ClipboardChanged {
        session_id: SessionId,
        entries: Vec<ClipboardEntryDto>,
    },
    /// A media library change pushed by the phone.
    MediaChanged {
        session_id: SessionId,
        change: MediaChangeDto,
    },
    /// A remote file change (directory monitor / sync) pushed by the phone.
    RemoteFileChanged {
        session_id: SessionId,
        change: RemoteFileChangeDto,
    },
    /// One debounced sync-watch batch was applied incrementally (Phase D /
    /// D6): carries the batch result so GUIs and the CLI can render progress
    /// without polling. `Warning` follows when the batch hit failures.
    /// A sync watch batch was applied incrementally (P1-2: carries
    /// `profile_id` and `session_id` so multi-profile/multi-device UIs can
    /// route the result; the batch result itself is nested under `result`).
    SyncWatchApplied {
        profile_id: String,
        session_id: SessionId,
        result: Box<crate::sync::SyncRunResultDto>,
    },
    Warning(crate::error::PublicError),
}

/// One delivered event with monotonic sequencing.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EventEnvelope {
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub event: BackendEvent,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Broadcast hub owned by a Runtime. `publish` is synchronous (non-blocking
/// send), so progress callbacks can emit events without an async context.
///
/// Closing (M8.1 Phase B / B3): the sender is wrapped in `Option` so
/// `close()` can drop it; every receiver then observes `RecvError::Closed`
/// instead of timing out forever (previously the sender lived as long as the
/// `Arc<RuntimeInner>`, so subscriptions never saw a closed stream).
#[derive(Clone)]
pub struct EventHub {
    tx: Arc<std::sync::Mutex<Option<broadcast::Sender<EventEnvelope>>>>,
    next_sequence: Arc<AtomicU64>,
}

impl EventHub {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity.max(1));
        Self {
            tx: Arc::new(std::sync::Mutex::new(Some(tx))),
            next_sequence: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Publish one event; a full channel drops oldest entries for new
    /// subscribers the same way tokio broadcast does (Lagged on receivers).
    /// Publishing after `close()` is a silent no-op (the hub is gone).
    pub fn publish(&self, event: BackendEvent) {
        let envelope = EventEnvelope {
            sequence: self.next_sequence.fetch_add(1, Ordering::SeqCst),
            timestamp_ms: now_ms(),
            event,
        };
        let guard = self
            .tx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(tx) = guard.as_ref() {
            let _ = tx.send(envelope);
        }
    }

    /// Subscribe with the hub's fixed buffer. `recv()` errors: `Lagged(n)`
    /// when too slow, `Closed` after the hub is closed (runtime shutdown).
    /// Subscribing after close yields a receiver that immediately reports
    /// `Closed`.
    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        let guard = self
            .tx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match guard.as_ref() {
            Some(tx) => tx.subscribe(),
            None => {
                let (tx, rx) = broadcast::channel(1);
                drop(tx);
                rx
            }
        }
    }

    /// Close the hub: drops the sender so all receivers observe `Closed`.
    /// Idempotent. After this, `publish` is a no-op and `subscribe` hands
    /// out an already-closed receiver.
    pub fn close(&self) {
        let mut guard = self
            .tx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.take();
    }

    /// Number of currently active subscribers (diagnostics/tests).
    pub fn subscriber_count(&self) -> usize {
        let guard = self
            .tx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.as_ref().map(|tx| tx.receiver_count()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::{FileEntryDto, RemoteFileChangeKind};

    fn sample_file(path: &str) -> FileEntryDto {
        FileEntryDto {
            path: path.to_string(),
            size: 7,
            created_at_ms: Some(1000),
            modified_at_ms: Some(2000),
            is_directory: false,
            checksum: Some("abc".to_string()),
            is_trash: Some(false),
            media_id: Some(42),
        }
    }

    #[test]
    fn remote_file_change_v1_payload_stays_byte_identical() {
        // v1 contract: only `change_kind` + `paths` serialize when the
        // optional metadata is absent (fixtures predating files/statuses
        // must keep producing identical JSON).
        let change = RemoteFileChangeDto {
            change_kind: RemoteFileChangeKind::DirectoryChanged,
            paths: vec!["/storage/emulated/0/DCIM".to_string()],
            files: Vec::new(),
            statuses: Vec::new(),
        };
        let value = serde_json::to_value(&change).expect("serialize");
        assert_eq!(value["change_kind"], "directory_changed");
        assert_eq!(value["paths"][0], "/storage/emulated/0/DCIM");
        assert!(value.get("files").is_none(), "empty files must be skipped");
        assert!(
            value.get("statuses").is_none(),
            "empty statuses must be skipped"
        );
    }

    #[test]
    fn remote_file_change_accepts_legacy_json_without_metadata() {
        let value = serde_json::json!({
            "change_kind": "file_changed",
            "paths": ["/storage/emulated/0/a.txt"],
        });
        let change: RemoteFileChangeDto =
            serde_json::from_value(value).expect("legacy payload decodes");
        assert_eq!(change.change_kind, RemoteFileChangeKind::FileChanged);
        assert_eq!(change.paths, vec!["/storage/emulated/0/a.txt"]);
        assert!(change.files.is_empty(), "files default to empty");
        assert!(change.statuses.is_empty(), "statuses default to empty");
    }

    #[test]
    fn remote_file_change_round_trips_full_metadata() {
        let change = RemoteFileChangeDto {
            change_kind: RemoteFileChangeKind::FileChanged,
            paths: vec!["/storage/emulated/0/a.txt".to_string()],
            files: vec![sample_file("/storage/emulated/0/a.txt")],
            statuses: vec!["modified".to_string()],
        };
        let value = serde_json::to_value(&change).expect("serialize");
        assert_eq!(value["files"][0]["path"], "/storage/emulated/0/a.txt");
        assert_eq!(value["files"][0]["size"], 7);
        assert_eq!(value["statuses"][0], "modified");
        let back: RemoteFileChangeDto = serde_json::from_value(value).expect("round trip decodes");
        assert_eq!(back, change);
    }
}
