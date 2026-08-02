use std::sync::Arc;

use serde::Serialize;

/// Device information returned by the phone during connection.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct DeviceInfo {
    /// ADB serial selected for this connection.
    pub serial: String,
    /// Phone-side stable identifier, when reported.
    pub phone_id: Option<String>,
    /// User-visible phone name, when reported.
    pub name: Option<String>,
    /// Phone model, when reported.
    pub model: Option<String>,
    /// Product brand, when reported.
    pub brand: Option<String>,
    /// Product manufacturer, when reported.
    pub manufacturer: Option<String>,
    /// Smartisan OS version, when reported.
    pub smartisan_version: Option<String>,
    /// APK version code, when reported.
    pub apk_version: Option<String>,
    /// APK version name, when reported.
    pub apk_version_name: Option<String>,
    /// Root path used for relative remote paths.
    pub root_path: String,
    /// External storage path, when reported.
    pub external_storage_path: Option<String>,
    /// Total internal storage size in bytes, when reported.
    pub disk_size: Option<u64>,
    /// Used internal storage size in bytes, when reported.
    pub used_disk_size: Option<u64>,
    /// Battery percentage, when reported.
    pub battery_percentage: Option<u32>,
    /// Whether the phone reports a locked screen.
    pub phone_locked: Option<bool>,
}

/// One device returned by adb devices -l.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AdbDevice {
    /// ADB serial.
    pub serial: String,
    /// ADB state, such as device or unauthorized.
    pub state: String,
    /// ADB product identifier.
    pub product: Option<String>,
    /// ADB model identifier.
    pub model: Option<String>,
    /// ADB device identifier.
    pub device: Option<String>,
}

/// File or directory metadata on the phone.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RemoteFile {
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
}

/// One decompressed clipboard entry.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ClipboardEntry {
    /// Clipboard text.
    pub text: String,
    /// Phone-provided timestamp in milliseconds.
    pub timestamp_ms: i64,
}

/// Direction of a file transfer progress event.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferDirection {
    /// Phone to local computer.
    Download,
    /// Local computer to phone.
    Upload,
}

/// Progress for one file transfer.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TransferProgress {
    /// Transfer direction.
    pub direction: TransferDirection,
    /// Bytes transferred so far.
    pub transferred: u64,
    /// Total bytes expected.
    pub total: u64,
}

/// A phone-side file I/O error exposed by the client.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RemoteIoError {
    /// SSP file error code, when present.
    pub code: Option<i32>,
    /// Localized or phone-provided error message.
    pub message: String,
}

/// Options for a delete request.
#[derive(Debug, Clone, Default)]
pub struct DeleteOptions {
    /// Ask the phone to move the entry to trash.
    pub trash: bool,
    /// Mark the request as part of synchronization.
    pub sync: bool,
}

/// Callback invoked after transfer progress changes.
pub type TransferProgressCallback = Arc<dyn Fn(TransferProgress) + Send + Sync>;

/// Options for a single-file upload or download.
#[derive(Clone, Default)]
pub struct TransferOptions {
    /// Permit replacing an existing local or remote target.
    pub overwrite: bool,
    /// Optional callback for progress events.
    pub progress: Option<TransferProgressCallback>,
}

impl std::fmt::Debug for TransferOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TransferOptions")
            .field("overwrite", &self.overwrite)
            .field("has_progress_callback", &self.progress.is_some())
            .finish()
    }
}
