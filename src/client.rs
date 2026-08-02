use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use md5::{Digest as _, Md5};
use prost::Message;
use serde::Serialize;
use tokio::fs::{self, File};
use tokio::io::AsyncReadExt;
use uuid::Uuid;

use crate::domain::{
    AdbDevice, ClipboardEntry, DeleteOptions, DeviceInfo, RemoteFile, TransferDirection,
    TransferOptions, TransferProgress,
};
use crate::error::{Error, Result};
use crate::i18n;
use crate::protocol::frame::{MAX_UPSTREAM_PAYLOAD, WireLog};
use crate::protocol::handshake::AdbRawKeyExchange;
use crate::protocol::proto::*;
use crate::session::Session;
use crate::state::StateStore;
use crate::transport::TransportConnector;
use crate::transport::adb::{AdbConnector, AdbForward, list_devices, list_devices_with_timeout};

// These fields describe the compatible original macOS host protocol identity,
// not this crate's release version. Older values cause the phone to close the
// connection immediately after GET_DEVICE_INFO.
const COMPATIBLE_HOST_APP_VERSION: &str = "2.5.6";
const COMPATIBLE_HOST_APP_VERSION_CODE: u32 = 408;

/// A supported connection target.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ConnectionTarget {
    /// Connect through the verified ADB service and forward.
    Adb { serial: Option<String> },
}

/// Options controlling a HandShaker connection.
#[derive(Debug, Clone)]
pub struct ClientOptions {
    /// Timeout applied to ADB commands, handshake, requests, and transfers.
    pub timeout: Duration,
    /// Interval between keep-alive heartbeat requests.
    pub heartbeat_interval: Duration,
    /// Optional path for an explicit, sensitive wire log.
    pub wire_log: Option<PathBuf>,
    /// Path to the adb executable.
    pub adb_path: PathBuf,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            heartbeat_interval: Duration::from_secs(10),
            wire_log: None,
            adb_path: PathBuf::from("adb"),
        }
    }
}

/// Result of an SSP heartbeat round trip.
#[derive(Debug, Clone, Serialize)]
pub struct PingResult {
    /// Elapsed host-side round-trip time in milliseconds.
    pub round_trip_ms: u128,
    /// Timestamp echoed from the host request, when present.
    pub host_timestamp: Option<u64>,
    /// Timestamp supplied by the phone, when present.
    pub client_timestamp: Option<u64>,
}

