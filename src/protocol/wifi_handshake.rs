use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use prost::Message;
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::error::{Error, Result};
use crate::i18n;
use crate::protocol::crypto::SessionKeys;
use crate::protocol::frame::{WireDirection, WireLog, write_upstream};
use crate::protocol::handshake::{HandshakeOutcome, HandshakeStrategy, read_normal_direct};
use crate::protocol::proto::*;
use crate::state::{StateStore, TrustRecord};

/// Longest wait for the phone-side trust dialog before giving up.
const TRUST_WAIT_TIMEOUT: Duration = Duration::from_secs(120);

/// Compatible host identity reported to the phone during REQUEST_01.
const HOST_APP_VERSION: &str = "2.5.6";
const HOST_PROTOCOL_VERSION: &str = "1";
const HOST_MIN_CLIENT_VERSION: &str = "1.0.0";
const HOST_MODEL: &str = "HandShaker-Rust";
const HOST_HEARTBEAT_TIMEOUT_SECONDS: u64 = 30;

/// Metadata gathered during the WiFi two-round handshake.
#[derive(Debug, Clone)]
pub(crate) struct WifiHandshakeInfo {
    /// Phone-side stable identifier (android_id), keyed trust records.
    pub device_uuid: String,
    /// Device name reported by the phone.
    pub device_name: Option<String>,
    /// `ro.serialno` reported by the phone.
    #[allow(dead_code)] // protocol field kept for future serial-based selection
    pub usb_serial: Option<String>,
    /// APK version code reported by the phone.
    pub apk_version: Option<String>,
    /// APK version name reported by the phone.
    pub apk_version_name: Option<String>,
    /// Derived key echoed by the phone on TRUST_ALWAYS, to persist.
    pub derived_key: Option<Vec<u8>>,
    /// Final trust type returned by the phone.
    pub trust_type: Option<SspHandShakeTrustType>,
    /// Whether the phone asked for the trust dialog (first connect).
    pub trust_waiting: bool,
}

/// WiFi (LAN) two-round handshake: REQUEST_01 -> RESPONSE_01,
/// then REQUEST_02 -> RESPONSE_02 (possibly multiple rounds for the trust
/// dialog). Do not reuse the ADB raw public-key exchange.
pub(crate) struct WifiTrustHandshake {
    host_uuid: String,
    /// Trust records indexed by device_uuid, consulted for the derived key.
    trust: BTreeMap<String, TrustRecord>,
    /// When set, REQUEST_02 carries TRUST_REMOVE to clear the phone record.
    trust_remove: bool,
    /// When present, a phone-side `failed` reply clears the stale local
    /// record so the next connect can re-trust instead of failing forever.
    trust_store: Option<StateStore>,
}

impl WifiTrustHandshake {
    pub fn new(host_uuid: String, trust: BTreeMap<String, TrustRecord>) -> Self {
        Self {
            host_uuid,
            trust,
            trust_remove: false,
            trust_store: None,
        }
    }

    /// Handshake that clears the phone-side trust record (trust reset).
    pub fn new_with_trust_remove(host_uuid: String) -> Self {
        Self {
            host_uuid,
            trust: BTreeMap::new(),
            trust_remove: true,
            trust_store: None,
        }
    }

    /// Attach the state store so a revoked trust clears the stale record.
    pub fn with_trust_store(mut self, store: StateStore) -> Self {
        self.trust_store = Some(store);
        self
    }
}

