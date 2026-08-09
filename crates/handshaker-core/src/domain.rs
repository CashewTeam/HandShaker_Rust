use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

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

/// One device discovered over mDNS (Bonjour), advertising `_handshaker_ssp._tcp`.
///
/// The WiFi server port is dynamic and changes over time, so it must always be
/// read from a fresh mDNS response (SRV record), never cached.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WifiDevice {
    /// Service instance name, e.g. `handshaker_ssp_`.
    pub instance: String,
    /// Host name from the SRV record, e.g. `fixture-phone.local`.
    pub host: String,
    /// Resolved addresses (IPv4 first when present, then IPv6).
    pub addresses: Vec<String>,
    /// TCP port from the SRV record.
    pub port: u16,
    /// TXT record key/value pairs (empty for the HandShaker service in
    /// captured traffic; other services may carry properties).
    pub txt: BTreeMap<String, String>,
}

/// One locally persisted WiFi trust record, without the derived key.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TrustRecordInfo {
    /// Phone-side device identifier (android_id), the trust key.
    pub device_uuid: String,
    /// Device name stored at trust time.
    pub device_name: Option<String>,
    /// Unix timestamp of the last successful trust.
    pub updated_at: u64,
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
    /// Phone-side extension data (JSON with star/orientation/updateTime),
    /// used by the photo-sync channel to distinguish content changes from
    /// metadata-only changes. Opaque to the host.
    pub ext_data: Option<String>,
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
    /// Download starting offset in bytes (downloads only). The phone seeks to
    /// this position once and streams from there — there is no resumption
    /// state, matching the captured wire behavior (`range` is a one-shot
    /// seek). Defaults to 0 (from the start of the file).
    pub offset: u64,
}

impl std::fmt::Debug for TransferOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TransferOptions")
            .field("overwrite", &self.overwrite)
            .field("offset", &self.offset)
            .field("has_progress_callback", &self.progress.is_some())
            .finish()
    }
}

/// One unit of a batch transfer: `source` is the local path for uploads and
/// the remote path for downloads; `target` is the counterpart on the other
/// side. Directories are expanded by the caller (`upload_tree`/`download_tree`)
/// into a flat list of file items before the batch runs.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BatchTransferItem {
    /// Source path (local for upload, remote for download).
    pub source: String,
    /// Target path (remote for upload, local for download).
    pub target: String,
}

/// Progress across the files of a batch transfer (serial execution).
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct BatchTransferProgress {
    /// Files finished so far (successful or failed).
    pub done: usize,
    /// Total files in the batch.
    pub total: usize,
}

/// Callback invoked after each file of a batch transfer completes.
pub type BatchTransferProgressCallback = Arc<dyn Fn(BatchTransferProgress) + Send + Sync>;

/// Options for `HandShakerClient::upload_many`/`download_many`.
#[derive(Clone)]
pub struct BatchTransferOptions {
    /// Permit replacing existing local or remote targets.
    pub overwrite: bool,
    /// Optional callback invoked after each file completes.
    pub progress: Option<BatchTransferProgressCallback>,
    /// Download starting offset in bytes (downloads only; one-shot seek, no
    /// resumption state). Defaults to 0.
    pub offset: u64,
    /// Maximum files transferred in parallel (1..=8). Defaults to 1 (serial),
    /// which keeps ordering deterministic and matches pre-0.4.1 behavior.
    pub concurrency: usize,
    /// Optional cancellation token: when cancelled, files that have not
    /// started yet are skipped and the batch returns `Error::Interrupted`
    /// (Phase D review fix: plan transfers must stop on `cancel_transfer`).
    pub cancel: Option<crate::cancellation::CancellationToken>,
}

impl Default for BatchTransferOptions {
    fn default() -> Self {
        Self {
            overwrite: false,
            progress: None,
            offset: 0,
            concurrency: 1,
            cancel: None,
        }
    }
}

impl std::fmt::Debug for BatchTransferOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BatchTransferOptions")
            .field("overwrite", &self.overwrite)
            .field("offset", &self.offset)
            .field("concurrency", &self.concurrency)
            .field("has_progress_callback", &self.progress.is_some())
            .field(
                "cancellation_requested",
                &self.cancel.as_ref().is_some_and(|t| t.is_cancelled()),
            )
            .finish()
    }
}

/// A failed file within a batch transfer. The batch continues past failures;
/// the message carries the per-file error text.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BatchTransferFailure {
    /// The item's source path.
    pub source: String,
    /// The item's target path.
    pub target: String,
    /// Human-readable error message for this file.
    pub message: String,
    /// Structured error class of this failure (Phase D review fix): lets
    /// callers distinguish transport death from ordinary per-file failures
    /// without parsing message text. `None` in pre-Phase-D payloads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<crate::error::ErrorCode>,
}

