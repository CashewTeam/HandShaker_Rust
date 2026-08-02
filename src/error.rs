use serde::Serialize;

use crate::i18n;

/// Result type returned by the HandShaker library.
pub type Result<T> = std::result::Result<T, Error>;

/// Stable error categories used by the library and CLI.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Usage,
    Configuration,
    DeviceSelection,
    AdbUnavailable,
    Transport,
    Handshake,
    Timeout,
    Protocol,
    RemoteIo,
    LocalIo,
    ConfirmationRequired,
    Interrupted,
}

#[derive(Debug)]
/// Error returned by connection, protocol, remote I/O, or local I/O paths.
pub enum Error {
    Usage(String),
    Configuration(String),
    DeviceSelection(String),
    AdbUnavailable(String),
    Transport(String),
    Handshake(String),
    Timeout(String),
    Protocol(String),
    RemoteIo { code: Option<i32>, message: String },
    LocalIo(String),
    ConfirmationRequired(String),
    Interrupted,
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::Usage(message) => i18n::format("error.usage", &[message]),
            Self::Configuration(message) => i18n::format("error.configuration", &[message]),
            Self::DeviceSelection(message) => i18n::format("error.device_selection", &[message]),
            Self::AdbUnavailable(message) => i18n::format("error.adb_unavailable", &[message]),
            Self::Transport(message) => i18n::format("error.transport", &[message]),
            Self::Handshake(message) => i18n::format("error.handshake", &[message]),
            Self::Timeout(message) => i18n::format("error.timeout", &[message]),
            Self::Protocol(message) => i18n::format("error.protocol", &[message]),
            Self::RemoteIo { code, message } => {
                i18n::format("error.remote_io", &[&format!("{code:?}"), message])
            }
            Self::LocalIo(message) => i18n::format("error.local_io", &[message]),
            Self::ConfirmationRequired(message) => {
                i18n::format("error.confirmation_required", &[message])
            }
            Self::Interrupted => i18n::text("error.interrupted").to_string(),
        };
        formatter.write_str(&message)
    }
}

impl std::error::Error for Error {}

impl Error {
    /// Return the stable category for this error.
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Usage(_) => ErrorCode::Usage,
            Self::Configuration(_) => ErrorCode::Configuration,
            Self::DeviceSelection(_) => ErrorCode::DeviceSelection,
            Self::AdbUnavailable(_) => ErrorCode::AdbUnavailable,
            Self::Transport(_) => ErrorCode::Transport,
            Self::Handshake(_) => ErrorCode::Handshake,
            Self::Timeout(_) => ErrorCode::Timeout,
            Self::Protocol(_) => ErrorCode::Protocol,
            Self::RemoteIo { .. } => ErrorCode::RemoteIo,
            Self::LocalIo(_) => ErrorCode::LocalIo,
            Self::ConfirmationRequired(_) => ErrorCode::ConfirmationRequired,
            Self::Interrupted => ErrorCode::Interrupted,
        }
    }

    /// Return the stable CLI process exit code for this error.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Usage(_) => 2,
            Self::Configuration(_) | Self::DeviceSelection(_) | Self::AdbUnavailable(_) => 3,
            Self::Transport(_) | Self::Handshake(_) | Self::Timeout(_) => 4,
            Self::Protocol(_) => 5,
            Self::RemoteIo { .. } => 6,
            Self::LocalIo(_) => 7,
            Self::ConfirmationRequired(_) => 8,
            Self::Interrupted => 130,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::LocalIo(value.to_string())
    }
}

impl From<prost::DecodeError> for Error {
    fn from(value: prost::DecodeError) -> Self {
        Self::Protocol(i18n::format("error.protobuf_decode", &[&value.to_string()]))
    }
}
