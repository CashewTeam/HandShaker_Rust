use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::error::{Error, Result};
use crate::protocol::crypto::SessionKeys;
use crate::protocol::frame::{WireDirection, WireLog, read_downstream, write_upstream};

#[async_trait]
pub(crate) trait HandshakeStrategy: Send + Sync {
    async fn establish(
        &self,
        stream: &mut TcpStream,
        request_timeout: Duration,
        wire_log: Option<&Arc<WireLog>>,
    ) -> Result<SessionKeys>;
}

/// The ADB service uses the capture-verified USB-style raw public-key exchange.
pub(crate) struct AdbRawKeyExchange;

#[async_trait]
impl HandshakeStrategy for AdbRawKeyExchange {
    async fn establish(
        &self,
        stream: &mut TcpStream,
        request_timeout: Duration,
        wire_log: Option<&Arc<WireLog>>,
    ) -> Result<SessionKeys> {
        let keys = SessionKeys::generate()?;
        let sid = 0x8000_0001;
        let payload = keys.build_enckey();
        let future = async {
            let frame = write_upstream(stream, sid, 0, &payload).await?;
            if let Some(log) = wire_log {
                log.record(WireDirection::Out, "ADB raw handshake", &frame);
            }
            let response = read_normal_direct(stream, sid, wire_log).await?;
            match response.trim_ascii() {
                b"failed" => return Err(Error::Handshake("手机拒绝了公钥".to_string())),
                b"locked" => return Err(Error::Handshake("手机处于锁屏状态".to_string())),
                b"needauth" => {
                    return Err(Error::Handshake("需要在手机端确认连接授权".to_string()));
                }
                _ => {}
            }
            let clear = keys.decrypt_handshake_result(&response)?;
            if clear != b"ok" {
                return Err(Error::Handshake(format!(
                    "握手解密结果不是 ok：{:?}",
                    String::from_utf8_lossy(&clear)
                )));
            }
            Ok(())
        };
        timeout(request_timeout, future)
            .await
            .map_err(|_| Error::Timeout("ADB 握手".to_string()))??;
        Ok(keys)
    }
}

async fn read_normal_direct(
    stream: &mut TcpStream,
    expected_sid: u32,
    wire_log: Option<&Arc<WireLog>>,
) -> Result<Vec<u8>> {
    let mut assembled = Vec::new();
    let mut total = None;
    loop {
        let (sid, chunk, header) = read_downstream(stream).await?;
        if sid != expected_sid {
            return Err(Error::Protocol(format!(
                "握手响应 sid 不匹配：期望 {expected_sid:#010x}，实际 {sid:#010x}"
            )));
        }
        if let Some(log) = wire_log {
            log.record(WireDirection::In, "handshake header", &header);
            log.record(WireDirection::In, "handshake chunk", &chunk);
        }
        assembled.extend_from_slice(&chunk);
        if total.is_none() && assembled.len() >= 8 {
            total = Some(u64::from_be_bytes(
                assembled[..8].try_into().expect("eight bytes"),
            ));
        }
        if let Some(total) = total {
            let total = usize::try_from(total)
                .map_err(|_| Error::Protocol("响应长度超出平台范围".to_string()))?;
            let expected = total
                .checked_add(8)
                .ok_or_else(|| Error::Protocol("响应长度溢出".to_string()))?;
            if assembled.len() == expected {
                return Ok(assembled.split_off(8));
            }
            if assembled.len() > expected {
                return Err(Error::Protocol("普通响应超过声明长度".to_string()));
            }
        }
    }
}
