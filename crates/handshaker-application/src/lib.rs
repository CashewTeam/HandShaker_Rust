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
    SessionId, SessionSnapshot, SessionState, StatFileRequest, TransportKind, UsbDetailDto,
};
pub use error::{AppResult, PublicError, PublicErrorCode, from_core_error};
pub use event::{BackendEvent, EventEnvelope, EventHub};
pub use file_plan::{
    ExecuteFilePlanRequest, FileConflictKind, FileOperationPlan, FilePlanConflict,
    FilePlanDirection, FilePlanItem, PlanDownloadRequest, PlanUploadRequest,
};
pub use media::{
    AudioAlbumDto, AudioFileDto, AudioLibraryDto, ExifDataDto, ImageAlbumDto, ImageFileDto,
    PhotoLibraryDto, ThumbnailsDto, VideoAlbumDto, VideoFileDto, VideoLibraryDto,
};
pub use runtime::{HandShakerRuntime, normalize_remote_path, resolve_remote_path};
pub use sync::{
    SyncActionDto, SyncConflictDto, SyncPlanDto, SyncProfileDto, SyncRunResultDto, SyncStatusDto,
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
/// (M8.1 Phase A: `session_client()` transition entry, event/transfer
/// semantics, documentation and fixtures). Breaking source-level changes are
/// allowed until the freeze; consumers must not treat preview versions as
/// stable. The freeze will drop the `-preview.N` suffix (see
/// `docs/application-api-v1.md`).
pub const APPLICATION_API_VERSION: &str = "1.0.0-preview.1";