/// A connected HandShaker device client.
///
/// # Examples
///
/// ```no_run
/// use handshaker_rust::{ClientOptions, ConnectionTarget, HandShakerClient};
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let client = HandShakerClient::connect(
///     ConnectionTarget::Adb { serial: None },
///     ClientOptions::default(),
/// )
/// .await?;
/// println!("{}", client.device_info().serial);
/// client.close().await?;
/// # Ok(())
/// # }
/// ```
pub struct HandShakerClient {
    session: Option<Session>,
    cleanup: Option<AdbForward>,
    device: DeviceInfo,
}

impl HandShakerClient {
    /// List devices reported by adb devices -l without starting HandShaker.
    pub async fn list_adb_devices(adb_path: impl AsRef<Path>) -> Result<Vec<AdbDevice>> {
        list_devices(adb_path.as_ref()).await
    }

    /// List devices with an explicit command timeout.
    pub async fn list_adb_devices_with_timeout(
        adb_path: impl AsRef<Path>,
        timeout: Duration,
    ) -> Result<Vec<AdbDevice>> {
        list_devices_with_timeout(adb_path.as_ref(), timeout).await
    }

    /// Connect, complete the ADB raw-key handshake, and fetch device info.
    pub async fn connect(target: ConnectionTarget, options: ClientOptions) -> Result<Self> {
        Self::connect_with_state(target, options, StateStore::discover()?).await
    }

    async fn connect_with_state(
        target: ConnectionTarget,
        options: ClientOptions,
        state: StateStore,
    ) -> Result<Self> {
        let _state = state.load_or_create()?;
        let wire_log = options
            .wire_log
            .as_deref()
            .map(WireLog::open)
            .transpose()?
            .map(Arc::new);
        let (serial, connected) = match target {
            ConnectionTarget::Adb { serial } => {
                let connector = AdbConnector::new(options.adb_path, serial, options.timeout);
                let connected = connector.connect().await?;
                (connected.device.serial.clone(), connected)
            }
        };
        let session = Session::establish(
            connected.stream,
            options.timeout,
            options.heartbeat_interval,
            wire_log,
            &AdbRawKeyExchange,
        )
        .await?;
        let mut client = Self {
            session: Some(session),
            cleanup: Some(connected.cleanup),
            device: DeviceInfo {
                serial,
                phone_id: None,
                name: None,
                model: None,
                brand: None,
                manufacturer: None,
                smartisan_version: None,
                apk_version: None,
                apk_version_name: None,
                root_path: "/sdcard".to_string(),
                external_storage_path: None,
                disk_size: None,
                used_disk_size: None,
                battery_percentage: None,
                phone_locked: None,
            },
        };
        client.device = client.fetch_device_info().await?;
        Ok(client)
    }

    #[cfg(test)]
    pub(crate) async fn connect_for_test(
        target: ConnectionTarget,
        options: ClientOptions,
        state_path: PathBuf,
    ) -> Result<Self> {
        Self::connect_with_state(target, options, StateStore::at(state_path)).await
    }

    /// Return the device information fetched during connection.
    pub fn device_info(&self) -> &DeviceInfo {
        &self.device
    }

    /// Return the phone root path used to resolve relative one-shot paths.
    pub fn root_path(&self) -> &str {
        &self.device.root_path
    }

    /// Send one heartbeat and return its timing and timestamps.
    pub async fn ping(&self) -> Result<PingResult> {
        let request = SspHeartBeatRequest {
            r#type: Some(SspRequestType::HeartBeatRequest as i32),
            host_timestamp: Some(unix_seconds()),
        };
        let start = Instant::now();
        let response =
            SspHeartBeatResponse::decode(self.session()?.request(&request).await?.as_slice())?;
        Ok(PingResult {
            round_trip_ms: start.elapsed().as_millis(),
            host_timestamp: response.host_timestamp,
            client_timestamp: response.client_timestamp,
        })
    }

    /// List files below a remote directory up to depth.
    pub async fn list_dir(&self, path: &str, depth: u32) -> Result<Vec<RemoteFile>> {
        let request = SspGetDirFilesRequest {
            r#type: Some(SspRequestType::GetDirFilesRequest as i32),
            dir: Some(ssp_file(path, true, None)),
            maxdepth: Some(depth.max(1)),
        };
        let response =
            SspGetDirFilesResponse::decode(self.session()?.request(&request).await?.as_slice())?;
        Ok(response.file.into_iter().map(remote_file).collect())
    }

    /// Count files below a remote directory with optional exclusion patterns.
    pub async fn file_count(&self, path: &str, depth: u32, exclusions: Vec<String>) -> Result<u64> {
        let request = SspGetFileCountRequest {
            r#type: Some(SspRequestType::GetFileCountRequest as i32),
            dir: Some(ssp_file(path, true, None)),
            maxdepth: Some(depth.max(1)),
            exclusion_pattern: exclusions,
        };
        let response =
            SspGetFileCountResponse::decode(self.session()?.request(&request).await?.as_slice())?;
        Ok(response.count.unwrap_or(0))
    }

    /// Check whether a remote file path exists.
    pub async fn file_exists(&self, path: &str) -> Result<bool> {
        let request = SspFileExistRequest {
            r#type: Some(SspRequestType::GetFileExistRequest as i32),
            file: Some(ssp_file(path, false, None)),
        };
        let response =
            SspFileExistResponse::decode(self.session()?.request(&request).await?.as_slice())?;
        Ok(response.exist.unwrap_or(false))
    }

    /// Read metadata for a remote path.
    pub async fn stat(&self, path: &str) -> Result<Option<RemoteFile>> {
        if path == self.root_path() {
            return Ok(Some(RemoteFile {
                path: path.to_string(),
                size: self.device.disk_size.unwrap_or(0),
                created_at: None,
                modified_at: None,
                is_directory: true,
                checksum: None,
                is_trash: None,
                id: None,
            }));
        }
        let (parent, _) = split_remote_path(path)?;
        Ok(self
            .list_dir(parent, 1)
            .await?
            .into_iter()
            .find(|file| file.path == path))
    }

    /// Create a remote directory and return its metadata.
    pub async fn create_dir(&self, path: &str) -> Result<RemoteFile> {
        let request = SspCreateFolderRequest {
            r#type: Some(SspRequestType::GetCreateFolderRequest as i32),
            file: Some(ssp_file(path, true, None)),
        };
        let response =
            SspCreateFolderResponse::decode(self.session()?.request(&request).await?.as_slice())?;
        ensure_remote_success(
            response.succeed,
            response.error_code,
            response.error_message,
        )?;
        response
            .file
            .map(remote_file)
            .ok_or_else(|| Error::Protocol(i18n::text("client.mkdir_missing_file").to_string()))
    }

    /// Rename a remote file or directory.
    pub async fn rename(&self, source: &str, target: &str) -> Result<()> {
        let request = SspRenameFileRequest {
            r#type: Some(SspRequestType::GetRenameFileRequest as i32),
            source_file: Some(ssp_file(source, false, None)),
            target_file: Some(ssp_file(target, false, None)),
        };
        let response =
            SspRenameFileResponse::decode(self.session()?.request(&request).await?.as_slice())?;
        ensure_remote_success(
            response.succeed,
            response.error_code,
            response.error_message,
        )
    }

    /// Delete remote paths with the requested trash and sync options.
    pub async fn delete(
        &self,
        paths: &[String],
        options: DeleteOptions,
    ) -> Result<Vec<RemoteFile>> {
        let request = SspDeleteFileRequest {
            r#type: Some(SspRequestType::GetDeleteFileRequest as i32),
            file: paths
                .iter()
                .map(|path| ssp_file(path, false, None))
                .collect(),
            is_sync: Some(options.sync),
            is_trash: Some(options.trash),
        };
        let response =
            SspDeleteFileResponse::decode(self.session()?.request(&request).await?.as_slice())?;
        ensure_remote_success(
            response.succeed,
            response.error_code,
            response.error_message,
        )?;
        ensure_deleted_files_succeeded(&response.file)?;
        Ok(response.file.into_iter().map(remote_file).collect())
    }

    /// Download one remote file through a temporary local file.
    pub async fn download(
        &self,
        remote: &str,
        local: &Path,
        options: TransferOptions,
    ) -> Result<u64> {
        if local.exists() && !options.overwrite {
            return Err(Error::LocalIo(i18n::format(
                "client.local_target_exists",
                &[&local.display().to_string()],
            )));
        }
        let request = SspDownloadFileRequest {
            r#type: Some(SspRequestType::GetDownloadFileRequest as i32),
            file: Some(ssp_file(remote, false, None)),
            range: Some(SspDataRange {
                offset: Some(0),
                length: Some(0),
            }),
            need_md5: Some(true),
            gzip: Some(false),
            is_sync: Some(false),
        };
        let mut open = self.session()?.open(&request).await?;
        let header =
            SspDownloadFileResponseHeader::decode(open.receive_normal().await?.as_slice())?;
        if header.ready != Some(true) {
            open.finish().await;
            return Err(remote_error(
                header.error_code,
                i18n::text("client.download_not_ready"),
            ));
        }
        let total = header.range.and_then(|range| range.length).ok_or_else(|| {
            Error::Protocol(i18n::text("client.download_missing_length").to_string())
        })?;
        if let Some(callback) = &options.progress {
            callback(TransferProgress {
                direction: TransferDirection::Download,
                transferred: 0,
                total,
            });
        }
        let temporary = temporary_download_path(local)?;
        let mut temporary_guard = TemporaryDownload::new(temporary.clone());
        let mut file = File::create(&temporary).await.map_err(|error| {
            Error::LocalIo(i18n::format(
                "client.create_failed",
                &[&temporary.display().to_string(), &error.to_string()],
            ))
        })?;
        let progress = options.progress.clone();
        let receive = open
            .receive_raw_to(total, &mut file, |transferred| {
                if let Some(callback) = &progress {
                    callback(TransferProgress {
                        direction: TransferDirection::Download,
                        transferred,
                        total,
                    });
                }
            })
            .await;
        open.finish().await;
        let actual_md5 = match receive {
            Ok(md5) => md5,
            Err(error) => {
                return Err(error);
            }
        };
        file.sync_all().await.map_err(|error| {
            Error::LocalIo(i18n::format(
                "client.sync_failed",
                &[&temporary.display().to_string(), &error.to_string()],
            ))
        })?;
        drop(file);
        if let Some(expected) = header.data_md5.filter(|value| !value.is_empty())
            && !expected.eq_ignore_ascii_case(&actual_md5)
        {
            return Err(Error::Protocol(i18n::format(
                "client.download_md5_mismatch",
                &[&expected, &actual_md5],
            )));
        }
        fs::rename(&temporary, local).await.map_err(|error| {
            Error::LocalIo(i18n::format(
                "client.move_failed",
                &[
                    &temporary.display().to_string(),
                    &local.display().to_string(),
                    &error.to_string(),
                ],
            ))
        })?;
        temporary_guard.commit();
        Ok(total)
    }

    /// Upload one local regular file to a remote path.
    pub async fn upload(
        &self,
        local: &Path,
        remote: &str,
        options: TransferOptions,
    ) -> Result<u64> {
        let metadata = fs::metadata(local).await.map_err(|error| {
            Error::LocalIo(i18n::format(
                "client.read_failed",
                &[&local.display().to_string(), &error.to_string()],
            ))
        })?;
        if !metadata.is_file() {
            return Err(Error::LocalIo(i18n::format(
                "client.not_regular_file",
                &[&local.display().to_string()],
            )));
        }
        if !options.overwrite && self.file_exists(remote).await? {
            return Err(Error::RemoteIo {
                code: Some(SspFileIoError::FileIoTargetAlreadyExist as i32),
                message: i18n::format("client.remote_target_exists", &[remote]),
            });
        }
        let (data_md5, size) = file_md5(local).await?;
        let request = SspUploadFileRequest {
            r#type: Some(SspRequestType::GetUploadFileRequestHeader as i32),
            file: Some(ssp_file(remote, false, Some(size))),
            data_md5: Some(data_md5),
            gzip: Some(false),
            is_sync: Some(false),
        };
        let mut open = self.session()?.open(&request).await?;
        let header = SspUploadFileResponseHeader::decode(open.receive_normal().await?.as_slice())?;
        if header.ready != Some(true) {
            open.finish().await;
            return Err(remote_error(
                header.error_code,
                i18n::text("client.upload_rejected"),
            ));
        }
        let mut file = File::open(local).await.map_err(|error| {
            Error::LocalIo(i18n::format(
                "client.open_failed",
                &[&local.display().to_string(), &error.to_string()],
            ))
        })?;
        let mut buffer = vec![0_u8; MAX_UPSTREAM_PAYLOAD.min(1024 * 1024)];
        let mut transferred = 0_u64;
        if let Some(callback) = &options.progress {
            callback(TransferProgress {
                direction: TransferDirection::Upload,
                transferred,
                total: size,
            });
        }
        loop {
            let read = file.read(&mut buffer).await.map_err(|error| {
                Error::LocalIo(i18n::format(
                    "client.read_failed",
                    &[&local.display().to_string(), &error.to_string()],
                ))
            })?;
            if read == 0 {
                break;
            }
            open.send(3, &buffer[..read]).await?;
            transferred += read as u64;
            if let Some(callback) = &options.progress {
                callback(TransferProgress {
                    direction: TransferDirection::Upload,
                    transferred,
                    total: size,
                });
            }
        }
        let complete = SspUploadFileResponse::decode(open.receive_normal().await?.as_slice())?;
        open.finish().await;
        ensure_remote_success(complete.succeed, complete.error_code, None)?;
        if complete.canceled == Some(true) {
            return Err(Error::RemoteIo {
                code: complete.error_code,
                message: i18n::text("client.upload_canceled").to_string(),
            });
        }
        Ok(size)
    }

    /// Read and decompress the phone clipboard history.
    pub async fn clipboard_list(&self) -> Result<Vec<ClipboardEntry>> {
        let entries = self.fetch_clipboards().await?;
        entries.into_iter().map(decode_clipboard).collect()
    }

    /// Add a text entry to the phone clipboard.
    pub async fn clipboard_set(&self, text: &str) -> Result<()> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(text.as_bytes()).map_err(|error| {
            Error::LocalIo(i18n::format(
                "client.compress_clipboard_failed",
                &[&error.to_string()],
            ))
        })?;
        let content = encoder.finish().map_err(|error| {
            Error::LocalIo(i18n::format(
                "client.compress_clipboard_failed",
                &[&error.to_string()],
            ))
        })?;
        let request = SspPostClipboardRequest {
            r#type: Some(SspRequestType::PostClipboardRequest as i32),
            clipboard: SspClipboard {
                content: Some(content),
                mstimestamp: Some(unix_millis()),
            },
        };
        let response =
            SspPostClipboardResponse::decode(self.session()?.request(&request).await?.as_slice())?;
        ensure_remote_success(response.succeed, None, None)
    }

    /// Delete one clipboard entry by its millisecond timestamp.
    pub async fn clipboard_delete(&self, timestamp_ms: i64) -> Result<()> {
        let clipboard = self
            .fetch_clipboards()
            .await?
            .into_iter()
            .find(|item| item.mstimestamp == Some(timestamp_ms))
            .ok_or_else(|| Error::RemoteIo {
                code: None,
                message: i18n::format(
                    "client.clipboard_entry_missing",
                    &[&timestamp_ms.to_string()],
                ),
            })?;
        let request = SspDeleteClipboardRequest {
            r#type: Some(SspRequestType::DeleteClipboardRequest as i32),
            clipboard,
        };
        let response = SspDeleteClipboardResponse::decode(
            self.session()?.request(&request).await?.as_slice(),
        )?;
        ensure_remote_success(response.succeed, None, None)
    }

    /// Clear the phone clipboard history.
    pub async fn clipboard_clear(&self) -> Result<()> {
        let request = SspClearClipboardRequest {
            r#type: Some(SspRequestType::ClearClipboardRequest as i32),
        };
        let response =
            SspClearClipboardResponse::decode(self.session()?.request(&request).await?.as_slice())?;
        ensure_remote_success(response.succeed, None, None)
    }

    /// Send QUIT and remove the ADB forward created by this client.
    pub async fn close(mut self) -> Result<()> {
        let session_result = if let Some(session) = self.session.take() {
            session.close().await
        } else {
            Ok(())
        };
        let cleanup_result = if let Some(mut cleanup) = self.cleanup.take() {
            cleanup.cleanup().await
        } else {
            Ok(())
        };
        session_result.and(cleanup_result)
    }

    async fn fetch_device_info(&self) -> Result<DeviceInfo> {
        let request = SspGetDeviceInfoRequest {
            r#type: Some(SspRequestType::GetDeviceInfoRequest as i32),
            host_timestamp: Some(unix_seconds()),
            host_smart_sync_protocol_version: Some("1".to_string()),
            need_device_info_callback: Some(false),
            need_photo_library_callback: Some(false),
            need_audio_library_callback: Some(false),
            need_video_library_callback: Some(false),
            host_app_version: Some(COMPATIBLE_HOST_APP_VERSION.to_string()),
            host_min_client_version: Some("1.0.0".to_string()),
            host_type: Some(1),
            host_app_version_code: Some(COMPATIBLE_HOST_APP_VERSION_CODE),
        };
        let response =
            SspGetDeviceInfoResponse::decode(self.session()?.request(&request).await?.as_slice())?;
        Ok(DeviceInfo {
            serial: self.device.serial.clone(),
            phone_id: response.phone_id,
            name: response.phone_name,
            model: response.phone_model,
            brand: response.product_brand,
            manufacturer: response.product_manufacturer,
            smartisan_version: response.smartisan_version,
            apk_version: response.apk_version,
            apk_version_name: response.apk_version_name,
            root_path: response.root_path.unwrap_or_else(|| "/sdcard".to_string()),
            external_storage_path: response.external_storage_path,
            disk_size: response.disk_size,
            used_disk_size: response.used_disk_size,
            battery_percentage: response.battery_percentage,
            phone_locked: response.phone_locked,
        })
    }

    async fn fetch_clipboards(&self) -> Result<Vec<SspClipboard>> {
        let request = SspGetClipboardRequest {
            r#type: Some(SspRequestType::GetClipboardRequest as i32),
        };
        let response =
            SspGetClipboardResponse::decode(self.session()?.request(&request).await?.as_slice())?;
        Ok(response.clipboard)
    }

    fn session(&self) -> Result<&Session> {
        self.session
            .as_ref()
            .ok_or_else(|| Error::Transport(i18n::text("client.connection_closed").to_string()))
    }
}

