use std::sync::Arc;

use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DeviceInfo {
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
    pub external_storage_path: Option<String>,
    pub disk_size: Option<u64>,
    pub used_disk_size: Option<u64>,
    pub battery_percentage: Option<u32>,
    pub phone_locked: Option<bool>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AdbDevice {
    pub serial: String,
    pub state: String,
    pub product: Option<String>,
    pub model: Option<String>,
    pub device: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RemoteFile {
    pub path: String,
    pub size: u64,
    pub created_at: Option<u64>,
    pub modified_at: Option<u64>,
    pub is_directory: bool,
    pub checksum: Option<String>,
    pub is_trash: Option<bool>,
    pub id: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ClipboardEntry {
    pub text: String,
    pub timestamp_ms: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferDirection {
    Download,
    Upload,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TransferProgress {
    pub direction: TransferDirection,
    pub transferred: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RemoteIoError {
    pub code: Option<i32>,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct DeleteOptions {
    pub trash: bool,
    pub sync: bool,
}

pub type TransferProgressCallback = Arc<dyn Fn(TransferProgress) + Send + Sync>;

#[derive(Clone, Default)]
pub struct TransferOptions {
    pub overwrite: bool,
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
