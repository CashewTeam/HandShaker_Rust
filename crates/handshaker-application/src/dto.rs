//! Application-layer DTOs: stable, UI/binding-independent values that never
//! expose prost, session frames, or transport internals.
//!
//! Freeze rules (M8):
//! - v1 field names are not renamed casually;
//! - enum discriminants are never reused (see `TransportKind`/`SessionState`);
//! - time is Unix milliseconds; byte counts are `u64`;
//! - unknown enum values on decode are rejected or mapped to an explicit
//!   unknown variant, never guessed.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Stable application-layer device identifier. Not the same as any low-level
/// temporary address; ADB uses the serial, USB uses the accessory location,
/// WiFi uses the device UUID when known (fallback: a stable temporary id).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceId(pub String);

/// Transport kind of a discovered device.
///
/// Frozen v1 contract: the *name* (snake_case in JSON) is the stable wire
/// value; the numeric discriminant is also fixed and never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TransportKind {
    /// ADB over USB/TCP forward.
    Adb = 1,
    /// WiFi direct (mDNS-discovered or manual IP:PORT).
    Wifi = 2,
    /// USB AOA accessory channel.
    UsbAccessory = 3,
}

/// ADB-specific detail (preserved so CLI JSON stays byte-compatible).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdbDetailDto {
    /// ADB state, such as `device` or `unauthorized`.
    pub state: String,
    pub product: Option<String>,
    pub model: Option<String>,
    pub device: Option<String>,
}

/// USB AOA detail (preserved for CLI JSON compatibility).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsbDetailDto {
    pub bus_number: u8,
    pub serial: Option<String>,
    pub vendor_id: u16,
    pub product_id: u16,
    /// Accessory mode token as the core serializes it (`Accessory`/`Plain`).
    pub mode: String,
}

/// A discovered device, UI-ready.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceDescriptor {
    /// Identity of this discovery entry. For Wi-Fi this is only a discovery
    /// *endpoint* id (the mDNS SRV port is dynamic) and must never be used
    /// as a long-lived device identity (Phase D / D2).
    pub id: DeviceId,
    /// Stable identity reconciled from the connected phone's `phone_id`
    /// (`phone:<uuid>`); `None` until a connection reports one (or for
    /// transports that never report one). UI should prefer
    /// `stable_id ?? id` for identity.
    #[serde(default)]
    pub stable_id: Option<DeviceId>,
    pub display_name: Option<String>,
    pub model: Option<String>,
    pub transport: TransportKind,
    pub transport_address: String,
    pub available: bool,
    /// Transport-specific detail, when the transport reports it.
    pub adb: Option<AdbDetailDto>,
    pub usb: Option<UsbDetailDto>,
}

/// Application-layer session identifier (u64, no pointer handles).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub u64);

/// Fixed session states (see M8 plan §5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionState {
    Connecting = 1,
    Ready = 2,
    Disconnecting = 3,
    Closed = 4,
    Failed = 5,
}

/// UI-ready device information snapshot (Phase D / D2: full core coverage).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DeviceInfoDto {
    pub serial: String,
    pub phone_id: Option<String>,
    pub name: Option<String>,
    pub model: Option<String>,
    pub brand: Option<String>,
    pub manufacturer: Option<String>,
    pub smartisan_version: Option<String>,
    pub apk_version: Option<String>,
    pub apk_version_name: Option<String>,
    pub root_path: String,
    /// External storage path, when reported (e.g. `/storage/XXXX-XXXX`).
    #[serde(default)]
    pub external_storage_path: Option<String>,
    /// Total internal storage size in bytes, when reported.
    #[serde(default)]
    pub disk_size: Option<u64>,
    /// Used internal storage size in bytes, when reported.
    #[serde(default)]
    pub used_disk_size: Option<u64>,
    /// Battery percentage, when reported.
    #[serde(default)]
    pub battery_percentage: Option<u32>,
    /// Whether the phone reports a locked screen.
    #[serde(default)]
    pub phone_locked: Option<bool>,
}

/// One directory entry (mirrors `RemoteFile` without core types).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntryDto {
    pub path: String,
    pub size: u64,
    pub created_at_ms: Option<u64>,
    pub modified_at_ms: Option<u64>,
    pub is_directory: bool,
    pub checksum: Option<String>,
    pub is_trash: Option<bool>,
    pub media_id: Option<u64>,
}

