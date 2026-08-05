use std::io::{Read, Write};
use std::net::SocketAddr;
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
use tokio::io::{AsyncReadExt, AsyncWrite};
use uuid::Uuid;

use crate::cancellation::RequestOptions;
use crate::domain::{
    AdbDevice, AudioAlbum, AudioFile, AudioLibrary, BatchTransferFailure, BatchTransferItem,
    BatchTransferOptions, BatchTransferProgress, BatchTransferResult, ClipboardEntry,
    DeleteOptions, DeviceInfo, ExifData, ImageAlbum, ImageFile, PhotoLibrary, PhotoSyncResult,
    RemoteFile, Thumbnails, TransferDirection, TransferOptions, TransferProgress, TrustRecordInfo,
    VideoAlbum, VideoFile, VideoLibrary, WifiDevice,
};
use crate::error::{Error, Result};
use crate::events::{EventFilter, EventSubscription};
use crate::exif_parser;
use crate::i18n;
use crate::protocol::frame::{MAX_UPSTREAM_PAYLOAD, WireLog};
use crate::protocol::handshake::AdbRawKeyExchange;
use crate::protocol::proto::*;
use crate::protocol::wifi_handshake::WifiTrustHandshake;
use crate::session::Session;
use crate::state::StateStore;
use crate::transport::TransportCleanup;
use crate::transport::TransportConnector;
use crate::transport::adb::{AdbConnector, list_devices, list_devices_with_timeout};
use crate::transport::usb::UsbConnector;
use crate::transport::wifi::WifiConnector;
use futures_util::StreamExt;

// These fields describe the compatible original macOS host protocol identity,
// not this crate's release version. Older values cause the phone to close the
// connection immediately after GET_DEVICE_INFO.
const COMPATIBLE_HOST_APP_VERSION: &str = "2.5.6";
const COMPATIBLE_HOST_APP_VERSION_CODE: u32 = 408;

/// Upper bound for a single media-library or thumbnail response. Libraries are
/// received in full (the protocol has no pagination), so an oversized reply is
/// treated as a protocol error instead of being buffered without limit.
const MEDIA_RESPONSE_LIMIT: usize = 64 * 1024 * 1024;

/// Decode a media response after enforcing the size cap.
fn decode_media_response<M: prost::Message + Default>(body: &[u8]) -> Result<M> {
    if body.len() > MEDIA_RESPONSE_LIMIT {
        return Err(Error::Protocol(i18n::format(
            "media.response_too_large",
            &[&(body.len() / 1024 / 1024).to_string()],
        )));
    }
    M::decode(body).map_err(|error| {
        Error::Protocol(i18n::format("error.protobuf_decode", &[&error.to_string()]))
    })
}

/// A supported connection target.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ConnectionTarget {
    /// Connect through the verified ADB service and forward.
    Adb { serial: Option<String> },
    /// Connect directly over WiFi to the phone's dynamic SSP port.
    Wifi { address: SocketAddr },
    /// Connect over the USB AOA accessory channel (bulk byte stream).
    /// `location_id` is the `bus-ports` location string when specified.
    Usb { location_id: Option<String> },
}

/// Options controlling a HandShaker connection.
#[derive(Debug, Clone)]
pub struct ClientOptions {
    /// Timeout applied to ADB commands, handshake, requests, and transfers.
    pub timeout: Duration,
    /// Interval between keep-alive heartbeat requests.
    pub heartbeat_interval: Duration,
    /// Computer name reported to the phone during the Wi-Fi handshake;
    /// `None` falls back to the host OS name.
    pub host_name: Option<String>,
    /// Optional path for an explicit, sensitive wire log.
    pub wire_log: Option<PathBuf>,
    /// P2-4: dump payload bytes into the wire log. Default false: the log
    /// records header/type/length only. Payloads may contain clipboard
    /// text, paths and media bytes — enabling this is an explicit
    /// sensitive-data opt-in.
    pub wire_log_payload: bool,
    /// Path to the adb executable.
    pub adb_path: PathBuf,
}

/// Optional phone-side push callbacks enabled during the initial device-info request.
#[derive(Debug, Clone, Copy, Default)]
pub struct EventCallbacks {
    /// Ask the phone to report device information changes.
    pub device_info: bool,
    /// Ask the phone to report photo-library changes.
    pub photo_library: bool,
    /// Ask the phone to report audio-library changes.
    pub audio_library: bool,
    /// Ask the phone to report video-library changes.
    pub video_library: bool,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            heartbeat_interval: Duration::from_secs(10),
            host_name: None,
            wire_log: None,
            wire_log_payload: false,
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
/// use handshaker_core::{ClientOptions, ConnectionTarget, HandShakerClient};
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
    cleanup: Option<TransportCleanup>,
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

    /// Discover HandShaker WiFi devices over mDNS without starting any service.
    ///
    /// Browsing runs for up to `browse_timeout`; the advertised WiFi port is
    /// dynamic and is read fresh from the mDNS SRV record.
    pub async fn discover_wifi_devices(browse_timeout: Duration) -> Result<Vec<WifiDevice>> {
        crate::discovery::discover_wifi_devices(browse_timeout).await
    }

    /// List locally persisted WiFi trust records (derived keys are never
    /// exposed), using the default state store.
    pub async fn list_trusted_devices() -> Result<Vec<TrustRecordInfo>> {
        Self::list_trusted_devices_with_store(StateStore::discover()?).await
    }

    /// List locally persisted WiFi trust records with an explicit state
    /// store, which controls where trust records live (Phase D / D3).
    pub async fn list_trusted_devices_with_store(
        state_store: StateStore,
    ) -> Result<Vec<TrustRecordInfo>> {
        let state = state_store.load_or_create()?;
        Ok(state
            .trust
            .iter()
            .map(|(device_uuid, record)| TrustRecordInfo {
                device_uuid: device_uuid.clone(),
                device_name: record.device_name.clone(),
                updated_at: record.updated_at,
            })
            .collect())
    }

    /// Remove the local trust record for a device (default state store);
    /// returns whether one existed.
    pub async fn remove_trusted_device(device_uuid: &str) -> Result<bool> {
        Self::remove_trusted_device_with_store(StateStore::discover()?, device_uuid).await
    }

    /// Remove the local trust record for a device with an explicit state
    /// store (Phase D / D3); returns whether one existed.
    pub async fn remove_trusted_device_with_store(
        state_store: StateStore,
        device_uuid: &str,
    ) -> Result<bool> {
        state_store.remove_trust(device_uuid)
    }

    /// Connect over WiFi, send TRUST_REMOVE to clear the phone-side record,
    /// and close. No device-info exchange is performed.
    ///
    /// The connected phone must report `expected_device_uuid`; a mismatch
    /// aborts the reset instead of silently targeting a different device.
    pub async fn reset_wifi_trust(
        address: SocketAddr,
        expected_device_uuid: &str,
        options: ClientOptions,
    ) -> Result<()> {
        Self::reset_wifi_trust_with_state_store(
            address,
            expected_device_uuid,
            options,
            StateStore::discover()?,
        )
        .await
    }

    /// Reset WiFi trust with an explicit state store (Phase D / D3), so the
    /// caller's `state_dir` controls where records live.
    pub async fn reset_wifi_trust_with_state_store(
        address: SocketAddr,
        expected_device_uuid: &str,
        options: ClientOptions,
        state_store: StateStore,
    ) -> Result<()> {
        let state = state_store.load_or_create()?;
        let wire_log = options
            .wire_log
            .as_deref()
            .map(|path| WireLog::open(path, options.wire_log_payload))
            .transpose()?
            .map(Arc::new);
        let connected = WifiConnector::new(address, options.timeout)
            .connect()
            .await?;
        let handshake = WifiTrustHandshake::new_with_trust_remove(state.host_uuid.to_string())
            .with_host_name(options.host_name.clone());
        let session = Session::establish(
            connected.stream,
            options.timeout,
            options.heartbeat_interval,
            wire_log,
            &handshake,
            address.to_string(),
        )
        .await?;
        let result = match &session.handshake_info {
            Some(info) if info.device_uuid == expected_device_uuid => Ok(()),
            Some(info) => Err(Error::Handshake(i18n::format(
                "trust.reset_device_mismatch",
                &[&info.device_uuid, expected_device_uuid],
            ))),
            None => Err(Error::Handshake(
                i18n::text("trust.reset_no_device").to_string(),
            )),
        };
        // Close the session first so a close failure cannot mask the
        // uuid mismatch reported above.
        let close = session.close().await;
        match (result, close) {
            (Err(error), _) => Err(error),
            (Ok(()), Err(close_error)) => Err(close_error),
            (Ok(()), Ok(())) => Ok(()),
        }
    }

    /// Connect, complete the ADB raw-key handshake, and fetch device info.
    pub async fn connect(target: ConnectionTarget, options: ClientOptions) -> Result<Self> {
        Self::connect_with_event_callbacks(target, options, EventCallbacks::default()).await
    }

    /// Connect with explicit phone-side push callback settings.
    pub async fn connect_with_event_callbacks(
        target: ConnectionTarget,
        options: ClientOptions,
        callbacks: EventCallbacks,
    ) -> Result<Self> {
        Self::connect_with_state(target, options, StateStore::discover()?, callbacks).await
    }

