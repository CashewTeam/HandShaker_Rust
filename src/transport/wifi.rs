use std::net::SocketAddr;
use std::time::Duration;

use async_trait::async_trait;
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::error::{Error, Result};
use crate::i18n;
use crate::transport::{ConnectedTransport, TransportCleanup, TransportConnector};

/// Direct TCP connector for the WiFi (LAN) channel.
///
/// The phone listens on a dynamic port obtained from the mDNS SRV record;
/// the caller is responsible for resolving it before constructing this connector.
pub(crate) struct WifiConnector {
    address: SocketAddr,
    timeout: Duration,
}

impl WifiConnector {
    pub fn new(address: SocketAddr, timeout: Duration) -> Self {
        Self { address, timeout }
    }
}

#[async_trait]
impl TransportConnector for WifiConnector {
    async fn connect(&self) -> Result<ConnectedTransport> {
        let stream = timeout(self.timeout, TcpStream::connect(self.address))
            .await
            .map_err(|_| Error::Timeout(i18n::text("wifi.connect_timeout").to_string()))?
            .map_err(|error| {
                Error::Transport(i18n::format(
                    "wifi.connect_failed",
                    &[&self.address.to_string(), &error.to_string()],
                ))
            })?;
        stream.set_nodelay(true).map_err(|error| {
            Error::Transport(i18n::format("wifi.nodelay_failed", &[&error.to_string()]))
        })?;
        Ok(ConnectedTransport {
            stream,
            label: self.address.to_string(),
            cleanup: TransportCleanup::None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connects_and_returns_address_label() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("local address");
        let accept = tokio::spawn(async move { listener.accept().await.expect("accept") });
        let connector = WifiConnector::new(address, Duration::from_secs(5));
        let connected = connector.connect().await.expect("connect");
        assert_eq!(connected.label, address.to_string());
        let _ = accept.await.expect("accept task");
        drop(connected.stream);
    }

    #[tokio::test]
    async fn refused_connection_reports_transport_error() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("local address");
        drop(listener); // Close the port so connect is refused.
        let connector = WifiConnector::new(address, Duration::from_secs(1));
        let error = connector.connect().await.expect_err("must fail");
        assert!(matches!(error, Error::Transport(_)), "{error:?}");
    }
}
