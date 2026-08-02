//! Reusable Rust client and CLI support for the Smartisan HandShaker SSP.
//!
//! The public API exposes stable domain values and connection configuration.
//! Wire frames, generated protobuf messages, handshake keys, and session
//! routing remain internal implementation details.

mod cancellation;
mod client;
mod domain;
mod error;
mod event_decode;
mod events;
pub mod i18n;
mod protocol;
mod session;
mod state;
#[cfg(test)]
mod test_support;
mod transport;

pub use cancellation::{CancellationInfo, CancellationOrigin, CancellationToken, RequestOptions};
pub use client::{ClientOptions, ConnectionTarget, EventCallbacks, HandShakerClient, PingResult};
pub use domain::{
    AdbDevice, ClipboardEntry, DeleteOptions, DeviceInfo, RemoteFile, RemoteIoError,
    TransferDirection, TransferOptions, TransferProgress, TransferProgressCallback,
};
pub use error::{Error, ErrorCode, Result};
pub use events::{
    ClientEvent, EventFilter, EventKind, EventStreamError, EventSubscription, FileChange,
    FileChangeStatus, FileEvent, FileEventKind, MediaAlbum, MediaItem, MediaKind,
    MediaLibraryChange, PhotoSyncChange, SyncMonitorChange, UnknownEvent, UnknownEventReason,
};