#[async_trait]
impl HandshakeStrategy for WifiTrustHandshake {
    async fn establish(
        &self,
        stream: &mut TcpStream,
        request_timeout: Duration,
        wire_log: Option<&Arc<WireLog>>,
    ) -> Result<HandshakeOutcome> {
        let keys = SessionKeys::generate()?;
        let sid = 0x8000_0001;

        // ---------- 1) REQUEST_01 -> RESPONSE_01 ----------
        let enckey = keys.build_enckey();
        let request01 = SspHandShakeRequest01 {
            r#type: Some(SspRequestType::HandshakeRequest01 as i32),
            host_uuid: Some(self.host_uuid.clone()),
            host_name: Some(host_name()),
            host_timestamp: Some(unix_seconds()),
            host_smart_sync_protocol_version: Some(HOST_PROTOCOL_VERSION.to_string()),
            host_app_version: Some(HOST_APP_VERSION.to_string()),
            host_min_client_version: Some(HOST_MIN_CLIENT_VERSION.to_string()),
            md5: Some(enckey[..16].to_vec()),
            enckey: Some(enckey[16..].to_vec()),
            host_model: Some(HOST_MODEL.to_string()),
            heartbeat_timeout_second: Some(HOST_HEARTBEAT_TIMEOUT_SECONDS),
        };
        send_unsigned(
            stream,
            sid,
            &request01,
            wire_log,
            i18n::text("wire.handshake_request_01"),
        )
        .await?;
        let response01 = timeout(request_timeout, read_normal_direct(stream, sid, wire_log))
            .await
            .map_err(|_| {
                Error::Timeout(i18n::text("handshake.wifi_response01_timeout").to_string())
            })??;
        let response01 =
            SspHandShakeResponse01::decode(response01.as_slice()).map_err(|error| {
                Error::Protocol(i18n::format(
                    "handshake.response01_decode",
                    &[&error.to_string()],
                ))
            })?;
        let device_uuid = response01.device_uuid.clone().ok_or_else(|| {
            Error::Protocol(i18n::text("handshake.response01_missing_uuid").to_string())
        })?;

        // ---------- 2) REQUEST_02 -> RESPONSE_02 (trust negotiation) ----------
        let derived_key = self.trust.get(&device_uuid).and_then(|record| {
            BASE64
                .decode(record.derived_key.as_bytes())
                .map_err(|_error| {
                    // Log only the static message: base64 DecodeError Display
                    // includes the offending byte, which would leak part of
                    // the encoded key into the local log.
                    tracing::warn!(
                        device_uuid = %device_uuid,
                        message = i18n::text("handshake.derived_key_decode_failed")
                    );
                })
                .ok()
        });
        let request02 = SspHandShakeRequest02 {
            r#type: Some(SspRequestType::HandshakeRequest02 as i32),
            host_uuid: Some(self.host_uuid.clone()),
            derived_key,
            trust_type: self
                .trust_remove
                .then_some(SspHandShakeTrustType::TrustRemove as i32),
        };
        send_unsigned(
            stream,
            sid,
            &request02,
            wire_log,
            i18n::text("wire.handshake_request_02"),
        )
        .await?;

        let mut info = WifiHandshakeInfo {
            device_uuid,
            device_name: response01.device_name.clone(),
            usb_serial: response01.usb_serial.clone(),
            apk_version: response01.apk_version.clone(),
            apk_version_name: response01.apk_version_name.clone(),
            derived_key: None,
            trust_type: None,
            trust_waiting: false,
        };
        let deadline = tokio::time::Instant::now() + TRUST_WAIT_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(Error::Timeout(
                    i18n::text("handshake.trust_timeout").to_string(),
                ));
            }
            let response = timeout(remaining, read_normal_direct(stream, sid, wire_log))
                .await
                .map_err(|_| Error::Timeout(i18n::text("handshake.trust_timeout").to_string()))??;
            let response =
                SspHandShakeResponse02::decode(response.as_slice()).map_err(|error| {
                    Error::Protocol(i18n::format(
                        "handshake.response02_decode",
                        &[&error.to_string()],
                    ))
                })?;

            info.trust_type = response
                .trust_type
                .and_then(|value| SspHandShakeTrustType::try_from(value).ok());
            if let Some(device_name) = &response.device_name {
                info.device_name = Some(device_name.clone());
            }
            if let Some(derived_key) = &response.derived_key {
                // The protocol fixes the derived key at 256 bytes; reject
                // oversized values instead of bloating state.json or breaking
                // the next REQUEST_02's 4 MiB upstream limit.
                if derived_key.len() > 256 {
                    return Err(Error::Protocol(
                        i18n::text("handshake.derived_key_too_large").to_string(),
                    ));
                }
                info.derived_key = Some(derived_key.clone());
            }

