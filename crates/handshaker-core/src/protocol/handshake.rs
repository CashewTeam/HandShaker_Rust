use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::timeout;

use crate::error::{Error, Result};
use crate::i18n;
use crate::protocol::crypto::SessionKeys;
use crate::protocol::frame::{WireDirection, WireLog, read_downstream, write_upstream};

/// Result of a completed transport handshake.
#[derive(Debug)]
pub(crate) struct HandshakeOutcome {
    pub keys: SessionKeys,
    /// WiFi/ADB two-round handshake metadata, when applicable.
    pub wifi: Option<super::wifi_handshake::WifiHandshakeInfo>,
}

/// A transport-agnostic handshake. Implementations only need a duplex byte
/// stream (TCP, USB bulk, ...) and speak the same SSP frames on all of them.
#[async_trait]
pub(crate) trait HandshakeStrategy<S>: Send + Sync
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    async fn establish(
        &self,
        stream: &mut S,
        request_timeout: Duration,
        wire_log: Option<&Arc<WireLog>>,
    ) -> Result<HandshakeOutcome>;
}

/// The ADB service uses the capture-verified USB-style raw public-key exchange.
pub(crate) struct AdbRawKeyExchange;

#[async_trait]
impl<S> HandshakeStrategy<S> for AdbRawKeyExchange
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    async fn establish(
        &self,
        stream: &mut S,
        request_timeout: Duration,
        wire_log: Option<&Arc<WireLog>>,
    ) -> Result<HandshakeOutcome> {
        let keys = SessionKeys::generate()?;
        let sid = 0x8000_0001;
        let payload = keys.build_enckey();
        let future = async {
            let frame = write_upstream(stream, sid, 0, &payload).await?;
            if let Some(log) = wire_log {
                log.record(
                    WireDirection::Out,
                    i18n::text("wire.adb_raw_handshake"),
                    &frame,
                );
            }
            let response = read_normal_direct(stream, sid, wire_log).await?;
            match response.trim_ascii() {
                b"failed" => {
                    return Err(Error::Handshake(
                        i18n::text("handshake.public_key_rejected").to_string(),
                    ));
                }
                b"locked" => {
                    return Err(Error::Handshake(
                        i18n::text("handshake.phone_locked").to_string(),
                    ));
                }
                b"needauth" => {
                    return Err(Error::Handshake(
                        i18n::text("handshake.authorization_required").to_string(),
                    ));
                }
                _ => {}
            }
            let clear = keys.decrypt_handshake_result(&response)?;
            if clear != b"ok" {
                // A malicious phone could inject arbitrary bytes into the
                // error string; log only a bounded hex prefix.
                let mut preview = String::new();
                for byte in clear.iter().take(16) {
                    use std::fmt::Write;
                    let _ = write!(preview, "{byte:02x}");
                }
                return Err(Error::Handshake(i18n::format(
                    "handshake.result_invalid",
                    &[&preview],
                )));
            }
            Ok(())
        };
        timeout(request_timeout, future)
            .await
            .map_err(|_| Error::Timeout(i18n::text("handshake.adb").to_string()))??;
        Ok(HandshakeOutcome { keys, wifi: None })
    }
}

pub(crate) async fn read_normal_direct<R>(
    stream: &mut R,
    expected_sid: u32,
    wire_log: Option<&Arc<WireLog>>,
) -> Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    // A rogue phone (or a spoofed mDNS entry) could declare an arbitrarily
    // large length prefix; cap the accumulated handshake response so the
    // wait loop cannot balloon client memory.
    const MAX_HANDSHAKE_RESPONSE: usize = 16 * 1024 * 1024;
    let mut assembled = Vec::new();
    let mut total = None;
    loop {
        let (sid, chunk, header) = read_downstream(stream).await?;
        if sid != expected_sid {
            return Err(Error::Protocol(i18n::format(
                "handshake.sid_mismatch",
                &[&format!("{expected_sid:#010x}"), &format!("{sid:#010x}")],
            )));
        }
        if let Some(log) = wire_log {
            log.record(
                WireDirection::In,
                i18n::text("wire.handshake_header"),
                &header,
            );
            log.record(
                WireDirection::In,
                i18n::text("wire.handshake_chunk"),
                &chunk,
            );
        }
        assembled.extend_from_slice(&chunk);
        if total.is_none() && assembled.len() >= 8 {
            total = Some(u64::from_be_bytes(
                assembled[..8].try_into().expect("eight bytes"),
            ));
        }
        if let Some(total) = total {
            let total = usize::try_from(total).map_err(|_| {
                Error::Protocol(i18n::text("session.response_length_too_large").to_string())
            })?;
            if total > MAX_HANDSHAKE_RESPONSE {
                return Err(Error::Protocol(
                    i18n::text("session.response_length_too_large").to_string(),
                ));
            }
            let expected = total.checked_add(8).ok_or_else(|| {
                Error::Protocol(i18n::text("session.response_length_overflow").to_string())
            })?;
            if assembled.len() == expected {
                return Ok(assembled.split_off(8));
            }
            if assembled.len() > expected {
                return Err(Error::Protocol(
                    i18n::text("session.response_too_long").to_string(),
                ));
            }
        }
    }
}