    /// Connect with an explicit state store, which controls where trust
    /// records and the host UUID live (M8.1 Phase B / B4). Callers that want
    /// the default config directory keep using [`Self::connect_with_event_callbacks`].
    pub async fn connect_with_state(
        target: ConnectionTarget,
        options: ClientOptions,
        state: StateStore,
        callbacks: EventCallbacks,
    ) -> Result<Self> {
        let state_store = state;
        let state = state_store.load_or_create()?;
        let wire_log = options
            .wire_log
            .as_deref()
            .map(|path| WireLog::open(path, options.wire_log_payload))
            .transpose()?
            .map(Arc::new);
        let (serial, connected) = match &target {
            ConnectionTarget::Adb { serial } => {
                let connector =
                    AdbConnector::new(options.adb_path.clone(), serial.clone(), options.timeout);
                let connected = connector.connect().await?;
                (connected.label.clone(), connected)
            }
            ConnectionTarget::Wifi { address } => {
                let connector = WifiConnector::new(*address, options.timeout);
                let connected = connector.connect().await?;
                (connected.label.clone(), connected)
            }
            ConnectionTarget::Usb { location_id } => {
                let location = location_id.clone();
                let uuid = state.host_uuid.to_string();
                let timeout = options.timeout;
                // libusb open/control/claim and the accessory switch poll are
                // blocking; keep them off the tokio worker threads. The phone
                // app opens the accessory asynchronously after ATTACHED, so a
                // first attempt can race it: retry once after a short pause.
                let (label, connected) = tokio::task::spawn_blocking(move || {
                    let attempt = || {
                        UsbConnector::new(location.clone(), &uuid, timeout)
                            .connect()
                            .map_err(|error| {
                                // LocalIo -> Transport so the CLI maps it to
                                // the connection error class.
                                if matches!(error, Error::LocalIo(_)) {
                                    Error::Transport(error.to_string())
                                } else {
                                    error
                                }
                            })
                    };
                    let connected = match attempt() {
                        Ok(connected) => connected,
                        Err(first) => {
                            std::thread::sleep(Duration::from_secs(3));
                            match attempt() {
                                Ok(connected) => connected,
                                Err(_) => return Err(first),
                            }
                        }
                    };
                    Ok::<_, Error>((connected.label.clone(), connected))
                })
                .await
                .map_err(|join| {
                    Error::Transport(i18n::format(
                        "usb.connect_task_failed",
                        &[&join.to_string()],
                    ))
                })??;
                (label, connected)
            }
        };
        let session = match &target {
            ConnectionTarget::Adb { .. } => {
                Session::establish(
                    connected.stream,
                    options.timeout,
                    options.heartbeat_interval,
                    wire_log,
                    &AdbRawKeyExchange,
                    serial.clone(),
                )
                .await?
            }
            ConnectionTarget::Wifi { .. } => {
                let handshake =
                    WifiTrustHandshake::new(state.host_uuid.to_string(), state.trust.clone())
                        .with_host_name(options.host_name.clone())
                        .with_trust_store(state_store.clone());
                Session::establish(
                    connected.stream,
                    options.timeout,
                    options.heartbeat_interval,
                    wire_log,
                    &handshake,
                    serial.clone(),
                )
                .await?
            }
            ConnectionTarget::Usb { .. } => {
                Session::establish(
                    connected.stream,
                    options.timeout,
                    options.heartbeat_interval,
                    wire_log,
                    &AdbRawKeyExchange,
                    serial.clone(),
                )
                .await?
            }
        };
        // Persist the trust record after a successful WiFi TRUST_ALWAYS.
        if let Some(info) = &session.handshake_info
            && let Some(derived_key) = &info.derived_key
        {
            state_store.upsert_trust(
                &info.device_uuid,
                info.device_name.as_deref(),
                derived_key,
            )?;
        }
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
        client.device = client.fetch_device_info(callbacks).await?;
        // Backfill identity from the WiFi handshake when device info lacked it.
        if let Some(info) = &client
            .session
            .as_ref()
            .and_then(|s| s.handshake_info.as_ref())
        {
            let device = &mut client.device;
            if device.phone_id.is_none() {
                device.phone_id = Some(info.device_uuid.clone());
            }
            if device.name.is_none() {
                device.name = info.device_name.clone();
            }
            if device.apk_version.is_none() {
                device.apk_version = info.apk_version.clone();
            }
            if device.apk_version_name.is_none() {
                device.apk_version_name = info.apk_version_name.clone();
            }
        }
        Ok(client)
    }

    #[cfg(test)]
    pub(crate) async fn connect_for_test_with_callbacks(
        target: ConnectionTarget,
        options: ClientOptions,
        state_path: PathBuf,
        callbacks: EventCallbacks,
    ) -> Result<Self> {
        Self::connect_with_state(target, options, StateStore::at(state_path), callbacks).await
    }

    /// Subscribe to typed unsolicited events from this connection.
    pub fn subscribe_events(&self, filter: EventFilter) -> EventSubscription {
        self.session
            .as_ref()
            .map(|session| session.subscribe_events(filter.clone()))
            .unwrap_or_else(|| EventSubscription::closed(filter))
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
        self.ping_with_options(RequestOptions::default()).await
    }

    /// Send one heartbeat with optional cancellation.
    pub async fn ping_with_options(&self, options: RequestOptions) -> Result<PingResult> {
        let request = SspHeartBeatRequest {
            r#type: Some(SspRequestType::HeartBeatRequest as i32),
            host_timestamp: Some(unix_seconds()),
        };
        let start = Instant::now();
        let response = SspHeartBeatResponse::decode(
            self.session()?
                .request_with_options(&request, options)
                .await?
                .as_slice(),
        )?;
        Ok(PingResult {
            round_trip_ms: start.elapsed().as_millis(),
            host_timestamp: response.host_timestamp,
            client_timestamp: response.client_timestamp,
        })
    }

    /// List files below a remote directory up to depth.
    pub async fn list_dir(&self, path: &str, depth: u32) -> Result<Vec<RemoteFile>> {
        self.list_dir_with_options(path, depth, RequestOptions::default())
            .await
    }

    /// List files below a remote directory with optional cancellation.
    pub async fn list_dir_with_options(
        &self,
        path: &str,
        depth: u32,
        options: RequestOptions,
    ) -> Result<Vec<RemoteFile>> {
        let request = SspGetDirFilesRequest {
            r#type: Some(SspRequestType::GetDirFilesRequest as i32),
            dir: Some(ssp_file(path, true, None)),
            maxdepth: Some(depth.max(1)),
        };
        let response = SspGetDirFilesResponse::decode(
            self.session()?
                .request_with_options(&request, options)
                .await?
                .as_slice(),
        )?;
        Ok(response.file.into_iter().map(remote_file).collect())
    }

    /// Count files below a remote directory with optional exclusion patterns.
    pub async fn file_count(&self, path: &str, depth: u32, exclusions: Vec<String>) -> Result<u64> {
        self.file_count_with_options(path, depth, exclusions, RequestOptions::default())
            .await
    }

    /// Count files with optional exclusion patterns and cancellation.
    pub async fn file_count_with_options(
        &self,
        path: &str,
        depth: u32,
        exclusions: Vec<String>,
        options: RequestOptions,
    ) -> Result<u64> {
        let request = SspGetFileCountRequest {
            r#type: Some(SspRequestType::GetFileCountRequest as i32),
            dir: Some(ssp_file(path, true, None)),
            maxdepth: Some(depth.max(1)),
            exclusion_pattern: exclusions,
        };
        let response = SspGetFileCountResponse::decode(
            self.session()?
                .request_with_options(&request, options)
                .await?
                .as_slice(),
        )?;
        Ok(response.count.unwrap_or(0))
    }

    /// Check whether a remote file path exists.
    pub async fn file_exists(&self, path: &str) -> Result<bool> {
        self.file_exists_with_options(path, RequestOptions::default())
            .await
    }

    /// Check whether a remote path exists with optional cancellation.
    pub async fn file_exists_with_options(
        &self,
        path: &str,
        options: RequestOptions,
    ) -> Result<bool> {
        let request = SspFileExistRequest {
            r#type: Some(SspRequestType::GetFileExistRequest as i32),
            file: Some(ssp_file(path, false, None)),
        };
        let response = SspFileExistResponse::decode(
            self.session()?
                .request_with_options(&request, options)
                .await?
                .as_slice(),
        )?;
        Ok(response.exist.unwrap_or(false))
    }

    /// Read metadata for a remote path.
    pub async fn stat(&self, path: &str) -> Result<Option<RemoteFile>> {
        self.stat_with_options(path, RequestOptions::default())
            .await
    }

