//! Reusable Rust client and CLI support for the Smartisan HandShaker SSP.
//!
//! The public API exposes stable domain values and connection configuration.
//! Wire frames, generated protobuf messages, handshake keys, and session
//! routing remain internal implementation details.

mod cancellation;
mod client;
mod discovery;
mod domain;
mod error;
mod event_decode;
mod events;
mod exif_parser;
pub mod i18n;
pub mod media_merge;
mod protocol;
mod session;
mod state;
mod sync;
mod sync_journal;
mod sync_store;
#[cfg(test)]
mod test_support;
mod transport;

/// Discover USB AOA accessory devices (VID 0x18d1 accessory interfaces).
pub fn list_usb_accessories() -> Result<Vec<UsbAccessory>> {
    transport::usb::list_accessories()
}

pub use transport::usb::{AccessoryMode, UsbAccessory};

pub use cancellation::{CancellationInfo, CancellationOrigin, CancellationToken, RequestOptions};
pub use client::{ClientOptions, ConnectionTarget, EventCallbacks, HandShakerClient, PingResult};
pub use domain::{
    AdbDevice, AudioAlbum, AudioFile, AudioLibrary, BatchTransferFailure, BatchTransferItem,
    BatchTransferOptions, BatchTransferProgress, BatchTransferResult, ClipboardEntry,
    DeleteOptions, DeviceInfo, ExifData, ImageAlbum, ImageFile, PhotoLibrary, PhotoSyncResult,
    RemoteFile, RemoteIoError, SyncConfig, SyncDiff, SyncFileRecord, SyncSnapshot, Thumbnails,
    TransferDirection, TransferOptions, TransferProgress, TransferProgressCallback,
    TrustRecordInfo, VideoAlbum, VideoFile, VideoLibrary, WifiDevice,
};
pub use error::{Error, ErrorCode, Result};
pub use events::{
    ClientEvent, EventFilter, EventKind, EventStreamError, EventSubscription, FileChange,
    FileChangeStatus, FileEvent, FileEventKind, MediaAlbum, MediaItem, MediaKind,
    MediaLibraryChange, PhotoSyncChange, SyncMonitorChange, UnknownEvent, UnknownEventReason,
};
pub use state::{State, StateStore};
pub use sync::{
    SyncRunResult, apply_file_change_with_checkpoint, check_conflicts,
    execute_plan_with_checkpoint, local_destination, plan_diff,
};
pub use sync_journal::{PendingSyncAction, SyncJournal};
pub use sync_store::{
    SyncLedgerIdentity, SyncStore, default_config_dir, ledger_scope_key, normalize_root,
    pc_id_from_host_uuid, sync_config,
};
