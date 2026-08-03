import Foundation

/// Runtime diagnostics (ffi/src/diagnostics.rs `hs_runtime_diagnostics`
/// result JSON). `adb_available`/`adb_version` probe the configured adb
/// binary with `adb version`; adb probing never fails the call.
public struct RuntimeDiagnostics: Codable, Sendable, Equatable {
    /// "1.5.0" (ABI version, independent of the Rust crate version).
    public let abi: String
    /// Application API version, e.g. "1.0.0-preview.1".
    public let applicationApi: String
    public let crateVersion: String
    /// "macos" / "linux" / "windows".
    public let platform: String
    /// "aarch64" / "x86_64" ...
    public let arch: String
    public let adbPath: String
    public let adbAvailable: Bool
    /// First line of `adb version`, when the probe succeeded.
    public let adbVersion: String?
    /// Configured state dir, or nil when unset (core default is used).
    public let stateDir: String?
    public let wireLogEnabled: Bool
    public let activeSessions: UInt64
    /// Number of non-terminal transfers; -1 when unknown.
    public let activeTransfers: Int64
    /// Feature tokens: "files", "clipboard", "trust", "media", "batch",
    /// "sync", "monitor", "events", "discovery", "diagnostics",
    /// "update_file_info", "media_merge".
    public let capabilities: [String]

    private enum CodingKeys: String, CodingKey {
        case abi
        case applicationApi = "application_api"
        case crateVersion = "crate_version"
        case platform
        case arch
        case adbPath = "adb_path"
        case adbAvailable = "adb_available"
        case adbVersion = "adb_version"
        case stateDir = "state_dir"
        case wireLogEnabled = "wire_log_enabled"
        case activeSessions = "active_sessions"
        case activeTransfers = "active_transfers"
        case capabilities
    }
}