fn remote_file(file: SspFile) -> RemoteFile {
    RemoteFile {
        path: file.path.unwrap_or_default(),
        size: file.file_size.unwrap_or(0),
        created_at: file.created_timestamp,
        modified_at: file.modified_timestamp,
        is_directory: file.is_directory.unwrap_or(false),
        checksum: file.checksum,
        is_trash: file.is_trash,
        id: file.id,
    }
}

fn ssp_file(path: &str, is_directory: bool, size: Option<u64>) -> SspFile {
    SspFile {
        path: Some(path.to_string()),
        file_size: size,
        is_directory: Some(is_directory),
        ..Default::default()
    }
}

fn ensure_remote_success(
    succeed: Option<bool>,
    code: Option<i32>,
    message: Option<String>,
) -> Result<()> {
    if succeed == Some(true) {
        return Ok(());
    }
    Err(remote_error(
        code,
        &message.unwrap_or_else(|| i18n::text("client.remote_failed").to_string()),
    ))
}

fn ensure_deleted_files_succeeded(files: &[SspFile]) -> Result<()> {
    if let Some(failed) = files.iter().find(|file| file.succeed == Some(false)) {
        return Err(Error::RemoteIo {
            code: failed.error_code,
            message: i18n::format(
                "client.delete_failed",
                &[failed.path.as_deref().unwrap_or("<unknown>")],
            ),
        });
    }
    Ok(())
}

