pub(crate) mod adb;
pub(crate) mod wifi;

use async_trait::async_trait;
use tokio::net::TcpStream;

use crate::error::Result;

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

#[derive(Debug)]
pub(crate) struct ConnectedTransport {
    pub stream: TcpStream,
    /// Stable label used for session logging and error messages
    /// (ADB serial, or the WiFi `ip:port` before the phone identity is known).
    pub label: String,
    pub cleanup: TransportCleanup,
}

#[async_trait]
pub(crate) trait TransportConnector {
    async fn connect(&self) -> Result<ConnectedTransport>;
}