/// One file whose metadata should be written back to the phone media store
/// (UPDATE_FILE_INFO; mirrors core `RemoteFile` without core types).
/// Field names are the stable snake_case JSON contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateFileInfoItemDto {
    /// Absolute remote path.
    pub path: String,
    /// File size in bytes.
    pub size: u64,
    /// Creation timestamp, when reported.
    pub created_at: Option<u64>,
    /// Modification timestamp, when reported.
    pub modified_at: Option<u64>,
    /// Whether the remote entry is a directory.
    pub is_directory: bool,
    /// MD5/checksum reported by the phone, when available.
    pub checksum: Option<String>,
    /// Whether the phone marks the entry as trash.
    pub is_trash: Option<bool>,
    /// Media-store identifier, when available.
    pub id: Option<u64>,
    /// Phone-side extension data (JSON with star/orientation/updateTime),
    /// opaque to the host.
    pub ext_data: Option<String>,
}

/// Snapshot of one open session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub id: SessionId,
    pub device: DeviceDescriptor,
    pub device_info: DeviceInfoDto,
    pub state: SessionState,
    pub connected_at_ms: u64,
    pub last_activity_at_ms: Option<u64>,
}

/// Round-trip latency of a device ping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PingResultDto {
    pub round_trip_ms: u64,
}

/// Runtime configuration (Rust-native; FFI uses `FfiRuntimeConfig`).
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub adb_path: PathBuf,
    pub default_timeout: Duration,
    pub heartbeat_interval: Duration,
    /// Computer name reported to the phone during the Wi-Fi handshake.
    /// `None` falls back to the host OS name.
    pub host_name: Option<String>,
    pub state_dir: Option<PathBuf>,
    pub wire_log: Option<PathBuf>,
    /// P2-4: dump payload bytes into the wire log (default false). The
    /// log itself is already sensitive; payloads may add clipboard text,
    /// paths and media bytes.
    pub wire_log_payload: bool,
    pub event_capacity: usize,
    /// Bounded transfer history (M8.1 Phase C / C4): keep at most this many
    /// finished transfers (oldest evicted first; live transfers are never
    /// evicted to make room).
    pub transfer_history_capacity: usize,
    /// Optional TTL for finished transfer entries; `None` keeps finished
    /// entries until capacity eviction.
    pub transfer_history_ttl: Option<Duration>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            adb_path: PathBuf::from("adb"),
            default_timeout: Duration::from_secs(30),
            heartbeat_interval: Duration::from_secs(10),
            host_name: None,
            state_dir: None,
            wire_log: None,
            wire_log_payload: false,
            event_capacity: 1024,
            transfer_history_capacity: 64,
            transfer_history_ttl: None,
        }
    }
}

/// Device listing request.
#[derive(Debug, Clone)]
pub struct ListDevicesRequest {
    pub include_adb: bool,
    pub include_wifi: bool,
    pub include_usb: bool,
    pub wifi_browse_timeout: Duration,
}

impl Default for ListDevicesRequest {
    fn default() -> Self {
        Self {
            include_adb: true,
            include_wifi: true,
            include_usb: true,
            wifi_browse_timeout: Duration::from_secs(3),
        }
    }
}

/// Connection request; `device` must come from a prior `list_devices` result
/// or be constructed with a matching transport target.
#[derive(Debug, Clone)]
pub struct ConnectRequest {
    pub device: DeviceDescriptor,
}

/// Directory listing request.
#[derive(Debug, Clone)]
pub struct ListFilesRequest {
    pub session_id: SessionId,
    /// Absolute remote path; relative paths are resolved against the device
    /// root by the application layer.
    pub path: String,
    /// Recursion depth (0 = one level).
    pub depth: u32,
}

/// Stat one remote path (M8 §5.5).
#[derive(Debug, Clone)]
pub struct StatFileRequest {
    pub session_id: SessionId,
    pub path: String,
}

/// Create one remote directory (M8 §5.5).
#[derive(Debug, Clone)]
pub struct CreateDirectoryRequest {
    pub session_id: SessionId,
    pub path: String,
}

