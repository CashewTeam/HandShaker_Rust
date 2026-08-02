mod client;
mod domain;
mod error;
pub mod i18n;
mod protocol;
mod session;
mod state;
mod transport;

pub use client::{ClientOptions, ConnectionTarget, HandShakerClient, PingResult};
pub use domain::{
    AdbDevice, ClipboardEntry, DeleteOptions, DeviceInfo, RemoteFile, RemoteIoError,
    TransferDirection, TransferOptions, TransferProgress, TransferProgressCallback,
};
pub use error::{Error, ErrorCode, Result};
