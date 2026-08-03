//! Stable public error model (M8 §6): partitioned codes, `message` for
//! display only, `detail` for diagnostics (never secrets or wire payloads).

/// Stable public error codes (partitioned, see M8 plan §6.2). Values are
/// frozen for v1: never reuse a released number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(i32)]
#[non_exhaustive]
pub enum PublicErrorCode {
    // 1000–1099 Runtime
    RuntimeClosed = 1001,
    // 1100–1199 arguments and state
    InvalidArgument = 1101,
    InvalidState = 1102,
    NotFound = 1103,
    // 2000–2099 device discovery
    DeviceNotFound = 2001,
    DeviceUnavailable = 2002,
    // 2100–2199 connection
    ConnectFailed = 2101,
    ConnectionLost = 2102,
    SessionNotFound = 2103,
    SessionClosed = 2104,
    // 2200–2299 trust and handshake
    TrustRequired = 2201,
    TrustRejected = 2202,
    // 3000–3099 remote file system
    RemotePathNotFound = 3001,
    RemotePermissionDenied = 3002,
    RemotePathExists = 3003,
    /// Generic remote-side operation failure (e.g. an aggregated batch of
    /// items where some failed; Phase D / D4).
    RemoteIo = 3004,
    // 3100–3199 local file system
    LocalPathNotFound = 3101,
    LocalPermissionDenied = 3102,
    LocalPathExists = 3103,
    // 4000–4199 transfer and tasks
    TransferNotFound = 4201,
    TransferCancelled = 4202,
    RemoteCancelled = 4203,
    // 5000–5199 protocol
    ProtocolError = 5001,
    DecodeError = 5101,
    // 6000–6299 transport backends
    AdbUnavailable = 6001,
    AdbUnauthorized = 6002,
    AdbOffline = 6003,
    WifiDiscoveryFailed = 6101,
    UsbUnavailable = 6201,
    // 7000–7299 media and clipboard
    MediaError = 7001,
    ClipboardError = 7101,
    SyncError = 7201,
    // 9000–9099 internal
    Internal = 9001,
}

impl PublicErrorCode {
    /// Stable machine-readable lowercase token (also the JSON `code` field).
    pub fn as_str(self) -> &'static str {
        use PublicErrorCode::*;
        match self {
            RuntimeClosed => "runtime_closed",
            InvalidArgument => "invalid_argument",
            InvalidState => "invalid_state",
            NotFound => "not_found",
            DeviceNotFound => "device_not_found",
            DeviceUnavailable => "device_unavailable",
            ConnectFailed => "connect_failed",
            ConnectionLost => "connection_lost",
            SessionNotFound => "session_not_found",
            SessionClosed => "session_closed",
            TrustRequired => "trust_required",
            TrustRejected => "trust_rejected",
            RemotePathNotFound => "remote_path_not_found",
            RemotePermissionDenied => "remote_permission_denied",
            RemotePathExists => "remote_path_exists",
            RemoteIo => "remote_io",
            LocalPathNotFound => "local_path_not_found",
            LocalPermissionDenied => "local_permission_denied",
            LocalPathExists => "local_path_exists",
            TransferNotFound => "transfer_not_found",
            TransferCancelled => "transfer_cancelled",
            RemoteCancelled => "remote_cancelled",
            ProtocolError => "protocol_error",
            DecodeError => "decode_error",
            AdbUnavailable => "adb_unavailable",
            AdbUnauthorized => "adb_unauthorized",
            AdbOffline => "adb_offline",
            WifiDiscoveryFailed => "wifi_discovery_failed",
            UsbUnavailable => "usb_unavailable",
            MediaError => "media_error",
            ClipboardError => "clipboard_error",
            SyncError => "sync_error",
            Internal => "internal",
        }
    }
}

/// Stable public error. `message` is for display only; programmatic decisions
/// must use `code`. `retryable` is a hint, never a promise.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PublicError {
    /// Stable token (`as_str`), not the numeric discriminant: numeric values
    /// are frozen but names are the JSON contract.
    pub code: PublicErrorCode,
    pub message: String,
    pub detail: Option<String>,
    pub retryable: bool,
    pub operation: Option<String>,
}

impl std::fmt::Display for PublicError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.detail, &self.operation) {
            (Some(detail), Some(operation)) => {
                write!(formatter, "{}: {detail} ({operation})", self.code.as_str())
            }
            (Some(detail), None) => write!(formatter, "{}: {detail}", self.code.as_str()),
            (None, Some(operation)) => write!(formatter, "{} ({operation})", self.code.as_str()),
            (None, None) => formatter.write_str(self.code.as_str()),
        }
    }
}

impl PublicError {
    pub fn new(code: PublicErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: None,
            retryable: false,
            operation: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn retryable(mut self) -> Self {
        self.retryable = true;
        self
    }

    pub fn operation(mut self, operation: impl Into<String>) -> Self {
        self.operation = Some(operation.into());
        self
    }
}

/// Result alias for application-layer operations.
pub type AppResult<T> = Result<T, PublicError>;

/// Map a core error to the stable public error. Rules: never leak keys or
/// wire payloads, never parse localized messages, unknown → `Internal`.
pub fn from_core_error(error: handshaker_core::Error, operation: &str) -> PublicError {
    use handshaker_core::Error;
    let (code, retryable) = match &error {
        Error::Interrupted => (PublicErrorCode::TransferCancelled, false),
        // M8.1 Phase C / C3: distinguish local cancellation from a phone-side
        // (remote) cancel so GUI can tell "I cancelled" from "the phone did".
        Error::Cancelled(info) => match info.origin {
            handshaker_core::CancellationOrigin::Local { .. } => {
                (PublicErrorCode::TransferCancelled, false)
            }
            handshaker_core::CancellationOrigin::Remote { .. } => {
                (PublicErrorCode::RemoteCancelled, false)
            }
        },
        Error::Timeout(_) => (PublicErrorCode::ConnectionLost, true),
        Error::Transport(_) => (PublicErrorCode::ConnectFailed, true),
        Error::Handshake(_) => (PublicErrorCode::TrustRejected, false),
        Error::Protocol(_) => (PublicErrorCode::ProtocolError, false),
        Error::AdbUnavailable(_) => (PublicErrorCode::AdbUnavailable, false),
        Error::DeviceSelection(_) => (PublicErrorCode::DeviceNotFound, false),
        Error::Configuration(_) => (PublicErrorCode::InvalidState, false),
        Error::RemoteIo { .. } => (PublicErrorCode::RemotePathNotFound, false),
        Error::LocalIo(_) => (PublicErrorCode::LocalPathNotFound, false),
        Error::Usage(_) => (PublicErrorCode::InvalidArgument, false),
        Error::ConfirmationRequired(_) => (PublicErrorCode::InvalidState, false),
    };
    let detail = match &error {
        Error::Transport(message)
        | Error::Handshake(message)
        | Error::Protocol(message)
        | Error::Configuration(message)
        | Error::LocalIo(message)
        | Error::Usage(message)
        | Error::Timeout(message) => Some(message.clone()),
        Error::RemoteIo { message, .. } => Some(message.clone()),
        _ => None,
    };
    PublicError {
        code,
        message: error.to_string(),
        detail,
        retryable,
        operation: Some(operation.to_string()),
    }
}