/// Aggregated result of a batch transfer: successes and per-file failures.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
pub struct BatchTransferResult {
    /// Items that transferred successfully.
    pub ok: Vec<BatchTransferItem>,
    /// Items that failed, with per-file error messages.
    pub failures: Vec<BatchTransferFailure>,
}

/// An image entry from the phone photo library.
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct ImageFile {
    /// Absolute remote path.
    pub path: Option<String>,
    /// File size in bytes.
    pub size: Option<u64>,
    /// Creation timestamp.
    pub created_at: Option<u64>,
    /// Modification timestamp.
    pub modified_at: Option<u64>,
    /// Width in pixels.
    pub width: Option<u32>,
    /// Height in pixels.
    pub height: Option<u32>,
    /// Exif orientation value.
    pub orientation: Option<u32>,
    /// Media-store identifier.
    pub media_id: Option<u64>,
    /// Album identifier.
    pub album_id: Option<u64>,
    /// MIME type.
    pub mime_type: Option<String>,
    /// Embedded thumbnail bytes (JPEG), when fetched.
    pub thumbnail: Option<Vec<u8>>,
    /// Album display name.
    pub album_name: Option<String>,
    /// Time the photo was taken.
    pub date_taken: Option<u64>,
    /// Latitude, as reported by the phone.
    pub latitude: Option<String>,
    /// Longitude, as reported by the phone.
    pub longitude: Option<String>,
    /// Miniature-thumbnail cache magic.
    pub mini_thumb_magic: Option<String>,
    /// Display title.
    pub title: Option<String>,
    /// Whether the phone failed to produce a thumbnail for this entry.
    pub thumbnail_error: bool,
    /// Whether the image is starred (favorite).
    pub starred: bool,
}

/// A photo album.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ImageAlbum {
    /// Album directory path.
    pub path: Option<String>,
    /// Album (bucket) identifier.
    pub album_id: Option<u64>,
    /// Album display name.
    pub name: Option<String>,
    /// Cover image, when supplied by the phone.
    pub cover_image: Option<Box<ImageFile>>,
}

/// A video entry from the phone video library.
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct VideoFile {
    /// Absolute remote path.
    pub path: Option<String>,
    /// File size in bytes.
    pub size: Option<u64>,
    /// Creation timestamp.
    pub created_at: Option<u64>,
    /// Modification timestamp.
    pub modified_at: Option<u64>,
    /// Width in pixels.
    pub width: Option<u32>,
    /// Height in pixels.
    pub height: Option<u32>,
    /// Exif orientation value.
    pub orientation: Option<u32>,
    /// Media-store identifier.
    pub media_id: Option<u64>,
    /// Album identifier.
    pub album_id: Option<u64>,
    /// MIME type.
    pub mime_type: Option<String>,
    /// Embedded thumbnail bytes (JPEG), when fetched.
    pub thumbnail: Option<Vec<u8>>,
    /// Whether the phone failed to produce a thumbnail.
    pub thumbnail_error: bool,
    /// Duration in seconds.
    pub duration: Option<f64>,
}

/// A video album.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct VideoAlbum {
    /// Album directory path.
    pub path: Option<String>,
    /// Album (bucket) identifier.
    pub album_id: Option<u64>,
    /// Album display name.
    pub name: Option<String>,
}

/// An audio track from the phone music library.
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct AudioFile {
    /// Absolute file path.
    pub path: Option<String>,
    /// File size in bytes.
    pub size: Option<u64>,
    /// Creation timestamp.
    pub created_at: Option<u64>,
    /// Modification timestamp.
    pub modified_at: Option<u64>,
    /// Media-store identifier.
    pub media_id: Option<u64>,
    /// Album identifier.
    pub album_id: Option<u64>,
    /// Track title.
    pub title: Option<String>,
    /// MIME type.
    pub mime_type: Option<String>,
    /// Artist identifier.
    pub artist_id: Option<u64>,
    /// Artist name.
    pub artist: Option<String>,
    /// Composer name.
    pub composer: Option<String>,
    /// ID3v1 genre index.
    pub genre: Option<u32>,
    /// Free-form comment.
    pub comment: Option<String>,
    /// Copyright string.
    pub copyright: Option<String>,
    /// Audio codec.
    pub audio_codec: Option<String>,
    /// Track number.
    pub track: Option<u32>,
    /// Duration in seconds.
    pub duration: Option<f64>,
}

/// An audio album.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AudioAlbum {
    /// Album directory path.
    pub path: Option<String>,
    /// Album identifier.
    pub album_id: Option<u64>,
    /// Album display name.
    pub name: Option<String>,
    /// Artist identifier.
    pub artist_id: Option<u64>,
    /// Artist name.
    pub artist: Option<String>,
    /// Release year.
    pub year: Option<u32>,
    /// Album-art thumbnail bytes, when fetched.
    pub thumbnail: Option<Vec<u8>>,
    /// Whether the phone failed to produce a thumbnail.
    pub thumbnail_error: bool,
}