            if self.trust_remove {
                // TRUST_REMOVE only completes on a TRUST_WAITING round or a
                // verified-ok result; error strings must not be reported as
                // a successful reset.
                match response.result.as_deref() {
                    None => {
                        let waiting = response
                            .trust_type
                            .and_then(|value| SspHandShakeTrustType::try_from(value).ok())
                            == Some(SspHandShakeTrustType::TrustWaiting);
                        if !waiting {
                            continue; // keep waiting for the real acknowledgement
                        }
                        return Ok(HandshakeOutcome {
                            keys,
                            wifi: Some(info),
                        });
                    }
                    Some("failed") => {
                        return Err(Error::Handshake(
                            i18n::text("handshake.trust_failed").to_string(),
                        ));
                    }
                    Some("locked") => {
                        return Err(Error::Handshake(
                            i18n::text("handshake.phone_locked").to_string(),
                        ));
                    }
                    Some("needauth") => {
                        return Err(Error::Handshake(
                            i18n::text("handshake.authorization_required").to_string(),
                        ));
                    }
                    Some(_) => {
                        let clear = keys.decrypt_handshake_result(
                            response.result.unwrap_or_default().as_bytes(),
                        )?;
                        if clear != b"ok" {
                            return Err(Error::Handshake(i18n::format(
                                "handshake.result_invalid",
                                &[&format!("{:?}", String::from_utf8_lossy(&clear))],
                            )));
                        }
                        return Ok(HandshakeOutcome {
                            keys,
                            wifi: Some(info),
                        });
                    }
                }
            }

            match response.result.as_deref() {
                None => {
                    // Intermediate round: waiting for the trust dialog.
                    info.trust_waiting = true;
                    tracing::info!(
                        device_uuid = %info.device_uuid,
                        message = i18n::text("handshake.trust_waiting")
                    );
                    continue;
                }
                Some("failed") => {
                    // The phone revoked or mismatched the trust: drop the stale
                    // local record so a later connect can re-trust. This also
                    // means a rogue device claiming the same uuid can wipe the
                    // stored record; log it so the event is visible.
                    tracing::warn!(
                        device_uuid = %info.device_uuid,
                        message = i18n::text("handshake.trust_record_cleared")
                    );
                    if let Some(store) = &self.trust_store {
                        let _ = store.remove_trust(&info.device_uuid);
                    }
                    return Err(Error::Handshake(
                        i18n::text("handshake.trust_failed").to_string(),
                    ));
                }
                Some("locked") => {
                    return Err(Error::Handshake(
                        i18n::text("handshake.phone_locked").to_string(),
                    ));
                }
                Some("needauth") => {
                    return Err(Error::Handshake(
                        i18n::text("handshake.authorization_required").to_string(),
                    ));
                }
                Some(_) => {
                    let clear = keys
                        .decrypt_handshake_result(response.result.unwrap_or_default().as_bytes())?;
                    if clear != b"ok" {
                        return Err(Error::Handshake(i18n::format(
                            "handshake.result_invalid",
                            &[&format!("{:?}", String::from_utf8_lossy(&clear))],
                        )));
                    }
                    return Ok(HandshakeOutcome {
                        keys,
                        wifi: Some(info),
                    });
                }
            }
        }
    }
}

async fn send_unsigned<M: Message>(
    stream: &mut TcpStream,
    sid: u32,
    message: &M,
    wire_log: Option<&Arc<WireLog>>,
    note: &str,
) -> Result<()> {
    let payload = message.encode_to_vec();
    let frame = write_upstream(stream, sid, 0, &payload).await?;
    if let Some(log) = wire_log {
        log.record(WireDirection::Out, note, &frame);
    }
    Ok(())
}

