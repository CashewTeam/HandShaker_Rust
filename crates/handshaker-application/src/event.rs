//! Backend event hub (M8 §5.7): a broadcast of `EventEnvelope` with a
//! monotonic sequence per Runtime. Subscribers may lag (broadcast `Lagged`
//! surfaced verbatim); closed subscriptions are identifiable; unknown core
//! events never panic the hub.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::broadcast;

use crate::dto::{DeviceDescriptor, DeviceId, SessionSnapshot};
use crate::transfer::TransferSnapshot;

/// Backend event kinds for v1 (Session and Transfer are fully implemented;
/// Clipboard/Media/RemoteFile are reserved for M8.9+).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum BackendEvent {
    RuntimeStarted,
    RuntimeStopping,
    DeviceAdded(DeviceDescriptor),
    DeviceUpdated(DeviceDescriptor),
    DeviceRemoved(DeviceId),
    SessionStateChanged(SessionSnapshot),
    TransferUpdated(TransferSnapshot),
    /// Reserved: clipboard changes (M8.9).
    #[allow(dead_code)]
    ClipboardChanged,
    /// Reserved: media-library changes (M8.9).
    #[allow(dead_code)]
    MediaChanged,
    /// Reserved: remote file changes (M8.9).
    #[allow(dead_code)]
    RemoteFileChanged,
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
#[derive(Clone)]
pub struct EventHub {
    tx: broadcast::Sender<EventEnvelope>,
    next_sequence: Arc<AtomicU64>,
}

impl EventHub {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity.max(1));
        Self {
            tx,
            next_sequence: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Publish one event; a full channel drops oldest entries for new
    /// subscribers the same way tokio broadcast does (Lagged on receivers).
    pub fn publish(&self, event: BackendEvent) {
        let envelope = EventEnvelope {
            sequence: self.next_sequence.fetch_add(1, Ordering::SeqCst),
            timestamp_ms: now_ms(),
            event,
        };
        let _ = self.tx.send(envelope);
    }

    /// Subscribe with the hub's fixed buffer. `recv()` errors: `Lagged(n)`
    /// when too slow, `Closed` after Runtime shutdown.
    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.tx.subscribe()
    }

    /// Number of currently active subscribers (diagnostics/tests).
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }
}