/// A snapshot of the phone photo library.
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct PhotoLibrary {
    /// All images in the library.
    pub images: Vec<ImageFile>,
    /// All photo albums.
    pub albums: Vec<ImageAlbum>,
    /// Media-store id of the camera album, when reported.
    pub camera_album_id: Option<u64>,
}

/// A snapshot of the phone video library.
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct VideoLibrary {
    /// All videos in the library.
    pub videos: Vec<VideoFile>,
    /// All video albums.
    pub albums: Vec<VideoAlbum>,
}

/// A snapshot of the phone audio library.
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct AudioLibrary {
    /// All audio tracks in the library.
    pub tracks: Vec<AudioFile>,
    /// All audio albums.
    pub albums: Vec<AudioAlbum>,
}

/// Thumbnail responses keyed by media category.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Thumbnails {
    /// Image thumbnails, in request order.
    pub images: Vec<ImageFile>,
    /// Video thumbnails, in request order.
    pub videos: Vec<VideoFile>,
    /// Audio album-art thumbnails, in request order.
    pub audio_albums: Vec<AudioAlbum>,
}

/// Exif metadata for a media file.
///
/// Fetched on demand with `HandShakerClient::fetch_exif`: the file is pulled
/// over the SSP download channel (WiFi or ADB) and parsed locally with
/// `kamadak-exif`, so the full EXIF payload is available for any media path
/// without extending the SSP schema.
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct ExifData {
    /// Exif orientation value.
    pub orientation: Option<u32>,
    /// Time the photo was taken (Unix seconds).
    pub date_taken: Option<u64>,
    /// Latitude string.
    pub latitude: Option<String>,
    /// Longitude string.
    pub longitude: Option<String>,
    /// Camera make, e.g. "Smartisan".
    pub make: Option<String>,
    /// Camera model, e.g. "U2 Pro".
    pub model: Option<String>,
    /// Software that wrote the file, e.g. "Camera".
    pub software: Option<String>,
    /// Lens model.
    pub lens_model: Option<String>,
    /// Focal length in millimetres.
    pub focal_length: Option<f64>,
    /// Exposure time in seconds, e.g. 1/125 -> 0.008.
    pub exposure_time: Option<f64>,
    /// F-number, e.g. 1.8.
    pub f_number: Option<f64>,
    /// ISO speed rating.
    pub iso: Option<u32>,
}

/// Photo-sync profile: which phone folder to sync and where to store it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncConfig {
    /// Stable phone identifier used to key the ledger file.
    pub device_uuid: String,
    /// Phone-side root folder to sync (e.g. /storage/emulated/0/DCIM/Camera).
    pub phone_root: String,
    /// Local destination directory for downloaded photos.
    pub local_root: String,
    /// Stable host-side pc identifier sent with PHOTO_SYNC_REQUEST(37).
    pub pc_id: String,
}

/// One recorded file in the sync ledger.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncFileRecord {
    /// Phone-side file size.
    pub size: u64,
    /// Phone-reported checksum (MD5 over name+size+head), when available.
    pub checksum: Option<String>,
    /// Phone-side extension data (star/orientation/updateTime JSON), opaque.
    pub ext_data: Option<String>,
    /// Phone-side modification timestamp.
    pub modified_at: Option<u64>,
    /// Local file path the photo was downloaded to.
    pub local_path: String,
    /// SHA-256 of the downloaded local file at ledger commit time.
    pub local_sha256: Option<String>,
}

/// Ledger snapshot keyed by absolute phone path.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncSnapshot {
    /// Phone path -> recorded file.
    pub files: BTreeMap<String, SyncFileRecord>,
}

/// Plan diff between the phone's current photo state and the local ledger.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct SyncDiff {
    /// Files to download (new or content-modified).
    pub added: Vec<String>,
    /// Files whose phone metadata changed (ext_data/modified_at) only.
    pub info_modified: Vec<String>,
    /// Files deleted on the phone (to remove locally, unless conflicted).
    pub deleted: Vec<String>,
    /// Local files kept because they differ from the ledger (user-modified).
    pub conflicts: Vec<String>,
}

/// Phone's answer to a PHOTO_SYNC_REQUEST(37).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PhotoSyncResult {
    /// Whether this pc_id is new to the phone (first sync).
    pub is_first: Option<bool>,
    /// Current phone-side file state list.
    pub files: Vec<RemoteFile>,
    /// Whether the phone accepted the request.
    pub is_success: Option<bool>,
}