fn remote_error(code: Option<i32>, fallback: &str) -> Error {
    let message = code
        .and_then(|value| SspFileIoError::try_from(value).ok())
        .map(|value| value.as_str_name().to_string())
        .unwrap_or_else(|| fallback.to_string());
    Error::RemoteIo { code, message }
}

fn decode_clipboard(clipboard: SspClipboard) -> Result<ClipboardEntry> {
    let compressed = clipboard.content.unwrap_or_default();
    let mut decoder = GzDecoder::new(compressed.as_slice());
    let mut text = String::new();
    decoder.read_to_string(&mut text).map_err(|error| {
        Error::Protocol(i18n::format(
            "client.decompress_clipboard_failed",
            &[&error.to_string()],
        ))
    })?;
    Ok(ClipboardEntry {
        text,
        timestamp_ms: clipboard.mstimestamp.unwrap_or(0),
    })
}

fn split_remote_path(path: &str) -> Result<(&str, &str)> {
    let index = path.rfind('/').ok_or_else(|| {
        Error::Usage(i18n::format(
            "client.remote_path_directory_required",
            &[path],
        ))
    })?;
    let parent = if index == 0 { "/" } else { &path[..index] };
    let name = &path[index + 1..];
    if name.is_empty() {
        return Err(Error::Usage(i18n::format(
            "client.remote_path_name_required",
            &[path],
        )));
    }
    Ok((parent, name))
}

