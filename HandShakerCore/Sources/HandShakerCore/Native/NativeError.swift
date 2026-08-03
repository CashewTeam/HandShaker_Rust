import Foundation

/// The error payload Rust sends on `status != 0` calls — the `PublicError`
/// JSON contract (`crates/handshaker-application/src/error.rs`):
/// `{"code":"...","message":"...","detail":null,"retryable":false,"operation":null}`.
///
/// `code` is the stable snake_case token (`PublicErrorCode::as_str`), never
/// the numeric discriminant. `message` is for display only; programmatic
/// decisions must switch on `code`. Optional fields tolerate both `null`
/// and missing keys via the synthesized `decodeIfPresent` decoding.
public struct HandShakerNativeError: Error, Codable, Sendable, Equatable {
    /// Stable machine-readable token, e.g. "session_not_found".
    public var code: String
    /// Human-readable message (display only).
    public var message: String
    /// Diagnostics detail; never secrets or wire payloads.
    public var detail: String?
    /// Hint, never a promise.
    public var retryable: Bool
    /// Operation that produced the error, e.g. "ping".
    public var operation: String?

    public init(
        code: String,
        message: String,
        detail: String? = nil,
        retryable: Bool = false,
        operation: String? = nil
    ) {
        self.code = code
        self.message = message
        self.detail = detail
        self.retryable = retryable
        self.operation = operation
    }
}

/// Swift-side error type mapped from the native `PublicErrorCode` tokens
/// (every `as_str` value in `crates/handshaker-application/src/error.rs`).
/// Each case carries the native `message` for display. `unknown` is the
/// fallback for tokens this SDK version does not know yet (forward
/// compatibility: a newer Rust library may emit new codes).
public enum HandShakerError: Error, Sendable, Equatable {
    // 1000–1099 Runtime
    case runtimeClosed(String)
    // 1100–1199 arguments and state
    case invalidArgument(String)
    case invalidState(String)
    case notFound(String)
    // 2000–2099 device discovery
    case deviceNotFound(String)
    case deviceUnavailable(String)
    // 2100–2199 connection
    case connectFailed(String)
    case connectionLost(String)
    case sessionNotFound(String)
    case sessionClosed(String)
    // 2200–2299 trust and handshake
    case trustRequired(String)
    case trustRejected(String)
    // 3000–3099 remote file system
    case remotePathNotFound(String)
    case remotePermissionDenied(String)
    case remotePathExists(String)
    case remoteIO(String)
    // 3100–3199 local file system
    case localPathNotFound(String)
    case localPermissionDenied(String)
    case localPathExists(String)
    // 4000–4199 transfer and tasks
    case transferNotFound(String)
    case transferCancelled(String)
    case remoteCancelled(String)
    // 5000–5199 protocol
    case protocolError(String)
    case decodeError(String)
    // 6000–6299 transport backends
    case adbUnavailable(String)
    case adbUnauthorized(String)
    case adbOffline(String)
    case wifiDiscoveryFailed(String)
    case usbUnavailable(String)
    // 7000–7299 media and clipboard
    case mediaError(String)
    case clipboardError(String)
    case syncError(String)
    // 9000–9099 internal
    case `internal`(String)
    /// Swift-side only (no Rust token): ABI or SDK precondition failures.
    case unsupported(String)
    /// Fallback for unknown native `code` tokens (holds the raw token).
    case unknown(String)

    /// Map a native `PublicError` onto the typed Swift error.
    public static func fromNative(_ error: HandShakerNativeError) -> HandShakerError {
        switch error.code {
        case "runtime_closed": return .runtimeClosed(error.message)
        case "invalid_argument": return .invalidArgument(error.message)
        case "invalid_state": return .invalidState(error.message)
        case "not_found": return .notFound(error.message)
        case "device_not_found": return .deviceNotFound(error.message)
        case "device_unavailable": return .deviceUnavailable(error.message)
        case "connect_failed": return .connectFailed(error.message)
        case "connection_lost": return .connectionLost(error.message)
        case "session_not_found": return .sessionNotFound(error.message)
        case "session_closed": return .sessionClosed(error.message)
        case "trust_required": return .trustRequired(error.message)
        case "trust_rejected": return .trustRejected(error.message)
        case "remote_path_not_found": return .remotePathNotFound(error.message)
        case "remote_permission_denied": return .remotePermissionDenied(error.message)
        case "remote_path_exists": return .remotePathExists(error.message)
        case "remote_io": return .remoteIO(error.message)
        case "local_path_not_found": return .localPathNotFound(error.message)
        case "local_permission_denied": return .localPermissionDenied(error.message)
        case "local_path_exists": return .localPathExists(error.message)
        case "transfer_not_found": return .transferNotFound(error.message)
        case "transfer_cancelled": return .transferCancelled(error.message)
        case "remote_cancelled": return .remoteCancelled(error.message)
        case "protocol_error": return .protocolError(error.message)
        case "decode_error": return .decodeError(error.message)
        case "adb_unavailable": return .adbUnavailable(error.message)
        case "adb_unauthorized": return .adbUnauthorized(error.message)
        case "adb_offline": return .adbOffline(error.message)
        case "wifi_discovery_failed": return .wifiDiscoveryFailed(error.message)
        case "usb_unavailable": return .usbUnavailable(error.message)
        case "media_error": return .mediaError(error.message)
        case "clipboard_error": return .clipboardError(error.message)
        case "sync_error": return .syncError(error.message)
        case "internal": return .internal(error.message)
        default: return .unknown(error.code)
        }
    }
}