    /// Read metadata with optional cancellation.
    pub async fn stat_with_options(
        &self,
        path: &str,
        options: RequestOptions,
    ) -> Result<Option<RemoteFile>> {
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
                ext_data: None,
            }));
        }
        let (parent, _) = split_remote_path(path)?;
        Ok(self
            .list_dir_with_options(parent, 1, options)
            .await?
            .into_iter()
            .find(|file| file.path == path))
    }

    /// Create a remote directory and return its metadata.
    pub async fn create_dir(&self, path: &str) -> Result<RemoteFile> {
        self.create_dir_with_options(path, RequestOptions::default())
            .await
    }

    /// Create a remote directory with optional cancellation.
    pub async fn create_dir_with_options(
        &self,
        path: &str,
        options: RequestOptions,
    ) -> Result<RemoteFile> {
        let request = SspCreateFolderRequest {
            r#type: Some(SspRequestType::GetCreateFolderRequest as i32),
            file: Some(ssp_file(path, true, None)),
        };
        let response = SspCreateFolderResponse::decode(
            self.session()?
                .request_with_options(&request, options)
                .await?
                .as_slice(),
        )?;
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
        self.rename_with_options(source, target, RequestOptions::default())
            .await
    }

    /// Rename a remote path with optional cancellation.
    pub async fn rename_with_options(
        &self,
        source: &str,
        target: &str,
        options: RequestOptions,
    ) -> Result<()> {
        let request = SspRenameFileRequest {
            r#type: Some(SspRequestType::GetRenameFileRequest as i32),
            source_file: Some(ssp_file(source, false, None)),
            target_file: Some(ssp_file(target, false, None)),
        };
        let response = SspRenameFileResponse::decode(
            self.session()?
                .request_with_options(&request, options)
                .await?
                .as_slice(),
        )?;
        ensure_remote_success(
            response.succeed,
            response.error_code,
            response.error_message,
        )
    }

    /// Delete remote paths with the requested trash and sync options.
    /// Update file metadata on the phone (UPDATE_FILE_INFO, request type 40).
    ///
    /// `files` carries the paths plus the fields the phone should write back
    /// into its media store (star, orientation, timestamps, trash, ...). The
    /// phone answers with a single success flag; `is_sync` asks the phone to
    /// feed the change into its sync manager.
    ///
    /// Wire semantics come from the Android `d/c.java:508-545` decompilation
    /// (`docs/08-file-operations.md` §8.11); there is no capture yet.
    pub async fn update_files_info(&self, files: &[RemoteFile], is_sync: bool) -> Result<bool> {
        let request = SspUpdateFileRequest {
            r#type: Some(SspRequestType::UpdateFileInfo as i32),
            files: files.iter().map(ssp_file_from_remote).collect(),
            is_sync: Some(is_sync),
        };
        let response =
            SspUpdateFileResponse::decode(self.session()?.request(&request).await?.as_slice())?;
        if response.is_success != Some(true) {
            return Err(Error::Protocol(
                i18n::text("client.update_file_info_failed").to_string(),
            ));
        }
        Ok(true)
    }

    /// Start a one-shot photo sync (PHOTO_SYNC_REQUEST, type 37).
    ///
    /// `files` must be the host's previous snapshot (the sync ledger) so the
    /// phone can answer `is_first` for a fresh pc and return its current file
    /// state; the host then diffs that state against its ledger.
    ///
    /// Wire semantics come from the Android `f/e.java` decompilation
    /// (`docs/10-photo-sync.md`); there is no capture yet — first real-device
    /// evidence lands with the M6 acceptance run.
    pub async fn photo_sync(&self, pc_id: &str, files: &[RemoteFile]) -> Result<PhotoSyncResult> {
        let request = SspPhotoSyncRequest {
            r#type: Some(SspRequestType::PhotoSyncRequest as i32),
            pc_id: Some(pc_id.to_string()),
            files: files.iter().map(ssp_file_from_remote).collect(),
        };
        let response =
            SspPhotoSyncResponse::decode(self.session()?.request(&request).await?.as_slice())?;
        // The phone omits `is_success` on success (proto2 default-field
        // omission, real-device verified 2026-08-03); an explicit `false` is
        // the only rejection signal. Callers decide how to treat None.
        Ok(PhotoSyncResult {
            is_first: response.is_first,
            files: response.files.into_iter().map(remote_file).collect(),
            is_success: response.is_success,
        })
    }

    /// Toggle the real-time sync monitor (SYNC_MONITOR_REQUEST, type 39).
    ///
    /// Returns whether the phone accepted the switch. The phone answers
    /// `is_success=false` when the request is invalid for the current sync
    /// state (e.g. enabling monitor while idle — `f/e.java` requestSyncMonitor
    /// requires `mSyncStatus != 0`), which is an informative answer, not a
    /// protocol failure. Callers decide how to surface it. As with
    /// photo_sync, `is_success` may be omitted on success.
    pub async fn sync_monitor(&self, enabled: bool) -> Result<bool> {
        let request = SspSyncMonitorRequest {
            r#type: Some(SspRequestType::SyncMonitorRequest as i32),
            is_sync_monitor: Some(enabled),
        };
        let response =
            SspSyncMonitorResponse::decode(self.session()?.request(&request).await?.as_slice())?;
        Ok(response.is_success.unwrap_or(true))
    }

    pub async fn delete(
        &self,
        paths: &[String],
        options: DeleteOptions,
    ) -> Result<Vec<RemoteFile>> {
        self.delete_with_options(paths, options, RequestOptions::default())
            .await
    }

    /// Delete remote paths with optional cancellation.
    pub async fn delete_with_options(
        &self,
        paths: &[String],
        options: DeleteOptions,
        request_options: RequestOptions,
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
        let response = SspDeleteFileResponse::decode(
            self.session()?
                .request_with_options(&request, request_options)
                .await?
                .as_slice(),
        )?;
        ensure_remote_success(
            response.succeed,
            response.error_code,
            response.error_message,
        )?;
        ensure_deleted_files_succeeded(&response.file)?;
        Ok(response.file.into_iter().map(remote_file).collect())
    }

    /// Register or unregister change monitoring on a remote directory.
    ///
    /// While registered, the phone pushes file events
    /// (`ClientEvent::DirectoryChanged`) over the event stream; unregister
    /// with `register = false` when done. Only plain directory paths are
    /// supported (MediaStore URIs are ignored by the phone observer).
    pub async fn monitor_folder(&self, path: &str, register: bool) -> Result<()> {
        self.monitor_folder_with_options(path, register, RequestOptions::default())
            .await
    }

    /// Register or unregister directory monitoring with optional cancellation.
    pub async fn monitor_folder_with_options(
        &self,
        path: &str,
        register: bool,
        options: RequestOptions,
    ) -> Result<()> {
        let request = SspMonitorFolderRequest {
            r#type: Some(SspRequestType::MonitorFolderRequest as i32),
            file: Some(ssp_file(path, true, None)),
            register: Some(register),
        };
        let response = SspMonitorFolderResponseHeader::decode(
            self.session()?
                .request_with_options(&request, options)
                .await?
                .as_slice(),
        )?;
        ensure_remote_success(response.succeed, None, response.error_message)
    }

    /// Download one remote file through a temporary local file.
    pub async fn download(
        &self,
        remote: &str,
        local: &Path,
        options: TransferOptions,
    ) -> Result<u64> {
        self.download_with_options(remote, local, options, RequestOptions::default())
            .await
    }

    /// Download one remote file with transfer and cancellation options.
    pub async fn download_with_options(
        &self,
        remote: &str,
        local: &Path,
        options: TransferOptions,
        request_options: RequestOptions,
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
                offset: Some(options.offset),
                length: Some(0),
            }),
            need_md5: Some(true),
            gzip: Some(false),
            is_sync: Some(false),
        };
        let mut open = self
            .session()?
            .open_with_options(&request, request_options)
            .await?;
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
        self.upload_with_options(local, remote, options, RequestOptions::default())
            .await
    }

    /// Upload one local file with transfer and cancellation options.
    pub async fn upload_with_options(
        &self,
        local: &Path,
        remote: &str,
        options: TransferOptions,
        request_options: RequestOptions,
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
        if !options.overwrite
            && self
                .file_exists_with_options(remote, request_options.clone())
                .await?
        {
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
        let mut open = self
            .session()?
            .open_with_options(&request, request_options)
            .await?;
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

    /// Transfer a list of files serially, aggregating per-file failures.
    ///
    /// Every item is attempted regardless of earlier failures; the returned
    /// [`BatchTransferResult`] lists successful items and per-file failures.
    /// Directories must already be expanded into file items (see
    /// [`HandShakerClient::upload_tree`] / [`download_tree`]).
    pub async fn upload_many(
        &self,
        items: &[BatchTransferItem],
        options: BatchTransferOptions,
    ) -> Result<BatchTransferResult> {
        self.batch_transfer(items, options, TransferDirection::Upload)
            .await
    }

    /// Download a list of files serially, aggregating per-file failures.
    pub async fn download_many(
        &self,
        items: &[BatchTransferItem],
        options: BatchTransferOptions,
    ) -> Result<BatchTransferResult> {
        self.batch_transfer(items, options, TransferDirection::Download)
            .await
    }

    async fn batch_transfer(
        &self,
        items: &[BatchTransferItem],
        options: BatchTransferOptions,
        direction: TransferDirection,
    ) -> Result<BatchTransferResult> {
        if options.concurrency == 0 || options.concurrency > 8 {
            return Err(Error::Usage(i18n::format(
                "client.batch_concurrency_range",
                &[&options.concurrency.to_string()],
            )));
        }
        let total = items.len();
        let done = std::sync::atomic::AtomicUsize::new(0);
        let result = std::sync::Mutex::new(BatchTransferResult::default());
        let transfer_options = TransferOptions {
            overwrite: options.overwrite,
            progress: None,
            offset: options.offset,
        };
        let callback = options.progress;
        // `cloned` + `async move` so each future owns its item: a borrowing
        // map closure would require HRTB Send proofs that fail inside
        // `tokio::spawn` callers (the application plan executor). Only
        // references and Copy flags are captured.
        let is_upload = matches!(direction, TransferDirection::Upload);
        let this = self;
        let transfer_options = &transfer_options;
        let done = &done;
        let callback = &callback;
        let cancel = &options.cancel;
        let futures = items.iter().cloned().map(|item| async move {
            // Cancellation (Phase D review fix): once the token is cancelled,
            // skip items that have not started; the batch below then returns
            // `Error::Interrupted` so callers land on the Cancelled state.
            if cancel.as_ref().is_some_and(|token| token.is_cancelled()) {
                return Err(BatchTransferFailure {
                    source: item.source,
                    target: item.target,
                    // Internal marker only: when a cancellation is requested
                    // mid-batch the whole batch returns `Error::Interrupted`
                    // below, so this message never reaches the user.
                    message: "cancelled".to_string(),
                    code: Some(crate::error::ErrorCode::Interrupted),
                });
            }
            let outcome = if is_upload {
                this.upload(
                    Path::new(&item.source),
                    &item.target,
                    transfer_options.clone(),
                )
                .await
            } else {
                this.download(
                    &item.source,
                    Path::new(&item.target),
                    transfer_options.clone(),
                )
                .await
            };
            let entry = match outcome {
                Ok(_) => Ok(item),
                Err(error) => Err(BatchTransferFailure {
                    source: item.source,
                    target: item.target,
                    message: error.to_string(),
                    code: Some(error.code()),
                }),
            };
            let completed = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if let Some(callback) = &callback {
                callback(BatchTransferProgress {
                    done: completed,
                    total,
                });
            }
            entry
        });
        let mut stream = futures_util::stream::iter(futures).buffer_unordered(options.concurrency);
        while let Some(entry) = stream.next().await {
            match entry {
                Ok(item) => result.lock().expect("batch result lock").ok.push(item),
                Err(failure) => result
                    .lock()
                    .expect("batch result lock")
                    .failures
                    .push(failure),
            }
        }
        let result = result.into_inner().expect("batch result lock");
        // A cancellation requested mid-batch must surface as an interrupt,
        // not as an ordinary aggregated failure list.
        if cancel.as_ref().is_some_and(|token| token.is_cancelled())
            && result
                .failures
                .iter()
                .any(|failure| failure.code == Some(crate::error::ErrorCode::Interrupted))
        {
            return Err(Error::Interrupted);
        }
        Ok(result)
    }

    /// Upload a local directory tree to a remote directory, mirroring the
    /// directory structure. Remote directories are created before files are
    /// uploaded; per-file failures are aggregated without aborting.
    pub async fn upload_tree(
        &self,
        local_dir: &Path,
        remote_dir: &str,
        options: BatchTransferOptions,
    ) -> Result<BatchTransferResult> {
        let mut items = Vec::new();
        let mut directories = std::collections::BTreeSet::new();
        collect_upload_tree(local_dir, remote_dir, &mut items, &mut directories)?;
        // Create remote directories shallow-first so parents exist before
        // children; skip ones that already exist.
        for directory in &directories {
            if !self.file_exists(directory).await? {
                self.create_dir(directory).await?;
            }
        }
        self.upload_many(&items, options).await
    }

    /// Download a remote directory tree into a local directory, mirroring the
    /// structure. The remote subtree is listed with a single recursive
    /// `GET_DIR_FILES` call; per-file failures are aggregated without aborting.
    pub async fn download_tree(
        &self,
        remote_dir: &str,
        local_dir: &Path,
        options: BatchTransferOptions,
    ) -> Result<BatchTransferResult> {
        tokio::fs::create_dir_all(local_dir)
            .await
            .map_err(|error| {
                Error::LocalIo(i18n::format(
                    "client.create_failed",
                    &[&local_dir.display().to_string(), &error.to_string()],
                ))
            })?;
        let files = self.list_dir(remote_dir, u32::MAX).await?;
        let mut items = Vec::new();
        let base = remote_dir.trim_end_matches('/');
        let base_prefix = format!("{base}/");
        for file in files {
            // The phone controls `file.path`: reject any entry outside the
            // requested subtree (absolute escape) or containing `..`/root
            // components before it can influence a host path.
            let Some(relative) = file.path.strip_prefix(&base_prefix) else {
                return Err(Error::Protocol(i18n::format(
                    "client.download_path_escape",
                    &[&file.path],
                )));
            };
            let has_escape_component = Path::new(relative).components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            });
            if has_escape_component {
                return Err(Error::Protocol(i18n::format(
                    "client.download_path_escape",
                    &[&file.path],
                )));
            }
            let local = local_dir.join(relative);
            // Defense in depth: the normalized target must stay under the
            // requested local root.
            if !local.starts_with(local_dir) {
                return Err(Error::Protocol(i18n::format(
                    "client.download_path_escape",
                    &[&file.path],
                )));
            }
            if file.is_directory {
                tokio::fs::create_dir_all(&local).await.map_err(|error| {
                    Error::LocalIo(i18n::format(
                        "client.create_failed",
                        &[&local.display().to_string(), &error.to_string()],
                    ))
                })?;
            } else {
                items.push(BatchTransferItem {
                    source: file.path,
                    target: local.display().to_string(),
                });
            }
        }
        self.download_many(&items, options).await
    }

    /// Read and decompress the phone clipboard history.
    pub async fn clipboard_list(&self) -> Result<Vec<ClipboardEntry>> {
        self.clipboard_list_with_options(RequestOptions::default())
            .await
    }

    /// Read and decompress clipboard history with optional cancellation.
    pub async fn clipboard_list_with_options(
        &self,
        options: RequestOptions,
    ) -> Result<Vec<ClipboardEntry>> {
        let entries = self.fetch_clipboards(options).await?;
        entries.into_iter().map(decode_clipboard).collect()
    }

    /// Add a text entry to the phone clipboard.
    pub async fn clipboard_set(&self, text: &str) -> Result<()> {
        self.clipboard_set_with_options(text, RequestOptions::default())
            .await
    }

    /// Add a text entry with optional cancellation.
    pub async fn clipboard_set_with_options(
        &self,
        text: &str,
        options: RequestOptions,
    ) -> Result<()> {
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
        let response = SspPostClipboardResponse::decode(
            self.session()?
                .request_with_options(&request, options)
                .await?
                .as_slice(),
        )?;
        ensure_remote_success(response.succeed, None, None)
    }

    /// Delete one clipboard entry by its millisecond timestamp.
    pub async fn clipboard_delete(&self, timestamp_ms: i64) -> Result<()> {
        self.clipboard_delete_with_options(timestamp_ms, RequestOptions::default())
            .await
    }

    /// Delete one clipboard entry with optional cancellation.
    pub async fn clipboard_delete_with_options(
        &self,
        timestamp_ms: i64,
        options: RequestOptions,
    ) -> Result<()> {
        let clipboard = self
            .fetch_clipboards(options.clone())
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
            self.session()?
                .request_with_options(&request, options)
                .await?
                .as_slice(),
        )?;
        ensure_remote_success(response.succeed, None, None)
    }

    /// Clear the phone clipboard history.
    pub async fn clipboard_clear(&self) -> Result<()> {
        self.clipboard_clear_with_options(RequestOptions::default())
            .await
    }

    /// Clear clipboard history with optional cancellation.
    pub async fn clipboard_clear_with_options(&self, options: RequestOptions) -> Result<()> {
        let request = SspClearClipboardRequest {
            r#type: Some(SspRequestType::ClearClipboardRequest as i32),
        };
        let response = SspClearClipboardResponse::decode(
            self.session()?
                .request_with_options(&request, options)
                .await?
                .as_slice(),
        )?;
        ensure_remote_success(response.succeed, None, None)
    }

    /// Fetch the full phone photo library (images and albums).
    pub async fn get_photo_library(&self) -> Result<PhotoLibrary> {
        self.get_photo_library_with_options(RequestOptions::default())
            .await
    }

    /// Fetch the full phone photo library with optional cancellation.
    pub async fn get_photo_library_with_options(
        &self,
        options: RequestOptions,
    ) -> Result<PhotoLibrary> {
        let request = SspGetPhotoLibraryRequest {
            r#type: Some(SspRequestType::GetPhotoLibRequest as i32),
        };
        let response = decode_media_response::<SspGetPhotoLibraryResponse>(
            self.session()?
                .request_with_options(&request, options)
                .await?
                .as_slice(),
        )?;
        Ok(PhotoLibrary {
            images: response.image.into_iter().map(image_file).collect(),
            albums: response.album.into_iter().map(image_album).collect(),
            camera_album_id: response.camera_album_id,
        })
    }

    /// Fetch the full phone video library (videos and albums).
    pub async fn get_video_library(&self) -> Result<VideoLibrary> {
        self.get_video_library_with_options(RequestOptions::default())
            .await
    }

    /// Fetch the full phone video library with optional cancellation.
    pub async fn get_video_library_with_options(
        &self,
        options: RequestOptions,
    ) -> Result<VideoLibrary> {
        let request = SspGetVideoLibraryRequest {
            r#type: Some(SspRequestType::GetVideoLibRequest as i32),
        };
        let response = decode_media_response::<SspGetVideoLibraryResponse>(
            self.session()?
                .request_with_options(&request, options)
                .await?
                .as_slice(),
        )?;
        Ok(VideoLibrary {
            videos: response.video.into_iter().map(video_file).collect(),
            albums: response.album.into_iter().map(video_album).collect(),
        })
    }

    /// Fetch the full phone audio library (tracks and albums).
    pub async fn get_audio_library(&self) -> Result<AudioLibrary> {
        self.get_audio_library_with_options(RequestOptions::default())
            .await
    }

    /// Fetch the full phone audio library with optional cancellation.
    pub async fn get_audio_library_with_options(
        &self,
        options: RequestOptions,
    ) -> Result<AudioLibrary> {
        let request = SspGetAudioLibraryRequest {
            r#type: Some(SspRequestType::GetAudioLibRequest as i32),
        };
        let response = decode_media_response::<SspGetAudioLibraryResponse>(
            self.session()?
                .request_with_options(&request, options)
                .await?
                .as_slice(),
        )?;
        Ok(AudioLibrary {
            tracks: response.audio.into_iter().map(audio_file).collect(),
            albums: response.album.into_iter().map(audio_album).collect(),
        })
    }

    /// Fetch thumbnails for images, videos, and audio albums in one request.
    ///
    /// Entries are matched by `media_id` first, falling back to `path` when
    /// the id is absent; failed entries come back with `thumbnail_error` set
    /// instead of failing the whole batch.
    pub async fn get_thumbnails(
        &self,
        images: &[ImageFile],
        videos: &[VideoFile],
        audio_albums: &[AudioAlbum],
    ) -> Result<Thumbnails> {
        self.get_thumbnails_with_options(images, videos, audio_albums, RequestOptions::default())
            .await
    }

    /// Fetch thumbnails with optional cancellation.
    pub async fn get_thumbnails_with_options(
        &self,
        images: &[ImageFile],
        videos: &[VideoFile],
        audio_albums: &[AudioAlbum],
        options: RequestOptions,
    ) -> Result<Thumbnails> {
        let request = SspGetThumbnailRequest {
            r#type: Some(SspRequestType::GetThumbnailRequest as i32),
            image: images.iter().map(thumbnail_image_request).collect(),
            video: videos.iter().map(thumbnail_video_request).collect(),
            audio_album: audio_albums
                .iter()
                .map(thumbnail_audio_album_request)
                .collect(),
        };
        let response = decode_media_response::<SspGetThumbnailResponse>(
            self.session()?
                .request_with_options(&request, options)
                .await?
                .as_slice(),
        )?;
        Ok(Thumbnails {
            images: response.image.into_iter().map(image_file).collect(),
            videos: response.video.into_iter().map(video_file).collect(),
            audio_albums: response.audio_album.into_iter().map(audio_album).collect(),
        })
    }

    /// Fetch Exif metadata for a remote media file.
    ///
    /// The file is pulled over the SSP download channel (WiFi or ADB) into a
    /// bounded in-memory buffer and parsed locally with `kamadak-exif`, so the
    /// full EXIF payload is available for any media path without extending the
    /// SSP schema. Files larger than [`exif_parser::EXIF_FETCH_LIMIT`] are rejected
    /// with a protocol error before any data is buffered.
    pub async fn fetch_exif(&self, path: &str) -> Result<ExifData> {
        let bytes = self
            .download_bytes(path, exif_parser::EXIF_FETCH_LIMIT)
            .await?;
        exif_parser::exif_from_bytes(&bytes)
    }

    /// Pull a remote file over the SSP download channel into memory, refusing
    /// bodies larger than `limit` before buffering any data.
    pub(crate) async fn download_bytes(&self, remote: &str, limit: u64) -> Result<Vec<u8>> {
        let request = SspDownloadFileRequest {
            r#type: Some(SspRequestType::GetDownloadFileRequest as i32),
            file: Some(ssp_file(remote, false, None)),
            range: Some(SspDataRange {
                offset: Some(0),
                length: Some(0),
            }),
            need_md5: Some(false),
            gzip: Some(false),
            is_sync: Some(false),
        };
        let mut open = self
            .session()?
            .open_with_options(&request, RequestOptions::default())
            .await?;
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
        if total > limit {
            open.finish().await;
            return Err(Error::Protocol(i18n::format(
                "exif.file_too_large",
                &[&total.to_string(), &limit.to_string()],
            )));
        }
        let mut sink = VecSink::default();
        let receive = open.receive_raw_to(total, &mut sink, |_| {}).await;
        open.finish().await;
        receive?;
        Ok(sink.bytes)
    }

    /// Send QUIT and remove the ADB forward created by this client.
    pub async fn close(mut self) -> Result<()> {
        let session_result = if let Some(session) = self.session.take() {
            session.close().await
        } else {
            Ok(())
        };
        let cleanup_result = if let Some(cleanup) = self.cleanup.take() {
            cleanup.cleanup().await
        } else {
            Ok(())
        };
        session_result.and(cleanup_result)
    }

    async fn fetch_device_info(&self, callbacks: EventCallbacks) -> Result<DeviceInfo> {
        let request = SspGetDeviceInfoRequest {
            r#type: Some(SspRequestType::GetDeviceInfoRequest as i32),
            host_timestamp: Some(unix_seconds()),
            host_smart_sync_protocol_version: Some("1".to_string()),
            need_device_info_callback: Some(callbacks.device_info),
            need_photo_library_callback: Some(callbacks.photo_library),
            need_audio_library_callback: Some(callbacks.audio_library),
            need_video_library_callback: Some(callbacks.video_library),
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

    async fn fetch_clipboards(&self, options: RequestOptions) -> Result<Vec<SspClipboard>> {
        let request = SspGetClipboardRequest {
            r#type: Some(SspRequestType::GetClipboardRequest as i32),
        };
        let response = SspGetClipboardResponse::decode(
            self.session()?
                .request_with_options(&request, options)
                .await?
                .as_slice(),
        )?;
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
        ext_data: file.ext_data,
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

fn ssp_file_from_remote(file: &RemoteFile) -> SspFile {
    SspFile {
        path: Some(file.path.clone()),
        file_size: Some(file.size),
        created_timestamp: file.created_at,
        modified_timestamp: file.modified_at,
        is_directory: Some(file.is_directory),
        checksum: file.checksum.clone(),
        is_trash: file.is_trash,
        id: file.id,
        ext_data: file.ext_data.clone(),
        ..Default::default()
    }
}

fn image_file(file: SspImageFile) -> ImageFile {
    ImageFile {
        path: file.path,
        size: file.file_size,
        created_at: file.created_timestamp,
        modified_at: file.modified_timestamp,
        width: file.width,
        height: file.height,
        orientation: file.orientation,
        media_id: file.media_id,
        album_id: file.album_id,
        mime_type: file.mime_type,
        thumbnail: file.thumbnail,
        album_name: file.album_name,
        date_taken: file.date_taken,
        latitude: file.latitude,
        longitude: file.longitude,
        mini_thumb_magic: file.mini_thumb_magic,
        title: file.title,
        thumbnail_error: file.get_thumbnail_error.unwrap_or(false),
        starred: file.starred.unwrap_or(false),
    }
}

fn image_album(album: SspImageAlbum) -> ImageAlbum {
    ImageAlbum {
        path: album.album_path,
        album_id: album.album_id,
        name: album.album_name,
        cover_image: album.cover_image.map(|cover| Box::new(image_file(cover))),
    }
}

fn video_file(file: SspVideoFile) -> VideoFile {
    VideoFile {
        path: file.path,
        size: file.file_size,
        created_at: file.created_timestamp.map(u64::from),
        modified_at: file.modified_timestamp.map(u64::from),
        width: file.width,
        height: file.height,
        orientation: file.orientation,
        media_id: file.media_id,
        album_id: file.album_id,
        mime_type: file.mime_type,
        thumbnail: file.thumbnail,
        thumbnail_error: file.get_thumbnail_error.unwrap_or(false),
        duration: file.duration,
    }
}

fn video_album(album: SspVideoAlbum) -> VideoAlbum {
    VideoAlbum {
        path: album.album_path,
        album_id: album.album_id,
        name: album.album_name,
    }
}

fn audio_file(file: SspAudioFile) -> AudioFile {
    AudioFile {
        path: file.path,
        size: file.file_size,
        created_at: file.created_timestamp,
        modified_at: file.modified_timestamp,
        media_id: file.media_id,
        album_id: file.album_id,
        title: file.title,
        mime_type: file.mime_type,
        artist_id: file.artist_id,
        artist: file.artist,
        composer: file.composer,
        genre: file.genre,
        comment: file.comment,
        copyright: file.copyright,
        audio_codec: file.audio_codec,
        track: file.track,
        duration: file.duration.map(|seconds| seconds / 1000.0),
    }
}

fn audio_album(album: SspAudioAlbum) -> AudioAlbum {
    AudioAlbum {
        path: album.album_path,
        album_id: album.album_id,
        name: album.album_name,
        artist_id: album.artist_id,
        artist: album.artist,
        year: album.year,
        thumbnail: album.thumbnail,
        thumbnail_error: album.get_thumbnail_error.unwrap_or(false),
    }
}

fn thumbnail_image_request(image: &ImageFile) -> SspImageFile {
    SspImageFile {
        media_id: image.media_id,
        path: image.path.clone(),
        ..Default::default()
    }
}

fn thumbnail_video_request(video: &VideoFile) -> SspVideoFile {
    SspVideoFile {
        media_id: video.media_id,
        path: video.path.clone(),
        ..Default::default()
    }
}

fn thumbnail_audio_album_request(album: &AudioAlbum) -> SspAudioAlbum {
    SspAudioAlbum {
        album_id: album.album_id,
        album_path: album.path.clone(),
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

/// Recursively collect local files below `local` into upload items whose
/// targets mirror the tree under `remote`, recording directory targets.
/// Synchronous walk: the collected plan is small and uploads happen later.
fn collect_upload_tree(
    local: &Path,
    remote: &str,
    items: &mut Vec<BatchTransferItem>,
    directories: &mut std::collections::BTreeSet<String>,
) -> Result<()> {
    directories.insert(remote.trim_end_matches('/').to_string());
    let entries = std::fs::read_dir(local).map_err(|error| {
        Error::LocalIo(i18n::format(
            "client.read_dir_failed",
            &[&local.display().to_string(), &error.to_string()],
        ))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            Error::LocalIo(i18n::format(
                "client.read_dir_failed",
                &[&local.display().to_string(), &error.to_string()],
            ))
        })?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let remote_child = format!("{}/{}", remote.trim_end_matches('/'), name);
        let file_type = entry.file_type().map_err(|error| {
            Error::LocalIo(i18n::format(
                "client.read_dir_failed",
                &[&path.display().to_string(), &error.to_string()],
            ))
        })?;
        if file_type.is_dir() {
            collect_upload_tree(&path, &remote_child, items, directories)?;
        } else if file_type.is_file() {
            items.push(BatchTransferItem {
                source: path.display().to_string(),
                target: remote_child,
            });
        }
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

/// Bounded in-memory sink for `receive_raw_to`, used by `download_bytes`.
#[derive(Default)]
struct VecSink {
    bytes: Vec<u8>,
}

impl AsyncWrite for VecSink {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::task::Poll::Ready(Ok({
            self.get_mut().bytes.extend_from_slice(buf);
            buf.len()
        }))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
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
    use crate::events::{ClientEvent, EventKind};
    use crate::test_support::{
        DownloadBehavior, FakeSsp, FakeSspConfig, FakeWifiSsp, UploadBehavior,
    };

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
    async fn wifi_connect_persists_trust_and_reuses_business_api() {
        use crate::test_support::{FakeWifiSsp, WIFI_DEVICE_UUID};
        use base64::Engine as _;

        let fake = FakeWifiSsp::start().await;
        let client = fake.connect().await;

        // The echoed derived key must be persisted keyed by device_uuid.
        let state: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(fake.state_path()).expect("state file"))
                .expect("state json");
        let derived = &state["trust"][WIFI_DEVICE_UUID]["derived_key"];
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(derived.as_str().expect("derived key string"))
            .expect("derived key base64");
        assert_eq!(decoded, vec![0x42_u8; 256]);

        // Business API is reused unchanged over the WiFi channel.
        assert_eq!(
            client.device_info().name.as_deref(),
            Some("Wifi Test Phone")
        );
        assert_eq!(client.device_info().model.as_deref(), Some("OD103"));
        assert_eq!(client.root_path(), "/storage/emulated/0");
        assert_eq!(
            client.ping().await.expect("ping").client_timestamp,
            Some(42)
        );
        let files = client.list_dir(".", 1).await.expect("list");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "/storage/emulated/0/a.txt");

        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn wifi_trust_reset_verifies_device_uuid() {
        use crate::state::StateStore;
        use crate::test_support::{FakeWifiSsp, WIFI_DEVICE_UUID};

        fn options() -> ClientOptions {
            ClientOptions {
                timeout: Duration::from_secs(5),
                heartbeat_interval: Duration::from_secs(60),
                host_name: None,
                adb_path: PathBuf::from("adb"),
                wire_log: None,
                wire_log_payload: false,
            }
        }

        // Matching uuid: the phone-side record is cleared and QUIT is sent.
        let fake = FakeWifiSsp::start().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let store = StateStore::at(temp.path().join("state.json"));
        HandShakerClient::reset_wifi_trust_with_state_store(
            fake.address(),
            WIFI_DEVICE_UUID,
            options(),
            store,
        )
        .await
        .expect("trust reset");
        fake.finish().await;

        // Mismatched uuid: the reset must be rejected, not silently succeed.
        let fake = FakeWifiSsp::start().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let store = StateStore::at(temp.path().join("state.json"));
        let error = HandShakerClient::reset_wifi_trust_with_state_store(
            fake.address(),
            "wrong-device",
            options(),
            store,
        )
        .await
        .expect_err("uuid mismatch must fail");
        assert!(matches!(error, Error::Handshake(_)), "{error:?}");
        fake.finish().await;
    }

    #[tokio::test]
    async fn explicit_callbacks_route_typed_device_events() {
        let fake = FakeSsp::start(FakeSspConfig {
            send_device_event: true,
            ..Default::default()
        })
        .await;
        let client = fake
            .connect_with_callbacks(EventCallbacks {
                device_info: true,
                ..Default::default()
            })
            .await;
        let mut events = client.subscribe_events(EventFilter::only([EventKind::DeviceInfoChanged]));
        let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("device event timeout")
            .expect("device event");
        let ClientEvent::DeviceInfoChanged(info) = event else {
            panic!("device info event")
        };
        assert_eq!(info.name.as_deref(), Some("pushed device"));
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn client_request_token_cancels_with_flag_and_keeps_session_usable() {
        let fake = FakeSsp::start(FakeSspConfig {
            delay_heartbeat: true,
            ..Default::default()
        })
        .await;
        let client = fake.connect().await;
        let token = crate::cancellation::CancellationToken::new();
        let error = {
            let request = client.ping_with_options(crate::cancellation::RequestOptions {
                cancellation: Some(token.clone()),
            });
            tokio::pin!(request);
            tokio::select! {
                result = &mut request => result.expect_err("ping should be cancelled"),
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    token.cancel();
                    request
                        .as_mut()
                        .await
                        .expect_err("ping should be cancelled")
                }
            }
        };
        assert!(matches!(
            error,
            Error::Cancelled(crate::cancellation::CancellationInfo {
                origin: crate::cancellation::CancellationOrigin::Local { flag_sent: true },
                connection_closed: false,
                ..
            })
        ));
        assert!(client.ping().await.is_ok());
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn download_cancellation_closes_session_and_preserves_target() {
        let fake = FakeSsp::start(FakeSspConfig {
            download: DownloadBehavior::Slow,
            ..Default::default()
        })
        .await;
        let local = tempfile::tempdir().expect("local tempdir");
        let target = local.path().join("target.txt");
        std::fs::write(&target, b"keep").expect("target");
        let client = fake.connect().await;
        let token = crate::cancellation::CancellationToken::new();
        let error = {
            let request = client.download_with_options(
                "/storage/emulated/0/test.txt",
                &target,
                TransferOptions {
                    overwrite: true,
                    ..Default::default()
                },
                crate::cancellation::RequestOptions {
                    cancellation: Some(token.clone()),
                },
            );
            tokio::pin!(request);
            tokio::select! {
                result = &mut request => result.expect_err("download should be cancelled"),
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    token.cancel();
                    request
                        .as_mut()
                        .await
                        .expect_err("download should be cancelled")
                }
            }
        };
        assert!(matches!(
            error,
            Error::Cancelled(crate::cancellation::CancellationInfo {
                origin: crate::cancellation::CancellationOrigin::Local { .. },
                connection_closed: true,
                ..
            })
        ));
        assert_eq!(
            std::fs::read(&target).expect("target after cancel"),
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
        assert!(client.ping().await.is_err());
        client.close().await.expect("close");
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

    #[tokio::test]
    async fn wifi_monitor_folder_registers_and_unregisters() {
        let fake = FakeWifiSsp::start().await;
        let client = fake.connect().await;
        client
            .monitor_folder("/storage/emulated/0/watch", true)
            .await
            .expect("register monitor");
        client
            .monitor_folder("/storage/emulated/0/watch", false)
            .await
            .expect("unregister monitor");
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn wifi_monitor_folder_rejection_is_reported() {
        let fake = FakeWifiSsp::start_with_monitor_reject().await;
        let client = fake.connect().await;
        let error = client
            .monitor_folder("/storage/emulated/0/watch", true)
            .await
            .expect_err("monitor rejected");
        assert!(matches!(error, Error::RemoteIo { .. }));
        assert_eq!(error.exit_code(), 6);
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn wifi_media_libraries_decode_and_map_fields() {
        let fake = FakeWifiSsp::start().await;
        let client = fake.connect().await;

        let photo = client.get_photo_library().await.expect("photo library");
        assert_eq!(photo.images.len(), 1);
        let image = &photo.images[0];
        assert_eq!(
            image.path.as_deref(),
            Some("/storage/emulated/0/DCIM/a.jpg")
        );
        assert_eq!(image.width, Some(640));
        assert_eq!(image.orientation, Some(1));
        assert!(image.starred);
        assert!(!image.thumbnail_error);
        assert_eq!(photo.albums[0].name.as_deref(), Some("Camera"));
        assert_eq!(photo.camera_album_id, Some(100));

        let video = client.get_video_library().await.expect("video library");
        assert_eq!(video.videos.len(), 1);
        assert_eq!(video.videos[0].duration, Some(12.5));
        assert_eq!(video.albums[0].album_id, Some(200));

        let audio = client.get_audio_library().await.expect("audio library");
        assert_eq!(audio.tracks.len(), 1);
        assert_eq!(audio.tracks[0].artist.as_deref(), Some("Artist"));
        assert_eq!(
            audio.tracks[0].duration,
            Some(210.0),
            "audio duration is converted from milliseconds to seconds"
        );
        assert_eq!(audio.albums[0].year, Some(2020));

        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn wifi_thumbnails_carry_bytes_and_report_failed_entries() {
        let fake = FakeWifiSsp::start().await;
        let client = fake.connect().await;

        let requested = vec![
            ImageFile {
                media_id: Some(1),
                path: Some("/storage/emulated/0/DCIM/a.jpg".to_string()),
                ..Default::default()
            },
            ImageFile {
                media_id: Some(2),
                path: Some("/storage/emulated/0/DCIM/b.jpg".to_string()),
                ..Default::default()
            },
        ];
        let thumbnails = client
            .get_thumbnails(&requested, &[], &[])
            .await
            .expect("thumbnails");
        assert_eq!(thumbnails.images.len(), 2);
        assert_eq!(
            thumbnails.images[0].thumbnail.as_deref(),
            Some(&[0xFF, 0xD8, 0xFF, 0xE0][..]),
            "first thumbnail carries JPEG bytes"
        );
        assert!(!thumbnails.images[0].thumbnail_error);
        assert_eq!(
            thumbnails.images[1].thumbnail.as_ref().map(Vec::len),
            None,
            "failed entry omits thumbnail bytes"
        );
        assert!(thumbnails.images[1].thumbnail_error);
        assert!(thumbnails.videos.is_empty());
        assert!(thumbnails.audio_albums.is_empty());

        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn fetch_exif_pulls_and_parses_jpeg() {
        let fake = FakeWifiSsp::start().await;
        let client = fake.connect().await;
        let data = client
            .fetch_exif("/storage/emulated/0/DCIM/exif.jpg")
            .await
            .expect("exif fetch");
        assert_eq!(data.orientation, Some(6));
        assert_eq!(data.make.as_deref(), Some("Fixture"));
        assert_eq!(data.model.as_deref(), Some("TestCam"));
        assert_eq!(data.latitude.as_deref(), Some("1.034167"));
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn fetch_exif_rejects_non_jpeg_with_protocol_error() {
        let fake = FakeWifiSsp::start().await;
        let client = fake.connect().await;
        let error = client
            .fetch_exif("/storage/emulated/0/DCIM/a.jpg")
            .await
            .expect_err("not a jpeg");
        assert_eq!(error.code(), crate::error::ErrorCode::Protocol);
        assert_eq!(error.exit_code(), 5);
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn update_files_info_returns_true_on_phone_acceptance() {
        let fake = FakeWifiSsp::start().await;
        let client = fake.connect().await;
        let result = client
            .update_files_info(
                &[RemoteFile {
                    path: "/storage/emulated/0/DCIM/star.jpg".to_string(),
                    size: 1024,
                    created_at: Some(1),
                    modified_at: Some(2),
                    is_directory: false,
                    checksum: Some("abcd".to_string()),
                    is_trash: Some(false),
                    id: Some(42),
                    ext_data: None,
                }],
                false,
            )
            .await
            .expect("update accepted");
        assert!(result);
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn update_files_info_reports_phone_rejection_as_protocol_error() {
        let fake = FakeWifiSsp::start().await;
        let client = fake.connect().await;
        let error = client
            .update_files_info(
                &[RemoteFile {
                    path: "/reject.txt".to_string(),
                    size: 0,
                    created_at: None,
                    modified_at: None,
                    is_directory: false,
                    checksum: None,
                    is_trash: None,
                    id: None,
                    ext_data: None,
                }],
                true,
            )
            .await
            .expect_err("rejected path");
        assert_eq!(error.code(), crate::error::ErrorCode::Protocol);
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn photo_sync_sends_snapshot_and_returns_phone_state() {
        let fake = FakeWifiSsp::start().await;
        let client = fake.connect().await;
        let result = client
            .photo_sync(
                "hs-abc",
                &[RemoteFile {
                    path: "/storage/emulated/0/DCIM/gone.jpg".to_string(),
                    size: 512,
                    created_at: None,
                    modified_at: Some(1),
                    is_directory: false,
                    checksum: Some("old".to_string()),
                    is_trash: None,
                    id: Some(9),
                    ext_data: None,
                }],
            )
            .await
            .expect("photo sync");
        // Real device omits is_success on success; the client must treat
        // None as acceptance and let the caller inspect the explicit flag.
        assert_eq!(result.is_success, None);
        // The fake echoes the snapshot minus the deleted file and adds a.jpg.
        let paths: Vec<&str> = result.files.iter().map(|file| file.path.as_str()).collect();
        assert!(!paths.contains(&"/storage/emulated/0/DCIM/gone.jpg"));
        assert!(paths.contains(&"/storage/emulated/0/DCIM/a.jpg"));
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn sync_monitor_enable_receives_file_change_push() {
        let fake = FakeWifiSsp::start().await;
        let client = fake.connect().await;
        let mut events = client.subscribe_events(EventFilter::all());
        let accepted = client.sync_monitor(true).await.expect("enable monitor");
        assert!(accepted);
        // The fake pushes one FILE_CHANGE(38) after the monitor is enabled.
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), events.recv())
            .await
            .expect("push timeout")
            .expect("push event");
        let crate::events::ClientEvent::FileChanged(changes) = event else {
            panic!("expected FileChanged push, got {event:?}");
        };
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[0].file.as_ref().map(|f| f.path.as_str()),
            Some("/storage/emulated/0/DCIM/live.jpg")
        );
        assert_eq!(changes[0].status, crate::events::FileChangeStatus::Added);
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn batch_transfer_with_concurrency_two_keeps_results_and_failures() {
        let temp = tempfile::tempdir().expect("tempdir");
        let local_a = temp.path().join("a.bin");
        let local_b = temp.path().join("b.bin");
        let local_c = temp.path().join("c.bin");

        let fake = FakeWifiSsp::start().await;
        let client = fake.connect().await;
        // The fake device serves b"download-data" for any path; three items
        // download concurrently with concurrency 2 and all must surface.
        let result = client
            .download_many(
                &[
                    BatchTransferItem {
                        source: "/storage/emulated/0/a.txt".to_string(),
                        target: local_a.display().to_string(),
                    },
                    BatchTransferItem {
                        source: "/storage/emulated/0/a.txt".to_string(),
                        target: local_b.display().to_string(),
                    },
                    BatchTransferItem {
                        source: "/storage/emulated/0/a.txt".to_string(),
                        target: local_c.display().to_string(),
                    },
                ],
                BatchTransferOptions {
                    overwrite: true,
                    progress: None,
                    offset: 0,
                    concurrency: 2,
                    cancel: None,
                },
            )
            .await
            .expect("concurrent batch");
        assert_eq!(result.ok.len(), 3, "three downloads succeed");
        assert_eq!(result.failures.len(), 0);
        for local in [&local_a, &local_b, &local_c] {
            let content = tokio::fs::read(local).await.expect("read target");
            assert_eq!(content, b"download-data");
        }
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn batch_transfer_rejects_out_of_range_concurrency() {
        let fake = FakeWifiSsp::start().await;
        let client = fake.connect().await;
        let error = client
            .upload_many(
                &[],
                BatchTransferOptions {
                    overwrite: false,
                    progress: None,
                    offset: 0,
                    concurrency: 0,
                    cancel: None,
                },
            )
            .await
            .expect_err("concurrency 0 rejected");
        assert_eq!(error.code(), crate::error::ErrorCode::Usage);

        let error = client
            .upload_many(
                &[],
                BatchTransferOptions {
                    overwrite: false,
                    progress: None,
                    offset: 0,
                    concurrency: 9,
                    cancel: None,
                },
            )
            .await
            .expect_err("concurrency 9 rejected");
        assert_eq!(error.code(), crate::error::ErrorCode::Usage);
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn batch_transfer_stops_when_cancellation_is_requested() {
        let temp = tempfile::tempdir().expect("tempdir");
        let local_a = temp.path().join("a.bin");
        let local_b = temp.path().join("b.bin");

        let fake = FakeWifiSsp::start().await;
        let client = fake.connect().await;
        // Cancelling before the batch starts must skip every item and
        // surface Error::Interrupted (not an aggregated failure list).
        let token = crate::cancellation::CancellationToken::new();
        token.cancel();
        let error = client
            .download_many(
                &[
                    BatchTransferItem {
                        source: "/storage/emulated/0/a.txt".to_string(),
                        target: local_a.display().to_string(),
                    },
                    BatchTransferItem {
                        source: "/storage/emulated/0/a.txt".to_string(),
                        target: local_b.display().to_string(),
                    },
                ],
                BatchTransferOptions {
                    overwrite: false,
                    progress: None,
                    offset: 0,
                    concurrency: 1,
                    cancel: Some(token.clone()),
                },
            )
            .await
            .expect_err("cancelled batch must interrupt");
        assert_eq!(error.code(), crate::error::ErrorCode::Interrupted);
        // No file may land after the cancellation.
        assert!(!local_a.exists());
        assert!(!local_b.exists());
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn batch_transfer_stops_mid_batch_after_first_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let local_a = temp.path().join("a.bin");
        let local_b = temp.path().join("b.bin");

        let fake = FakeWifiSsp::start().await;
        let client = fake.connect().await;
        // Mid-batch cancellation (Phase D review follow-up): with concurrency
        // 1 the progress callback runs after the first file completes and
        // before the second file starts, so cancelling from the callback
        // exercises the "skip not-yet-started files" path (the pre-cancelled
        // test above only covers cancellation before the batch starts).
        let token = crate::cancellation::CancellationToken::new();
        let cancel_token = token.clone();
        let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let done_in_callback = done.clone();
        let error = client
            .download_many(
                &[
                    BatchTransferItem {
                        source: "/storage/emulated/0/a.txt".to_string(),
                        target: local_a.display().to_string(),
                    },
                    BatchTransferItem {
                        source: "/storage/emulated/0/a.txt".to_string(),
                        target: local_b.display().to_string(),
                    },
                ],
                BatchTransferOptions {
                    overwrite: false,
                    progress: Some(Arc::new(move |progress| {
                        done_in_callback.store(progress.done, std::sync::atomic::Ordering::Relaxed);
                        cancel_token.cancel();
                    })),
                    offset: 0,
                    concurrency: 1,
                    cancel: Some(token.clone()),
                },
            )
            .await
            .expect_err("mid-batch cancellation must interrupt");
        assert_eq!(error.code(), crate::error::ErrorCode::Interrupted);
        assert_eq!(
            done.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "callback ran after the first file completed"
        );
        // The first file finished before the cancellation; the second file
        // never started and must not exist.
        let content = tokio::fs::read(&local_a).await.expect("read target");
        assert_eq!(content, b"download-data");
        assert!(!local_b.exists());
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn upload_many_aggregates_per_file_results() {
        let temp = tempfile::tempdir().expect("tempdir");
        let first = temp.path().join("one.bin");
        let second = temp.path().join("two.bin");
        tokio::fs::write(&first, b"one").await.expect("write");
        tokio::fs::write(&second, b"two").await.expect("write");

        let fake = FakeWifiSsp::start().await;
        let client = fake.connect().await;
        let result = client
            .upload_many(
                &[
                    BatchTransferItem {
                        source: first.display().to_string(),
                        target: "/remote/one.bin".to_string(),
                    },
                    BatchTransferItem {
                        source: second.display().to_string(),
                        target: "/remote/two.bin".to_string(),
                    },
                ],
                BatchTransferOptions {
                    overwrite: false,
                    progress: None,
                    offset: 0,
                    concurrency: 1,
                    cancel: None,
                },
            )
            .await
            .expect("batch upload");
        assert_eq!(result.ok.len(), 2);
        assert!(result.failures.is_empty());
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn download_with_offset_streams_from_the_seek_position() {
        let temp = tempfile::tempdir().expect("tempdir");
        let local = temp.path().join("tail.bin");

        let fake = FakeWifiSsp::start().await;
        let client = fake.connect().await;
        client
            .download_with_options(
                "/storage/emulated/0/a.txt",
                &local,
                TransferOptions {
                    overwrite: true,
                    progress: None,
                    offset: 5,
                },
                RequestOptions::default(),
            )
            .await
            .expect("range download");
        // Fake serves b"download-data" (13 bytes); offset 5 yields the tail.
        let content = tokio::fs::read(&local).await.expect("read target");
        assert_eq!(content, b"oad-data");
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn download_offset_beyond_eof_yields_an_empty_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let local = temp.path().join("empty.bin");

        let fake = FakeWifiSsp::start().await;
        let client = fake.connect().await;
        client
            .download_with_options(
                "/storage/emulated/0/a.txt",
                &local,
                TransferOptions {
                    overwrite: true,
                    progress: None,
                    offset: 1000,
                },
                RequestOptions::default(),
            )
            .await
            .expect("range download past EOF");
        let content = tokio::fs::read(&local).await.expect("read target");
        assert!(content.is_empty());
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn download_many_with_offset_round_trips_through_the_batch() {
        let temp = tempfile::tempdir().expect("tempdir");
        let local = temp.path().join("batch-tail.bin");

        let fake = FakeWifiSsp::start().await;
        let client = fake.connect().await;
        let result = client
            .download_many(
                &[BatchTransferItem {
                    source: "/storage/emulated/0/a.txt".to_string(),
                    target: local.display().to_string(),
                }],
                BatchTransferOptions {
                    overwrite: true,
                    progress: None,
                    offset: 9,
                    concurrency: 1,
                    cancel: None,
                },
            )
            .await
            .expect("batch range download");
        assert_eq!(result.ok.len(), 1);
        let content = tokio::fs::read(&local).await.expect("read target");
        assert_eq!(content, b"data", "last 4 bytes of download-data");
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn download_many_writes_all_files_and_reports_failures() {
        let temp = tempfile::tempdir().expect("tempdir");
        let local_a = temp.path().join("a.bin");
        let local_b = temp.path().join("b.bin");

        let fake = FakeWifiSsp::start().await;
        let client = fake.connect().await;
        // /storage/emulated/0/missing.bin is not served by the fake device
        // (only exif.jpg and the default download-data path), so the first
        // item succeeds with download-data and the second one must be
        // reported; the batch continues.
        let result = client
            .download_many(
                &[
                    BatchTransferItem {
                        source: "/storage/emulated/0/a.txt".to_string(),
                        target: local_a.display().to_string(),
                    },
                    BatchTransferItem {
                        source: "/storage/emulated/0/a.txt".to_string(),
                        target: local_b.display().to_string(),
                    },
                ],
                BatchTransferOptions {
                    overwrite: true,
                    progress: None,
                    offset: 0,
                    concurrency: 1,
                    cancel: None,
                },
            )
            .await
            .expect("batch download");
        assert_eq!(result.ok.len(), 2);
        assert!(result.failures.is_empty());
        let content = tokio::fs::read(&local_a).await.expect("read a");
        assert_eq!(content, b"download-data");
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn upload_tree_mirrors_local_directory_structure() {
        let temp = tempfile::tempdir().expect("tempdir");
        let tree = temp.path().join("tree");
        std::fs::create_dir_all(tree.join("sub")).expect("mkdir sub");
        std::fs::write(tree.join("root.txt"), b"root").expect("write");
        std::fs::write(tree.join("sub").join("leaf.txt"), b"leaf").expect("write");

        let fake = FakeWifiSsp::start().await;
        let client = fake.connect().await;
        let result = client
            .upload_tree(
                &tree,
                "/storage/emulated/0/hs_m5_test",
                BatchTransferOptions {
                    overwrite: false,
                    progress: None,
                    offset: 0,
                    concurrency: 1,
                    cancel: None,
                },
            )
            .await
            .expect("upload tree");
        assert_eq!(result.ok.len(), 2, "both files uploaded");
        assert!(result.failures.is_empty());
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn download_tree_mirrors_remote_directory_structure() {
        let temp = tempfile::tempdir().expect("tempdir");
        let local_root = temp.path().join("out");

        let fake = FakeWifiSsp::start().await;
        let client = fake.connect().await;
        let result = client
            .download_tree(
                "/storage/emulated/0/hs_m5_test",
                &local_root,
                BatchTransferOptions {
                    overwrite: true,
                    progress: None,
                    offset: 0,
                    concurrency: 1,
                    cancel: None,
                },
            )
            .await
            .expect("download tree");
        assert_eq!(result.ok.len(), 1, "fake device lists one file");
        let content = tokio::fs::read(local_root.join("a.txt"))
            .await
            .expect("downloaded file");
        assert_eq!(content, b"download-data");
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn download_tree_rejects_paths_escaping_the_requested_subtree() {
        let temp = tempfile::tempdir().expect("tempdir");
        let local_root = temp.path().join("out");

        let fake = FakeWifiSsp::start_with_escape_path().await;
        let client = fake.connect().await;
        let error = client
            .download_tree(
                "/storage/emulated/0/hs_m5_test",
                &local_root,
                BatchTransferOptions {
                    overwrite: true,
                    progress: None,
                    offset: 0,
                    concurrency: 1,
                    cancel: None,
                },
            )
            .await
            .expect_err("escape must be rejected");
        assert_eq!(error.code(), crate::error::ErrorCode::Protocol);
        // The root directory is created up front, but no hostile file may be
        // written into it.
        let mut entries = tokio::fs::read_dir(&local_root)
            .await
            .expect("read out dir");
        assert!(
            entries.next_entry().await.expect("first entry").is_none(),
            "no files may be written by an escaping listing"
        );
        client.close().await.expect("close");
        fake.finish().await;
    }
}
