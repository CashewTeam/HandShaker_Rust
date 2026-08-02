use std::collections::BTreeMap;
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

/// One device discovered over mDNS (Bonjour), advertising `_handshaker_ssp._tcp`.
///
/// The WiFi server port is dynamic and changes over time, so it must always be
/// read from a fresh mDNS response (SRV record), never cached.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WifiDevice {
    /// Service instance name, e.g. `handshaker_ssp_`.
    pub instance: String,
    /// Host name from the SRV record, e.g. `Android-2.local`.
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
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PhotoLibrary {
    /// All images in the library.
    pub images: Vec<ImageFile>,
    /// All photo albums.
    pub albums: Vec<ImageAlbum>,
    /// Media-store id of the camera album, when reported.
    pub camera_album_id: Option<u64>,
}

/// A snapshot of the phone video library.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct VideoLibrary {
    /// All videos in the library.
    pub videos: Vec<VideoFile>,
    /// All video albums.
    pub albums: Vec<VideoAlbum>,
}

/// A snapshot of the phone audio library.
#[derive(Debug, Clone, Serialize, PartialEq)]
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
/// Reserved for the EXIF fetch milestone (M5, planned to use the ADB shell
/// channel like the original macOS client); `HandShakerClient::fetch_exif`
/// currently returns a not-implemented error.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ExifData {
    /// Exif orientation value.
    pub orientation: Option<u32>,
    /// Time the photo was taken.
    pub date_taken: Option<u64>,
    /// Latitude string.
    pub latitude: Option<String>,
    /// Longitude string.
    pub longitude: Option<String>,
}
