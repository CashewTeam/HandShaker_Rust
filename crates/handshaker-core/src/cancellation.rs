use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tokio::sync::watch;

/// A cloneable signal used to request cancellation of one operation.
#[derive(Clone, Debug)]
pub struct CancellationToken {
    inner: Arc<CancellationState>,
}

#[derive(Debug)]
struct CancellationState {
    cancelled: AtomicBool,
    changes: watch::Sender<bool>,
}

impl CancellationToken {
    /// Create a token in the not-cancelled state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Request cancellation for every operation using this token.
    pub fn cancel(&self) {
        if !self.inner.cancelled.swap(true, Ordering::SeqCst) {
            self.inner.changes.send_replace(true);
        }
    }

    /// Return whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    pub(crate) async fn cancelled(&self) {
        let mut changes = self.inner.changes.subscribe();
        if *changes.borrow() || self.is_cancelled() {
            return;
        }
        let _ = changes.changed().await;
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        let (changes, _) = watch::channel(false);
        Self {
            inner: Arc::new(CancellationState {
                cancelled: AtomicBool::new(false),
                changes,
            }),
        }
    }
}

/// Options shared by public requests that support cancellation.
#[derive(Clone, Debug, Default)]
pub struct RequestOptions {
    /// Optional token that stops waiting for this request.
    pub cancellation: Option<CancellationToken>,
}

impl RequestOptions {
    /// Return options using the supplied cancellation token.
    pub fn with_cancellation(cancellation: CancellationToken) -> Self {
        Self {
            cancellation: Some(cancellation),
        }
    }
}

/// The side that requested cancellation of a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CancellationOrigin {
    /// The caller requested cancellation through a token.
    Local { flag_sent: bool },
    /// The phone sent a CANCEL_REQUEST for this request.
    Remote { error_code: Option<i32> },
}

/// Details carried by a cancelled request error or event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancellationInfo {
    /// Session id of the cancelled request.
    pub sid: u32,
    /// Local or phone-side cancellation source.
    pub origin: CancellationOrigin,
    /// Whether the connection had to be terminated for this cancellation.
    pub connection_closed: bool,
}
