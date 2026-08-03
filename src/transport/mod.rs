pub(crate) mod adb;
pub(crate) mod usb;
pub(crate) mod wifi;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::error::Result;

/// A duplex byte stream usable by the SSP session layer, regardless of
/// transport (TCP forward, WiFi socket, USB AOA bulk).
pub(crate) trait TransportStream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> TransportStream for T {}

/// Host-side resource tied to a connected transport, released on close.
#[derive(Debug)]
pub(crate) enum TransportCleanup {
    /// ADB forward created for this connection; removed exactly once.
    Adb(adb::AdbForward),
    /// No host-side resource to clean up (e.g. direct WiFi TCP).
    None,
}

impl TransportCleanup {
    pub async fn cleanup(self) -> Result<()> {
        match self {
            TransportCleanup::Adb(mut forward) => forward.cleanup().await,
            TransportCleanup::None => Ok(()),
        }
    }
}

#[allow(dead_code)]
pub(crate) struct ConnectedTransport {
    pub stream: Box<dyn TransportStream>,
    /// Stable label used for session logging and error messages
    /// (ADB serial, or the WiFi `ip:port` before the phone identity is known).
    pub label: String,
    pub cleanup: TransportCleanup,
}

#[async_trait]
pub(crate) trait TransportConnector {
    async fn connect(&self) -> Result<ConnectedTransport>;
}