/// Count files under a remote directory. `exclusions` are the protocol
/// exclusion patterns (`SspGetFileCountRequest.exclusion_pattern`).
#[derive(Debug, Clone)]
pub struct CountFilesRequest {
    pub session_id: SessionId,
    pub path: String,
    pub depth: u32,
    pub exclusions: Vec<String>,
}

/// Move/rename a remote path (M8 §5.5).
#[derive(Debug, Clone)]
pub struct MovePathRequest {
    pub session_id: SessionId,
    pub source: String,
    pub target: String,
}

/// Delete remote paths (M8 §5.5). `trash` moves to trash when the phone
/// supports it; `sync` marks the request as part of synchronization.
#[derive(Debug, Clone)]
pub struct DeletePathsRequest {
    pub session_id: SessionId,
    pub paths: Vec<String>,
    pub trash: bool,
    pub sync: bool,
}

/// Result of a delete request. `deleted` carries the confirmed deleted
/// entries in `FileEntryDto` shape (mirrors core `RemoteFile`), so callers
/// can render the same JSON contract as the legacy core response.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DeleteResultDto {
    /// Entries the phone confirmed deleted (post-delete snapshot shape).
    pub deleted: Vec<FileEntryDto>,
}

/// Update file metadata on the phone (UPDATE_FILE_INFO). `files` carries
/// the paths plus the fields the phone should write back into its media
/// store; `is_sync` asks the phone to feed the change into its sync
/// manager. Serialized/deserialized for the FFI request JSON; the FFI
/// entry point always overrides `session_id` from the call argument.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateFileInfoRequest {
    pub session_id: SessionId,
    pub files: Vec<UpdateFileInfoItemDto>,
    pub is_sync: bool,
}

/// One clipboard entry (mirrors core `ClipboardEntry`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ClipboardEntryDto {
    pub text: String,
    pub timestamp_ms: i64,
}

// ---- phone-initiated change events (M8.1 Phase C / C1 bridge) ----

/// Media library that produced a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MediaKindDto {
    Photo,
    Video,
    Audio,
}

/// One media entry inside a library change (a stable subset of core
/// `MediaItem`; album payloads are intentionally not bridged yet).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct MediaChangeItemDto {
    pub media_id: Option<u64>,
    pub path: Option<String>,
    pub size: Option<u64>,
    pub created_at: Option<u64>,
    pub modified_at: Option<u64>,
    pub mime_type: Option<String>,
    pub title: Option<String>,
    pub album_name: Option<String>,
}

/// A media library change pushed by the phone. The library category is
/// `media_kind` (not `kind`) so the event JSON keeps its own `kind` tag
/// distinct from the payload (serde internally-tagged contract).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MediaChangeDto {
    pub media_kind: MediaKindDto,
    pub added: Vec<MediaChangeItemDto>,
    pub deleted: Vec<MediaChangeItemDto>,
    pub updated: Vec<MediaChangeItemDto>,
}

/// Category of a phone-initiated remote file change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteFileChangeKind {
    /// A directory monitor event.
    DirectoryChanged,
    /// A synchronization file change.
    FileChanged,
    /// A one-shot photo synchronization response.
    PhotoSyncChanged,
    /// A synchronization monitor response.
    SyncMonitorChanged,
}

/// Summarized remote file change. `change_kind` + `paths` keep the v1
/// contract byte-identical; `files` and `statuses` are optional v1.1
/// additions for watch/sync incremental use — full metadata and the
/// per-path `FileChangeStatus` snake_case string when the phone supplied
/// them. Both default to empty and are skipped when empty, so legacy JSON
/// without the new keys decodes and serializes unchanged. The category is
/// `change_kind` (not `kind`) to keep the event JSON `kind` tag distinct
/// from the payload.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RemoteFileChangeDto {
    pub change_kind: RemoteFileChangeKind,
    pub paths: Vec<String>,
    /// Full metadata for each changed path, parallel to `paths` when the
    /// phone supplied it (empty when only paths are known).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<FileEntryDto>,
    /// Per-path `FileChangeStatus` snake_case strings (e.g. "added",
    /// "deleted", "modified"), parallel to `paths`; empty when unknown
    /// (directory-monitor events carry no status).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub statuses: Vec<String>,
}
