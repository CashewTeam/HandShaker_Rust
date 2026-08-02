#![cfg(test)]

use std::fs;
use std::io::Write;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use aes::Aes256;
use base64::Engine;
use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use flate2::Compression;
use flate2::write::GzEncoder;
use md5::{Digest, Md5};
use prost::Message;
use rand::rngs::OsRng;
use rsa::pkcs1::DecodeRsaPublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::client::{ClientOptions, ConnectionTarget, EventCallbacks, HandShakerClient};
use crate::protocol::crypto::KEY_TABLE;
use crate::protocol::proto::*;

type Aes256CbcDecryptor = cbc::Decryptor<Aes256>;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) enum DownloadBehavior {
    #[default]
    Success,
    Md5Mismatch,
    Short,
    Slow,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) enum UploadBehavior {
    #[default]
    Success,
    NotReady,
    Failed,
    Canceled,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FakeSspConfig {
    pub(crate) minimal_device_info: bool,
    pub(crate) send_device_event: bool,
    pub(crate) delay_heartbeat: bool,
    pub(crate) partial_delete: bool,
    pub(crate) download: DownloadBehavior,
    pub(crate) upload: UploadBehavior,
    pub(crate) create_error: bool,
}

pub(crate) struct FakeSsp {
    temp: TempDir,
    adb_path: PathBuf,
    state_path: PathBuf,
    server: JoinHandle<()>,
}

impl FakeSsp {
    pub(crate) async fn start(config: FakeSspConfig) -> Self {
        let temp = tempfile::tempdir().expect("fake SSP tempdir");
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("fake SSP listener");
        let port = listener.local_addr().expect("fake SSP address").port();
        let port_path = temp.path().join("port");
        fs::write(&port_path, port.to_string()).expect("fake SSP port file");
        let active_path = temp.path().join("active-forward");
        let calls_path = temp.path().join("adb-calls");
        fs::write(&calls_path, "").expect("fake adb calls");
        let adb_path = temp.path().join("adb");
        write_fake_adb(
            &adb_path,
            temp.path(),
            &port_path,
            &active_path,
            &calls_path,
        );
        let state_path = temp.path().join("config").join("state.json");
        let server = tokio::spawn(run_server(listener, config));
        Self {
            temp,
            adb_path,
            state_path,
            server,
        }
    }

    pub(crate) async fn connect(&self) -> HandShakerClient {
        self.connect_with_callbacks(EventCallbacks::default()).await
    }

    pub(crate) async fn connect_with_callbacks(
        &self,
        callbacks: EventCallbacks,
    ) -> HandShakerClient {
        HandShakerClient::connect_for_test_with_callbacks(
            ConnectionTarget::Adb {
                serial: Some("FAKE123".to_string()),
            },
            ClientOptions {
                timeout: Duration::from_secs(3),
                heartbeat_interval: Duration::from_secs(60),
                adb_path: self.adb_path.clone(),
                wire_log: None,
            },
            self.state_path.clone(),
            callbacks,
        )
        .await
        .expect("fake SSP client connection")
    }

    pub(crate) fn adb_calls(&self) -> String {
        fs::read_to_string(self.temp.path().join("adb-calls")).expect("fake adb calls")
    }

    pub(crate) fn forward_is_clean(&self) -> bool {
        !self.temp.path().join("active-forward").exists()
    }

    pub(crate) async fn finish(self) {
        timeout(Duration::from_secs(3), self.server)
            .await
            .expect("fake SSP server should finish")
            .expect("fake SSP server task");
    }
}

/// Fake phone speaking the WiFi two-round handshake, for client integration
/// tests over a direct TCP connection (no adb).
pub(crate) struct FakeWifiSsp {
    #[allow(dead_code)] // keeps the temporary config directory alive for cleanup
    temp: TempDir,
    address: SocketAddr,
    state_path: PathBuf,
    server: JoinHandle<()>,
}

impl FakeWifiSsp {
    pub(crate) async fn start() -> Self {
        let temp = tempfile::tempdir().expect("fake wifi SSP tempdir");
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("fake wifi SSP listener");
        let address = listener.local_addr().expect("fake wifi SSP address");
        let state_path = temp.path().join("config").join("state.json");
        let server = tokio::spawn(run_server_wifi(listener));
        Self {
            temp,
            address,
            state_path,
            server,
        }
    }

    pub(crate) async fn connect(&self) -> HandShakerClient {
        HandShakerClient::connect_for_test_with_callbacks(
            ConnectionTarget::Wifi {
                address: self.address,
            },
            ClientOptions {
                timeout: Duration::from_secs(5),
                heartbeat_interval: Duration::from_secs(60),
                adb_path: PathBuf::from("adb"),
                wire_log: None,
            },
            self.state_path.clone(),
            EventCallbacks::default(),
        )
        .await
        .expect("fake wifi SSP client connection")
    }

    pub(crate) fn state_path(&self) -> &Path {
        &self.state_path
    }

    pub(crate) fn address(&self) -> SocketAddr {
        self.address
    }

    pub(crate) async fn finish(self) {
        timeout(Duration::from_secs(3), self.server)
            .await
            .expect("fake wifi SSP server should finish")
            .expect("fake wifi SSP server task");
    }
}

async fn run_server_wifi(listener: TcpListener) {
    let (mut stream, _) = listener.accept().await.expect("fake wifi SSP accept");

    // REQUEST_01 -> RESPONSE_01
    let (sid, flag, payload) = read_upstream(&mut stream).await;
    assert_eq!(sid, 0x8000_0001);
    assert_eq!(flag, 0);
    let request01 = SspHandShakeRequest01::decode(payload.as_slice()).expect("fake request01");
    let public = decode_host_key_from_request01(&request01);
    let response01 = SspHandShakeResponse01 {
        r#type: Some(SspRequestType::HandshakeResponse01 as i32),
        apk_version: Some("201".to_string()),
        apk_version_name: Some("1.2.0".to_string()),
        client_smart_sync_protocol_version: Some("1".to_string()),
        client_min_host_version: Some("2.1.0".to_string()),
        device_uuid: Some(WIFI_DEVICE_UUID.to_string()),
        device_name: Some("Wifi Test Phone".to_string()),
        usb_serial: Some("WIFI-SN".to_string()),
        is_smartisan_device: Some(true),
        client_min_host_version_code: Some(333),
        ..Default::default()
    };
    write_normal(&mut stream, sid, &response01.encode_to_vec()).await;

    // REQUEST_02 -> RESPONSE_02 (first connect: TRUST_ALWAYS with derived key;
    // trust reset: TRUST_WAITING after the phone clears its record).
    let (sid2, flag2, payload2) = read_upstream(&mut stream).await;
    assert_eq!(flag2, 0);
    let request02 = SspHandShakeRequest02::decode(payload2.as_slice()).expect("fake request02");
    if request02.trust_type == Some(SspHandShakeTrustType::TrustRemove as i32) {
        let response02 = SspHandShakeResponse02 {
            r#type: Some(SspRequestType::HandshakeResponse02 as i32),
            trust_type: Some(SspHandShakeTrustType::TrustWaiting as i32),
            device_uuid: Some(WIFI_DEVICE_UUID.to_string()),
            device_name: Some("Wifi Test Phone".to_string()),
            derived_key: None,
            result: None,
        };
        write_normal(&mut stream, sid2, &response02.encode_to_vec()).await;
    } else {
        assert!(
            request02.derived_key.is_none(),
            "first connect must not present a derived key"
        );
        let derived_key = vec![0x42_u8; 256];
        let encrypted = public
            .encrypt(&mut OsRng, Pkcs1v15Encrypt, b"ok")
            .expect("fake wifi handshake encryption");
        let encoded = base64::engine::general_purpose::STANDARD.encode(encrypted);
        let response02 = SspHandShakeResponse02 {
            r#type: Some(SspRequestType::HandshakeResponse02 as i32),
            trust_type: Some(SspHandShakeTrustType::TrustAlways as i32),
            device_uuid: Some(WIFI_DEVICE_UUID.to_string()),
            device_name: Some("Wifi Test Phone".to_string()),
            derived_key: Some(derived_key),
            result: Some(encoded),
        };
        write_normal(&mut stream, sid2, &response02.encode_to_vec()).await;
    }

    // Business phase: signed requests, same framing as ADB.
    loop {
        let (sid, flag, body) = match read_upstream_result(&mut stream).await {
            Ok(value) => value,
            Err(_) => break,
        };
        if flag != 1 {
            continue;
        }
        if body.len() < 128 {
            break;
        }
        let (signature, protobuf) = body.split_at(128);
        public
            .verify(
                rsa::Pkcs1v15Sign::new::<sha2::Sha256>(),
                &sha2::Sha256::digest(protobuf),
                signature,
            )
            .expect("fake wifi SSP signature");
        let request = SspRequest::decode(protobuf).expect("fake wifi SSP request type");
        let Some(request_type) = request
            .r#type
            .and_then(|value| SspRequestType::try_from(value).ok())
        else {
            continue;
        };
        match request_type {
            SspRequestType::GetDeviceInfoRequest => {
                let response = SspGetDeviceInfoResponse {
                    r#type: None,
                    apk_version: Some("1.2.0".to_string()),
                    apk_version_name: Some("1.2.0-r6".to_string()),
                    phone_model: Some("OD103".to_string()),
                    phone_name: Some("Wifi Test Phone".to_string()),
                    root_path: Some("/storage/emulated/0".to_string()),
                    product_brand: Some("Smartisan".to_string()),
                    phone_id: Some(WIFI_DEVICE_UUID.to_string()),
                    disk_size: Some(1000),
                    used_disk_size: Some(100),
                    battery_percentage: Some(66),
                    phone_locked: Some(false),
                    ..Default::default()
                };
                write_normal(&mut stream, sid, &response.encode_to_vec()).await;
            }
            SspRequestType::HeartBeatRequest => {
                let request = SspHeartBeatRequest::decode(protobuf).expect("fake wifi heartbeat");
                let response = SspHeartBeatResponse {
                    r#type: None,
                    host_timestamp: request.host_timestamp,
                    client_timestamp: Some(42),
                };
                write_normal(&mut stream, sid, &response.encode_to_vec()).await;
            }
            SspRequestType::GetDirFilesRequest => {
                let response = SspGetDirFilesResponse {
                    r#type: None,
                    file: vec![SspFile {
                        path: Some("/storage/emulated/0/a.txt".to_string()),
                        file_size: Some(3),
                        ..Default::default()
                    }],
                    timecost: Some(1),
                    ..Default::default()
                };
                write_normal(&mut stream, sid, &response.encode_to_vec()).await;
            }
            SspRequestType::QuitRequest => break,
            _ => continue,
        }
    }
}

fn decode_host_key_from_request01(request: &SspHandShakeRequest01) -> RsaPublicKey {
    let encrypted = request.enckey.as_deref().expect("fake request01 enckey");
    let clear = Aes256CbcDecryptor::new((&KEY_TABLE[16..48]).into(), (&KEY_TABLE[..16]).into())
        .decrypt_padded_vec_mut::<Pkcs7>(encrypted)
        .expect("fake wifi AES public key");
    let public_der = base64::engine::general_purpose::STANDARD
        .decode(clear)
        .expect("fake wifi base64 public key");
    assert_eq!(
        request.md5.as_deref().expect("fake request01 md5"),
        &Md5::digest(&public_der)[..]
    );
    RsaPublicKey::from_pkcs1_der(&public_der).expect("fake wifi RSA public key")
}

pub(crate) const WIFI_DEVICE_UUID: &str = "wifi-device-1";

fn write_fake_adb(
    path: &Path,
    root: &Path,
    port_path: &Path,
    active_path: &Path,
    calls_path: &Path,
) {
    let script = format!(
        "#!/bin/sh\nset -eu\nROOT={}\nPORT={}\nACTIVE={}\nCALLS={}\nprintf '%s\\n' \"$*\" >> \"$CALLS\"\ncase \"$*\" in\n  *'devices -l') printf 'List of devices attached\\nFAKE123 device product:fake model:U2_Pro device:U2_Pro\\n' ;;\n  *'forward --list') if [ -f \"$ACTIVE\" ]; then printf 'FAKE123 tcp:%s tcp:10086\\n' \"$(cat \"$PORT\")\"; fi ;;\n  *'forward tcp:0 tcp:10086') : > \"$ACTIVE\"; cat \"$PORT\" ;;\n  *'forward --remove tcp:'*) rm -f \"$ACTIVE\" ;;\n  *) ;;\nesac\n",
        shell_quote(root),
        shell_quote(port_path),
        shell_quote(active_path),
        shell_quote(calls_path),
    );
    fs::write(path, script).expect("fake adb script");
    let mut permissions = fs::metadata(path).expect("fake adb metadata").permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions).expect("fake adb permissions");
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

async fn run_server(listener: TcpListener, config: FakeSspConfig) {
    let (mut stream, _) = listener.accept().await.expect("fake SSP accept");
    let (sid, flag, enckey) = read_upstream(&mut stream).await;
    assert_eq!(sid, 0x8000_0001);
    assert_eq!(flag, 0);
    let public = decode_host_key(&enckey);
    let encrypted = public
        .encrypt(&mut OsRng, Pkcs1v15Encrypt, b"ok")
        .expect("fake handshake encryption");
    let encoded = base64::engine::general_purpose::STANDARD.encode(encrypted);
    write_normal(&mut stream, sid, encoded.as_bytes()).await;

    let mut upload: Option<(u32, u64, Vec<u8>)> = None;
    loop {
        let (sid, flag, body) = match read_upstream_result(&mut stream).await {
            Ok(value) => value,
            Err(_) => break,
        };
        if flag == 3 {
            let Some((upload_sid, expected, mut data)) = upload.take() else {
                continue;
            };
            data.extend_from_slice(&body);
            if data.len() < expected as usize {
                upload = Some((upload_sid, expected, data));
                continue;
            }
            let response = match config.upload {
                UploadBehavior::Success => SspUploadFileResponse {
                    r#type: None,
                    file: None,
                    canceled: Some(false),
                    succeed: Some(true),
                    error_code: None,
                },
                UploadBehavior::Failed => SspUploadFileResponse {
                    r#type: None,
                    file: None,
                    canceled: Some(false),
                    succeed: Some(false),
                    error_code: Some(SspFileIoError::FileIoPermissionError as i32),
                },
                UploadBehavior::Canceled => SspUploadFileResponse {
                    r#type: None,
                    file: None,
                    canceled: Some(true),
                    succeed: Some(false),
                    error_code: Some(SspFileIoError::FileIoCancelAction as i32),
                },
                UploadBehavior::NotReady => unreachable!("not-ready upload has no data"),
            };
            write_normal(&mut stream, upload_sid, &response.encode_to_vec()).await;
            continue;
        }
        if flag == 2 {
            continue;
        }
        if body.len() < 128 {
            break;
        }
        let protobuf = &body[128..];
        let request = SspRequest::decode(protobuf).expect("fake SSP request type");
        let Some(request_type) = request
            .r#type
            .and_then(|value| SspRequestType::try_from(value).ok())
        else {
            continue;
        };
        match request_type {
            SspRequestType::GetDeviceInfoRequest => {
                let request =
                    SspGetDeviceInfoRequest::decode(protobuf).expect("device info request");
                let response = if config.minimal_device_info {
                    SspGetDeviceInfoResponse {
                        r#type: None,
                        ..Default::default()
                    }
                } else {
                    SspGetDeviceInfoResponse {
                        r#type: None,
                        apk_version: Some("1.2.0".to_string()),
                        apk_version_name: Some("1.2.0-r6".to_string()),
                        phone_model: Some("U2 Pro".to_string()),
                        phone_name: Some("M0 Test Phone".to_string()),
                        root_path: Some("/storage/emulated/0".to_string()),
                        product_brand: Some("Smartisan".to_string()),
                        product_manufacturer: Some("Smartisan".to_string()),
                        phone_id: Some("fake-phone".to_string()),
                        disk_size: Some(1000),
                        used_disk_size: Some(100),
                        battery_percentage: Some(88),
                        phone_locked: Some(false),
                        ..Default::default()
                    }
                };
                write_normal(&mut stream, sid, &response.encode_to_vec()).await;
                if config.send_device_event && request.need_device_info_callback == Some(true) {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    let event = SspGetDeviceInfoResponse {
                        r#type: Some(SspRequestType::GetDeviceInfoRequest as i32),
                        phone_name: Some("pushed device".to_string()),
                        ..Default::default()
                    };
                    write_normal(&mut stream, 0x9000_0001, &event.encode_to_vec()).await;
                }
            }
            SspRequestType::HeartBeatRequest => {
                let request = SspHeartBeatRequest::decode(protobuf).expect("heartbeat request");
                if config.delay_heartbeat {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
                let response = SspHeartBeatResponse {
                    r#type: None,
                    host_timestamp: request.host_timestamp,
                    client_timestamp: Some(42),
                };
                write_normal(&mut stream, sid, &response.encode_to_vec()).await;
            }
            SspRequestType::GetDirFilesRequest => {
                let request = SspGetDirFilesRequest::decode(protobuf).expect("list request");
                let path = request
                    .dir
                    .and_then(|file| file.path)
                    .unwrap_or_else(|| "/storage/emulated/0".to_string());
                let response = SspGetDirFilesResponse {
                    r#type: None,
                    dir: Some(SspFile {
                        path: Some(path.clone()),
                        is_directory: Some(true),
                        ..Default::default()
                    }),
                    maxdepth: request.maxdepth,
                    timecost: Some(1),
                    file: vec![SspFile {
                        path: Some(format!("{path}/test.txt")),
                        file_size: Some(12),
                        is_directory: Some(false),
                        ..Default::default()
                    }],
                };
                write_normal(&mut stream, sid, &response.encode_to_vec()).await;
            }
            SspRequestType::GetFileCountRequest => {
                let response = SspGetFileCountResponse {
                    r#type: None,
                    count: Some(1),
                    ..Default::default()
                };
                write_normal(&mut stream, sid, &response.encode_to_vec()).await;
            }
            SspRequestType::GetFileExistRequest => {
                let response = SspFileExistResponse {
                    r#type: None,
                    exist: Some(true),
                    ..Default::default()
                };
                write_normal(&mut stream, sid, &response.encode_to_vec()).await;
            }
            SspRequestType::GetCreateFolderRequest => {
                let request = SspCreateFolderRequest::decode(protobuf).expect("mkdir request");
                let path = request.file.and_then(|file| file.path);
                let response = SspCreateFolderResponse {
                    r#type: None,
                    file: path.map(|path| SspFile {
                        path: Some(path),
                        is_directory: Some(true),
                        ..Default::default()
                    }),
                    succeed: Some(!config.create_error),
                    error_code: config
                        .create_error
                        .then_some(SspFileIoError::FileIoPermissionError as i32),
                    error_message: config.create_error.then(|| "permission denied".to_string()),
                };
                write_normal(&mut stream, sid, &response.encode_to_vec()).await;
            }
            SspRequestType::GetRenameFileRequest => {
                let response = SspRenameFileResponse {
                    r#type: None,
                    succeed: Some(true),
                    ..Default::default()
                };
                write_normal(&mut stream, sid, &response.encode_to_vec()).await;
            }
            SspRequestType::GetDeleteFileRequest => {
                let request = SspDeleteFileRequest::decode(protobuf).expect("delete request");
                let file = request
                    .file
                    .into_iter()
                    .enumerate()
                    .map(|(index, file)| SspFile {
                        path: file.path,
                        succeed: Some(!(config.partial_delete && index > 0)),
                        error_code: (config.partial_delete && index > 0)
                            .then_some(SspFileIoError::FileIoPermissionError as i32),
                        ..Default::default()
                    })
                    .collect();
                let response = SspDeleteFileResponse {
                    r#type: None,
                    file,
                    succeed: Some(true),
                    ..Default::default()
                };
                write_normal(&mut stream, sid, &response.encode_to_vec()).await;
            }
            SspRequestType::GetDownloadFileRequest => {
                let data = b"download-data".to_vec();
                let checksum = match config.download {
                    DownloadBehavior::Md5Mismatch => "00000000000000000000000000000000".to_string(),
                    _ => format!("{:x}", Md5::digest(&data)),
                };
                let response = SspDownloadFileResponseHeader {
                    r#type: None,
                    range: Some(SspDataRange {
                        offset: Some(0),
                        length: Some(data.len() as u64),
                    }),
                    data_md5: Some(checksum),
                    ready: Some(true),
                    ..Default::default()
                };
                write_normal(&mut stream, sid, &response.encode_to_vec()).await;
                if matches!(config.download, DownloadBehavior::Short) {
                    write_raw(&mut stream, sid, &data[..data.len() - 2]).await;
                    return;
                }
                if matches!(config.download, DownloadBehavior::Slow) {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                write_raw(&mut stream, sid, &data).await;
            }
            SspRequestType::GetUploadFileRequestHeader => {
                let request = SspUploadFileRequest::decode(protobuf).expect("upload request");
                let size = request.file.and_then(|file| file.file_size).unwrap_or(0);
                if matches!(config.upload, UploadBehavior::NotReady) {
                    let response = SspUploadFileResponseHeader {
                        r#type: None,
                        ready: Some(false),
                        error_code: Some(SspFileIoError::FileIoPermissionError as i32),
                        ..Default::default()
                    };
                    write_normal(&mut stream, sid, &response.encode_to_vec()).await;
                } else if size == 0 {
                    let response = SspUploadFileResponse {
                        r#type: None,
                        succeed: Some(true),
                        canceled: Some(false),
                        ..Default::default()
                    };
                    write_normal(&mut stream, sid, &response.encode_to_vec()).await;
                } else {
                    let response = SspUploadFileResponseHeader {
                        r#type: None,
                        ready: Some(true),
                        ..Default::default()
                    };
                    write_normal(&mut stream, sid, &response.encode_to_vec()).await;
                    upload = Some((sid, size, Vec::new()));
                }
            }
            SspRequestType::GetClipboardRequest => {
                let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
                encoder
                    .write_all(b"sample clipboard")
                    .expect("clipboard gzip");
                let response = SspGetClipboardResponse {
                    r#type: None,
                    clipboard: vec![SspClipboard {
                        content: Some(encoder.finish().expect("clipboard gzip finish")),
                        mstimestamp: Some(42),
                    }],
                };
                write_normal(&mut stream, sid, &response.encode_to_vec()).await;
            }
            SspRequestType::PostClipboardRequest
            | SspRequestType::ClearClipboardRequest
            | SspRequestType::DeleteClipboardRequest => {
                let response = match request_type {
                    SspRequestType::PostClipboardRequest => SspPostClipboardResponse {
                        r#type: None,
                        succeed: Some(true),
                    }
                    .encode_to_vec(),
                    SspRequestType::ClearClipboardRequest => SspClearClipboardResponse {
                        r#type: None,
                        succeed: Some(true),
                    }
                    .encode_to_vec(),
                    SspRequestType::DeleteClipboardRequest => SspDeleteClipboardResponse {
                        r#type: None,
                        succeed: Some(true),
                    }
                    .encode_to_vec(),
                    _ => unreachable!(),
                };
                write_normal(&mut stream, sid, &response).await;
            }
            SspRequestType::QuitRequest => break,
            _ => {}
        }
    }
}

fn decode_host_key(payload: &[u8]) -> RsaPublicKey {
    let clear = Aes256CbcDecryptor::new((&KEY_TABLE[16..48]).into(), (&KEY_TABLE[..16]).into())
        .decrypt_padded_vec_mut::<Pkcs7>(&payload[16..])
        .expect("fake AES public key");
    let public_der = base64::engine::general_purpose::STANDARD
        .decode(clear)
        .expect("fake base64 public key");
    RsaPublicKey::from_pkcs1_der(&public_der).expect("fake RSA public key")
}

async fn read_upstream(stream: &mut TcpStream) -> (u32, u8, Vec<u8>) {
    read_upstream_result(stream)
        .await
        .expect("fake upstream frame")
}

async fn read_upstream_result(stream: &mut TcpStream) -> std::io::Result<(u32, u8, Vec<u8>)> {
    let mut header = [0_u8; 9];
    stream.read_exact(&mut header).await?;
    let sid = u32::from_be_bytes(header[..4].try_into().expect("sid"));
    let flag = header[4];
    let length = u32::from_be_bytes(header[5..].try_into().expect("length")) as usize;
    let mut payload = vec![0_u8; length];
    stream.read_exact(&mut payload).await?;
    Ok((sid, flag, payload))
}

async fn write_normal(stream: &mut TcpStream, sid: u32, body: &[u8]) {
    let mut payload = (body.len() as u64).to_be_bytes().to_vec();
    payload.extend_from_slice(body);
    for chunk in payload.chunks(7) {
        stream.write_all(&sid.to_be_bytes()).await.expect("sid");
        stream
            .write_all(&(chunk.len() as u16).to_be_bytes())
            .await
            .expect("chunk length");
        stream.write_all(chunk).await.expect("chunk");
    }
}

async fn write_raw(stream: &mut TcpStream, sid: u32, body: &[u8]) {
    // A download can be cancelled by the client closing the connection; a
    // broken pipe mid-write is then the legitimate end of transmission.
    for chunk in body.chunks(3) {
        if stream.write_all(&sid.to_be_bytes()).await.is_err() {
            return;
        }
        if stream
            .write_all(&(chunk.len() as u16).to_be_bytes())
            .await
            .is_err()
        {
            return;
        }
        if stream.write_all(chunk).await.is_err() {
            return;
        }
    }
}
