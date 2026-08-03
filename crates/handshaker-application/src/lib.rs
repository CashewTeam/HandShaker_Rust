//! handshaker-application: UI/CLI/binding-agnostic application service layer.
//!
//! Freeze contract (M8):
//! - never exposes prost types, session frames, or transport internals;
//! - never depends on CLI (no clap, no stdout, no JSON envelope);
//! - `HandShakerRuntime` is the single entry point for GUI/bindings;
//! - public DTOs and error codes are the v1 contract (see docs).

mod dto;
mod error;
mod event;
mod runtime;
mod transfer;

pub use dto::{
    AdbDetailDto, ConnectRequest, CreateDirectoryRequest, DeletePathsRequest, DeleteResultDto,
    DeviceDescriptor, DeviceId, DeviceInfoDto, FileEntryDto, ListDevicesRequest, ListFilesRequest,
    MovePathRequest, RuntimeConfig, SessionId, SessionSnapshot, SessionState, StatFileRequest,
    TransportKind, UsbDetailDto,
};
pub use error::{AppResult, PublicError, PublicErrorCode, from_core_error};
pub use event::{BackendEvent, EventEnvelope, EventHub};
pub use runtime::{HandShakerRuntime, normalize_remote_path, resolve_remote_path};
pub use transfer::{
    DownloadRequest, TransferDirectionDto, TransferId, TransferRegistry, TransferSnapshot,
    TransferState, UploadRequest,
};

#[cfg(test)]
mod tests;

/// Application API version; bumped only on breaking changes of the frozen
/// contract above (independent of the Rust crate version).
pub const APPLICATION_API_VERSION: &str = "1.0.0";
