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
    pub id: DeviceId,
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

/// UI-ready device information snapshot.
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
    pub state_dir: Option<PathBuf>,
    pub wire_log: Option<PathBuf>,
    pub event_capacity: usize,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            adb_path: PathBuf::from("adb"),
            default_timeout: Duration::from_secs(30),
            heartbeat_interval: Duration::from_secs(10),
            state_dir: None,
            wire_log: None,
            event_capacity: 1024,
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
