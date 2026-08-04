//! handshaker-application: UI/CLI/binding-agnostic application service layer.
//!
//! Freeze contract (M8):
//! - never exposes prost types, session frames, or transport internals;
//! - never depends on CLI (no clap, no stdout, no JSON envelope);
//! - `HandShakerRuntime` is the single entry point for GUI/bindings;
//! - public DTOs and error codes are the v1 contract (see docs).

mod discovery;
mod dto;
mod error;
mod event;
mod file_plan;
mod media;
mod runtime;
mod sync;
mod transfer;
mod trust;

pub use discovery::{DeviceDiscoveryResult, DeviceDiscoveryWarning};
pub use dto::{
    AdbDetailDto, ClipboardEntryDto, ConnectRequest, CountFilesRequest, CreateDirectoryRequest,
    DeletePathsRequest, DeleteResultDto, DeviceDescriptor, DeviceId, DeviceInfoDto, FileEntryDto,
    ListDevicesRequest, ListFilesRequest, MediaChangeDto, MediaChangeItemDto, MediaKindDto,
    MovePathRequest, PingResultDto, RemoteFileChangeDto, RemoteFileChangeKind, RuntimeConfig,
    SessionId, SessionSnapshot, SessionState, StatFileRequest, TransportKind,
    UpdateFileInfoItemDto, UpdateFileInfoRequest, UsbDetailDto,
};
pub use error::{AppResult, PublicError, PublicErrorCode, from_core_error};
pub use event::{BackendEvent, EventEnvelope, EventHub};
pub use file_plan::{
    ExecuteFilePlanRequest, FileConflictKind, FileOperationPlan, FilePlanConflict,
    FilePlanDirection, FilePlanItem, PlanDownloadRequest, PlanUploadRequest,
};
pub use media::{
    AudioAlbumDto, AudioFileDto, AudioLibraryDto, ExifDataDto, ImageAlbumDto, ImageFileDto,
    MEDIA_PAGE_DEFAULT_LIMIT, MEDIA_PAGE_MAX_LIMIT, PhotoLibraryDto, ThumbnailsDto, VideoAlbumDto,
    VideoFileDto, VideoLibraryDto, merge_audio_library, merge_photo_library, merge_video_library,
};
pub use runtime::{HandShakerRuntime, normalize_remote_path, resolve_remote_path};
pub use sync::{
    SyncActionDto, SyncConflictDto, SyncLedgerStatusDto, SyncPlanDto, SyncProfileDto,
    SyncRunResultDto, SyncStatusDto,
};
pub use transfer::{
    BatchTransferItemDto, BatchTransferRequest, BatchTransferResultDto, DownloadRequest,
    TransferDirectionDto, TransferFailureDto, TransferId, TransferRegistry, TransferSnapshot,
    TransferState, TreeTransferDto, UploadRequest,
};
pub use trust::{RemoveTrustRequest, RemoveTrustResult, ResetWifiTrustRequest, TrustRecordDto};

#[cfg(test)]
mod tests;

/// Application API version; bumped only on breaking changes of the contract
/// above (independent of the Rust crate version).
///
/// Current status: `preview` — the v1 contract is still being finalized
/// (Phase D: `session_client()` transition entry removed with the last CLI
/// call site; event/transfer semantics, documentation and fixtures). Breaking
/// source-level changes are allowed until the freeze; consumers must not
/// treat preview versions as stable. The freeze will drop the
/// `-preview.N` suffix (see `docs/application-api-v1.md`).
///
/// Frozen as `1.0.0` on 2026-08-04 (audit DoD 16/16, see
/// `docs/HandShaker_Rust_Code_Audit_ad96fb4.md` §8): the v1 contract is
/// stable — breaking source-level/JSON changes now require a major bump.
/// Note: `RuntimeStarted`/`DeviceAdded`/`DeviceRemoved` event variants are
/// part of the contract (Swift handles them) but are not emitted by the
/// runtime yet (no discovery watcher).
pub const APPLICATION_API_VERSION: &str = "1.0.0";

/// P1-7: version of the **JSON wire contract** (requests, responses,
/// events, DTO shapes) exposed over the FFI, independent of the C ABI
/// version and the Application API string. Bump on any breaking JSON
/// change (renamed field, changed nesting, new required field, enum token
/// change); Swift verifies this at runtime-creation time via
/// `hs_runtime_diagnostics`. v1 = the contract as of ABI 1.5.0 (nested
/// `device_updated.device`, struct `sync_watch_applied` with
/// profile_id/session_id, SyncStatusDto reconciliation fields).
pub const JSON_CONTRACT_VERSION: u32 = 1;
