pub(crate) mod adb;

use async_trait::async_trait;
use tokio::net::TcpStream;

use crate::domain::AdbDevice;
use crate::error::Result;

pub(crate) struct ConnectedTransport {
    pub stream: TcpStream,
    pub device: AdbDevice,
    pub cleanup: adb::AdbForward,
}

#[async_trait]
pub(crate) trait TransportConnector {
    async fn connect(&self) -> Result<ConnectedTransport>;
}