fn host_name() -> String {
    // Protocol field; not user-facing text.
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("USER").map(|user| format!("{user}-handshaker")))
        .unwrap_or_else(|_| "handshaker-rust".to_string())
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_secs()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use aes::Aes256;
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as B64;
    use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
    use md5::{Digest as _, Md5};
    use rand::rngs::OsRng;
    use rsa::pkcs1::DecodeRsaPublicKey;
    use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    use crate::protocol::crypto::KEY_TABLE;
    use crate::state::TrustRecord;

    use super::*;

    type Aes256CbcDecryptor = cbc::Decryptor<Aes256>;

    async fn read_upstream(stream: &mut TcpStream) -> (u32, u8, Vec<u8>) {
        let mut header = [0_u8; 9];
        stream
            .read_exact(&mut header)
            .await
            .expect("upstream header");
        let sid = u32::from_be_bytes(header[..4].try_into().expect("sid"));
        let flag = header[4];
        let length = u32::from_be_bytes(header[5..].try_into().expect("length")) as usize;
        let mut payload = vec![0_u8; length];
        stream
            .read_exact(&mut payload)
            .await
            .expect("upstream payload");
        (sid, flag, payload)
    }

    async fn write_normal(stream: &mut TcpStream, sid: u32, body: &[u8]) {
        let mut payload = (body.len() as u64).to_be_bytes().to_vec();
        payload.extend_from_slice(body);
        for chunk in payload.chunks(5) {
            stream.write_all(&sid.to_be_bytes()).await.expect("sid");
            stream
                .write_all(&(chunk.len() as u16).to_be_bytes())
                .await
                .expect("chunk length");
            stream.write_all(chunk).await.expect("chunk");
        }
    }

    /// Recover the client RSA public key from REQUEST_01's md5/enckey fields.
    fn public_key_from_request01(payload: &[u8]) -> RsaPublicKey {
        let request = SspHandShakeRequest01::decode(payload).expect("request01");
        let encrypted = request.enckey.as_deref().expect("enckey");
        let clear = Aes256CbcDecryptor::new((&KEY_TABLE[16..48]).into(), (&KEY_TABLE[..16]).into())
            .decrypt_padded_vec_mut::<Pkcs7>(encrypted)
            .expect("AES public key");
        let public_der = B64.decode(clear).expect("base64 public key");
        assert_eq!(
            request.md5.as_deref().expect("md5"),
            &Md5::digest(&public_der)[..]
        );
        RsaPublicKey::from_pkcs1_der(&public_der).expect("RSA public key")
    }

    fn response02(
        trust_type: i32,
        derived_key: Option<&[u8]>,
        result: Option<String>,
    ) -> SspHandShakeResponse02 {
        SspHandShakeResponse02 {
            r#type: Some(SspRequestType::HandshakeResponse02 as i32),
            trust_type: Some(trust_type),
            device_uuid: Some("test-device".to_string()),
            device_name: Some("TestPhone".to_string()),
            derived_key: derived_key.map(|key| key.to_vec()),
            result,
        }
    }

    fn encrypted_ok(public: &RsaPublicKey) -> String {
        let encrypted = public
            .encrypt(&mut OsRng, Pkcs1v15Encrypt, b"ok")
            .expect("encrypt ok");
        B64.encode(encrypted)
    }

    const DEVICE_UUID: &str = "test-device";

    /// Capture-verified REQUEST_01 wire facts (docs/14 §10.2 and docs/04 §4.3).
    #[test]
    fn request01_carries_compatible_host_identity_and_split_enckey() {
        let request = SspHandShakeRequest01 {
            r#type: Some(SspRequestType::HandshakeRequest01 as i32),
            host_uuid: Some("host-uuid-1".to_string()),
            host_name: Some(host_name()),
            host_timestamp: Some(1234),
            host_smart_sync_protocol_version: Some(HOST_PROTOCOL_VERSION.to_string()),
            host_app_version: Some(HOST_APP_VERSION.to_string()),
            host_min_client_version: Some(HOST_MIN_CLIENT_VERSION.to_string()),
            md5: Some(vec![0xAB; 16]),
            enckey: Some(vec![0xCD; 16]),
            host_model: Some(HOST_MODEL.to_string()),
            heartbeat_timeout_second: Some(HOST_HEARTBEAT_TIMEOUT_SECONDS),
        };
        let encoded = request.encode_to_vec();
        let decoded = SspHandShakeRequest01::decode(encoded.as_slice()).expect("decode");
        assert_eq!(
            decoded.host_app_version.as_deref(),
            Some("2.5.6"),
            "host_app_version must match the verified 2.5.6 compatibility identity"
        );
        assert_eq!(decoded.host_min_client_version.as_deref(), Some("1.0.0"));
        assert_eq!(
            decoded.host_smart_sync_protocol_version.as_deref(),
            Some("1")
        );
        assert_eq!(decoded.heartbeat_timeout_second, Some(30));
        assert_eq!(decoded.md5.as_deref(), Some(&[0xAB; 16][..]));
        assert_eq!(decoded.enckey.as_deref(), Some(&[0xCD; 16][..]));
    }

    /// Capture-verified RESPONSE_01 field values (docs/14 §10.2).
    #[test]
    fn response01_decodes_capture_verified_phone_fields() {
        let encoded = SspHandShakeResponse01 {
            r#type: Some(SspRequestType::HandshakeResponse01 as i32),
            apk_version: Some("201".to_string()),
            apk_version_name: Some("1.2.0".to_string()),
            client_timestamp: Some(1_752_000_000),
            client_smart_sync_protocol_version: Some("1".to_string()),
            client_min_host_version: Some("2.1.0".to_string()),
            device_uuid: Some("android-id".to_string()),
            device_name: Some("DeviceName".to_string()),
            usb_serial: Some("serialno".to_string()),
            is_smartisan_device: Some(true),
            client_min_host_version_code: Some(333),
        }
        .encode_to_vec();
        let response = SspHandShakeResponse01::decode(encoded.as_slice()).expect("decode");
        assert_eq!(response.apk_version.as_deref(), Some("201"));
        assert_eq!(response.apk_version_name.as_deref(), Some("1.2.0"));
        assert_eq!(
            response.client_smart_sync_protocol_version.as_deref(),
            Some("1")
        );
        assert_eq!(response.client_min_host_version.as_deref(), Some("2.1.0"));
        assert_eq!(response.device_uuid.as_deref(), Some("android-id"));
        assert_eq!(response.usb_serial.as_deref(), Some("serialno"));
        assert_eq!(response.is_smartisan_device, Some(true));
        assert_eq!(response.client_min_host_version_code, Some(333));
    }

    #[tokio::test]
    async fn first_connect_waits_for_trust_dialog_then_succeeds() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("listener");
        let address = listener.local_addr().expect("address");
        let derived_key = vec![0xAB_u8; 256];
        let server_derived_key = derived_key.clone();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");

            let (sid, flag, payload) = read_upstream(&mut stream).await;
            assert_eq!(sid, 0x8000_0001);
            assert_eq!(flag, 0);
            let public = public_key_from_request01(&payload);
            let response01 = SspHandShakeResponse01 {
                r#type: Some(SspRequestType::HandshakeResponse01 as i32),
                device_uuid: Some(DEVICE_UUID.to_string()),
                device_name: Some("TestPhone".to_string()),
                usb_serial: Some("SN123".to_string()),
                apk_version: Some("201".to_string()),
                apk_version_name: Some("1.2.0".to_string()),
                ..Default::default()
            };
            write_normal(&mut stream, sid, &response01.encode_to_vec()).await;

            let (sid2, flag2, payload2) = read_upstream(&mut stream).await;
            assert_eq!(sid2, sid);
            assert_eq!(flag2, 0);
            let request02 = SspHandShakeRequest02::decode(payload2.as_slice()).expect("request02");
            assert_eq!(request02.host_uuid.as_deref(), Some("host-uuid-1"));
            assert!(request02.derived_key.is_none());
            assert!(request02.trust_type.is_none());

            write_normal(
                &mut stream,
                sid2,
                &response02(SspHandShakeTrustType::TrustWaiting as i32, None, None).encode_to_vec(),
            )
            .await;
            write_normal(
                &mut stream,
                sid2,
                &response02(
                    SspHandShakeTrustType::TrustAlways as i32,
                    Some(&server_derived_key),
                    Some(encrypted_ok(&public)),
                )
                .encode_to_vec(),
            )
            .await;
        });

        let mut stream = TcpStream::connect(address).await.expect("connect");
        let handshake = WifiTrustHandshake::new("host-uuid-1".to_string(), BTreeMap::new());
        let outcome = handshake
            .establish(&mut stream, Duration::from_secs(5), None)
            .await
            .expect("handshake");
        let info = outcome.wifi.expect("wifi info");
        assert_eq!(info.device_uuid, DEVICE_UUID);
        assert_eq!(info.device_name.as_deref(), Some("TestPhone"));
        assert_eq!(info.usb_serial.as_deref(), Some("SN123"));
        assert_eq!(info.derived_key.as_deref(), Some(derived_key.as_slice()));
        assert!(info.trust_waiting);
        assert_eq!(info.trust_type, Some(SspHandShakeTrustType::TrustAlways));
        let _ = server.await.expect("server task");
        drop(stream);
    }

    #[tokio::test]
    async fn reconnect_presents_stored_derived_key_and_succeeds_immediately() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("listener");
        let address = listener.local_addr().expect("address");
        let derived_key = vec![0xCD_u8; 256];
        let server_derived_key = derived_key.clone();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let (sid, _, payload) = read_upstream(&mut stream).await;
            let public = public_key_from_request01(&payload);
            write_normal(
                &mut stream,
                sid,
                &SspHandShakeResponse01 {
                    r#type: Some(SspRequestType::HandshakeResponse01 as i32),
                    device_uuid: Some(DEVICE_UUID.to_string()),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .await;

            let (sid2, _, payload2) = read_upstream(&mut stream).await;
            let request02 = SspHandShakeRequest02::decode(payload2.as_slice()).expect("request02");
            assert_eq!(
                request02.derived_key.as_deref(),
                Some(server_derived_key.as_slice()),
                "stored derived key must be presented on reconnect"
            );
            write_normal(
                &mut stream,
                sid2,
                &response02(
                    SspHandShakeTrustType::TrustAlways as i32,
                    Some(&server_derived_key),
                    Some(encrypted_ok(&public)),
                )
                .encode_to_vec(),
            )
            .await;
        });

        let mut trust = BTreeMap::new();
        trust.insert(
            DEVICE_UUID.to_string(),
            TrustRecord {
                device_name: Some("TestPhone".to_string()),
                derived_key: B64.encode(&derived_key),
                updated_at: 1,
            },
        );
        let mut stream = TcpStream::connect(address).await.expect("connect");
        let handshake = WifiTrustHandshake::new("host-uuid-1".to_string(), trust);
        let outcome = handshake
            .establish(&mut stream, Duration::from_secs(5), None)
            .await
            .expect("handshake");
        let info = outcome.wifi.expect("wifi info");
        assert!(!info.trust_waiting);
        let _ = server.await.expect("server task");
        drop(stream);
    }

    #[tokio::test]
    async fn wrong_derived_key_reports_failed() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("listener");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let (sid, _, payload) = read_upstream(&mut stream).await;
            let _public = public_key_from_request01(&payload);
            write_normal(
                &mut stream,
                sid,
                &SspHandShakeResponse01 {
                    r#type: Some(SspRequestType::HandshakeResponse01 as i32),
                    device_uuid: Some(DEVICE_UUID.to_string()),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .await;
            let (sid2, _, _) = read_upstream(&mut stream).await;
            write_normal(
                &mut stream,
                sid2,
                &response02(
                    SspHandShakeTrustType::TrustAlways as i32,
                    None,
                    Some("failed".to_string()),
                )
                .encode_to_vec(),
            )
            .await;
        });

        let mut stream = TcpStream::connect(address).await.expect("connect");
        // A stored trust record must be dropped when the phone replies failed.
        let temp = tempfile::tempdir().expect("tempdir");
        let store = StateStore::at(temp.path().join("state.json"));
        store
            .upsert_trust(DEVICE_UUID, Some("TestPhone"), &[0xEE; 256])
            .expect("seed trust");
        let handshake = WifiTrustHandshake::new(
            "host-uuid-1".to_string(),
            store.load_or_create().expect("state").trust,
        )
        .with_trust_store(store.clone());
        let error = handshake
            .establish(&mut stream, Duration::from_secs(5), None)
            .await
            .expect_err("must fail");
        assert!(matches!(error, Error::Handshake(_)), "{error:?}");
        assert!(
            store
                .load_or_create()
                .expect("state")
                .trust
                .get(DEVICE_UUID)
                .is_none(),
            "stale trust record must be cleared after a failed reconnect"
        );
        let _ = server.await.expect("server task");
        drop(stream);
    }

    #[tokio::test]
    async fn trust_remove_failure_is_reported_not_success() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("listener");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let (sid, _, payload) = read_upstream(&mut stream).await;
            let _public = public_key_from_request01(&payload);
            write_normal(
                &mut stream,
                sid,
                &SspHandShakeResponse01 {
                    r#type: Some(SspRequestType::HandshakeResponse01 as i32),
                    device_uuid: Some(DEVICE_UUID.to_string()),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .await;
            let (sid2, _, _) = read_upstream(&mut stream).await;
            write_normal(
                &mut stream,
                sid2,
                &response02(
                    SspHandShakeTrustType::TrustAlways as i32,
                    None,
                    Some("failed".to_string()),
                )
                .encode_to_vec(),
            )
            .await;
        });

        let mut stream = TcpStream::connect(address).await.expect("connect");
        let handshake = WifiTrustHandshake::new_with_trust_remove("host-uuid-1".to_string());
        let error = handshake
            .establish(&mut stream, Duration::from_secs(5), None)
            .await
            .expect_err("a failed trust remove must not be reported as success");
        assert!(matches!(error, Error::Handshake(_)), "{error:?}");
        let _ = server.await.expect("server task");
        drop(stream);
    }

    #[tokio::test]
    async fn trust_remove_is_sent_and_acknowledged() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("listener");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let (sid, _, payload) = read_upstream(&mut stream).await;
            let _public = public_key_from_request01(&payload);
            write_normal(
                &mut stream,
                sid,
                &SspHandShakeResponse01 {
                    r#type: Some(SspRequestType::HandshakeResponse01 as i32),
                    device_uuid: Some(DEVICE_UUID.to_string()),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .await;
            let (sid2, _, payload2) = read_upstream(&mut stream).await;
            let request02 = SspHandShakeRequest02::decode(payload2.as_slice()).expect("request02");
            assert_eq!(
                request02.trust_type,
                Some(SspHandShakeTrustType::TrustRemove as i32)
            );
            write_normal(
                &mut stream,
                sid2,
                &response02(SspHandShakeTrustType::TrustWaiting as i32, None, None).encode_to_vec(),
            )
            .await;
        });

        let mut stream = TcpStream::connect(address).await.expect("connect");
        let handshake = WifiTrustHandshake::new_with_trust_remove("host-uuid-1".to_string());
        let outcome = handshake
            .establish(&mut stream, Duration::from_secs(5), None)
            .await
            .expect("handshake");
        assert!(outcome.wifi.is_some());
        let _ = server.await.expect("server task");
        drop(stream);
    }
}