fn temporary_download_path(local: &Path) -> Result<PathBuf> {
    let parent = local.parent().unwrap_or_else(|| Path::new("."));
    let name = local
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            Error::LocalIo(i18n::format(
                "client.local_path_name_required",
                &[&local.display().to_string()],
            ))
        })?;
    Ok(parent.join(format!(".{name}.handshaker-part-{}", Uuid::new_v4())))
}

struct TemporaryDownload {
    path: PathBuf,
    committed: bool,
}

impl TemporaryDownload {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            committed: false,
        }
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for TemporaryDownload {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

async fn file_md5(path: &Path) -> Result<(String, u64)> {
    let mut file = File::open(path).await.map_err(|error| {
        Error::LocalIo(i18n::format(
            "client.open_failed",
            &[&path.display().to_string(), &error.to_string()],
        ))
    })?;
    let mut digest = Md5::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut total = 0_u64;
    loop {
        let read = file.read(&mut buffer).await.map_err(|error| {
            Error::LocalIo(i18n::format(
                "client.read_failed",
                &[&path.display().to_string(), &error.to_string()],
            ))
        })?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        total += read as u64;
    }
    Ok((format!("{:x}", digest.finalize()), total))
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn unix_millis() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{DownloadBehavior, FakeSsp, FakeSspConfig, UploadBehavior};

    #[test]
    fn splits_remote_paths() {
        assert_eq!(split_remote_path("/a/b").expect("path"), ("/a", "b"));
        assert_eq!(split_remote_path("/a").expect("path"), ("/", "a"));
    }

    #[test]
    fn rejects_remote_directory_without_name() {
        assert!(split_remote_path("/a/").is_err());
    }

    #[test]
    fn clipboard_round_trip_gzip() {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all("sample".as_bytes()).unwrap();
        let entry = decode_clipboard(SspClipboard {
            content: Some(encoder.finish().unwrap()),
            mstimestamp: Some(42),
        })
        .unwrap();
        assert_eq!(entry.text, "sample");
        assert_eq!(entry.timestamp_ms, 42);
    }

    #[test]
    fn partial_delete_failure_is_not_reported_as_success() {
        let files = vec![
            SspFile {
                path: Some("/ok".into()),
                succeed: Some(true),
                ..Default::default()
            },
            SspFile {
                path: Some("/failed".into()),
                succeed: Some(false),
                error_code: Some(SspFileIoError::FileIoPermissionError as i32),
                ..Default::default()
            },
        ];
        let error = ensure_deleted_files_succeeded(&files).expect_err("partial failure");
        assert!(matches!(
            error,
            Error::RemoteIo {
                code: Some(code),
                ..
            } if code == SspFileIoError::FileIoPermissionError as i32
        ));
    }

    #[tokio::test]
    async fn fake_ssp_covers_public_client_operations_and_forward_cleanup() {
        let fake = FakeSsp::start(FakeSspConfig::default()).await;
        let client = fake.connect().await;

        assert_eq!(client.device_info().model.as_deref(), Some("U2 Pro"));
        assert_eq!(client.root_path(), "/storage/emulated/0");
        assert_eq!(
            client.ping().await.expect("ping").client_timestamp,
            Some(42)
        );
        assert_eq!(client.list_dir(".", 1).await.expect("list").len(), 1);
        assert_eq!(
            client.file_count(".", 1, Vec::new()).await.expect("count"),
            1
        );
        assert!(client.file_exists("test.txt").await.expect("exists"));
        assert!(
            client
                .stat("/storage/emulated/0/test.txt")
                .await
                .expect("stat")
                .is_some()
        );
        assert!(
            client
                .create_dir("/storage/emulated/0/m0")
                .await
                .expect("mkdir")
                .is_directory
        );
        client
            .rename("/storage/emulated/0/m0", "/storage/emulated/0/m0-renamed")
            .await
            .expect("rename");
        client
            .delete(
                &["/storage/emulated/0/m0-renamed".to_string()],
                DeleteOptions::default(),
            )
            .await
            .expect("delete");

        let clipboard = client.clipboard_list().await.expect("clipboard list");
        assert_eq!(clipboard[0].text, "sample clipboard");
        client
            .clipboard_set("new clipboard")
            .await
            .expect("clipboard set");
        client.clipboard_delete(42).await.expect("clipboard delete");
        client.clipboard_clear().await.expect("clipboard clear");

        let local = tempfile::tempdir().expect("local tempdir");
        let source = local.path().join("upload.txt");
        let downloaded = local.path().join("download.txt");
        std::fs::write(&source, b"upload-data").expect("source");
        client
            .upload(
                &source,
                "/storage/emulated/0/upload.txt",
                TransferOptions {
                    overwrite: true,
                    ..Default::default()
                },
            )
            .await
            .expect("upload");
        client
            .download(
                "/storage/emulated/0/test.txt",
                &downloaded,
                TransferOptions::default(),
            )
            .await
            .expect("download");
        assert_eq!(
            std::fs::read(&downloaded).expect("downloaded file"),
            b"download-data"
        );

        client.close().await.expect("close");
        assert!(fake.forward_is_clean());
        let calls = fake.adb_calls();
        assert!(calls.contains("forward --remove tcp:"));
        assert!(!calls.contains("force-stop"));
        fake.finish().await;
    }

    #[tokio::test]
    async fn fake_ssp_missing_device_fields_use_domain_defaults() {
        let fake = FakeSsp::start(FakeSspConfig {
            minimal_device_info: true,
            ..Default::default()
        })
        .await;
        let client = fake.connect().await;
        assert_eq!(client.root_path(), "/sdcard");
        assert!(client.device_info().model.is_none());
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn fake_ssp_remote_errors_preserve_phone_error_codes() {
        let fake = FakeSsp::start(FakeSspConfig {
            create_error: true,
            ..Default::default()
        })
        .await;
        let client = fake.connect().await;
        let error = client
            .create_dir("/storage/emulated/0/denied")
            .await
            .expect_err("mkdir should fail");
        assert!(matches!(
            error,
            Error::RemoteIo {
                code: Some(code),
                ..
            } if code == SspFileIoError::FileIoPermissionError as i32
        ));
        client.close().await.expect("close");
        fake.finish().await;

        let fake = FakeSsp::start(FakeSspConfig {
            partial_delete: true,
            ..Default::default()
        })
        .await;
        let client = fake.connect().await;
        let error = client
            .delete(
                &[
                    "/storage/emulated/0/a".to_string(),
                    "/storage/emulated/0/b".to_string(),
                ],
                DeleteOptions::default(),
            )
            .await
            .expect_err("partial delete should fail");
        assert!(matches!(
            error,
            Error::RemoteIo {
                code: Some(code),
                ..
            } if code == SspFileIoError::FileIoPermissionError as i32
        ));
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn fake_ssp_download_errors_clean_temporary_files_and_target() {
        let fake = FakeSsp::start(FakeSspConfig {
            download: DownloadBehavior::Md5Mismatch,
            ..Default::default()
        })
        .await;
        let local = tempfile::tempdir().expect("local tempdir");
        let target = local.path().join("target.txt");
        std::fs::write(&target, b"keep").expect("target");
        let client = fake.connect().await;
        let error = client
            .download(
                "/storage/emulated/0/test.txt",
                &target,
                TransferOptions {
                    overwrite: true,
                    ..Default::default()
                },
            )
            .await
            .expect_err("md5 mismatch");
        assert!(matches!(error, Error::Protocol(_)));
        assert_eq!(
            std::fs::read(&target).expect("target after failure"),
            b"keep"
        );
        assert!(
            std::fs::read_dir(local.path())
                .expect("local entries")
                .filter_map(std::result::Result::ok)
                .all(|entry| !entry
                    .file_name()
                    .to_string_lossy()
                    .contains("handshaker-part"))
        );
        client.close().await.expect("close");
        fake.finish().await;

        let fake = FakeSsp::start(FakeSspConfig {
            download: DownloadBehavior::Short,
            ..Default::default()
        })
        .await;
        let local = tempfile::tempdir().expect("local tempdir");
        let target = local.path().join("short.txt");
        let client = fake.connect().await;
        assert!(
            client
                .download(
                    "/storage/emulated/0/test.txt",
                    &target,
                    TransferOptions::default(),
                )
                .await
                .is_err()
        );
        assert!(!target.exists());
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn fake_ssp_upload_error_states_are_reported() {
        for behavior in [
            UploadBehavior::NotReady,
            UploadBehavior::Failed,
            UploadBehavior::Canceled,
        ] {
            let fake = FakeSsp::start(FakeSspConfig {
                upload: behavior,
                ..Default::default()
            })
            .await;
            let local = tempfile::tempdir().expect("local tempdir");
            let source = local.path().join("upload.txt");
            std::fs::write(&source, b"upload-data").expect("source");
            let client = fake.connect().await;
            let error = client
                .upload(
                    &source,
                    "/storage/emulated/0/upload.txt",
                    TransferOptions {
                        overwrite: true,
                        ..Default::default()
                    },
                )
                .await
                .expect_err("upload failure");
            assert!(matches!(error, Error::RemoteIo { .. }));
            client.close().await.expect("close");
            fake.finish().await;
        }
    }
}
