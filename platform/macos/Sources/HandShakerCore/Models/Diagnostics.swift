import Foundation

/// Runtime diagnostics (ffi/src/diagnostics.rs `hs_runtime_diagnostics`
/// result JSON). `adb_available`/`adb_version` probe the configured adb
/// binary with `adb version`; adb probing never fails the call.
public struct RuntimeDiagnostics: Codable, Sendable, Equatable {
    /// "1.5.0" (ABI version, independent of the Rust crate version).
    public let abi: String
    /// Application API version, e.g. "1.0.0-preview.1".
    public let applicationApi: String
    /// P1-7: JSON wire contract version (independent of the C ABI); the
    /// SDK refuses to create a runtime when it is older than the version
    /// this SDK was built against. 0 = older library without the field.
    public let jsonContract: UInt32
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
    /// P2-5: live event subscriptions (bounded by the native
    /// MAX_SUBSCRIPTIONS; each pins a small Tokio runtime).
    public let activeSubscriptions: Int
    public let capabilities: [String]

    /// Minimum JSON contract version this SDK understands (P1-7). Bump
    /// together with the Swift models whenever a breaking JSON change is
    /// shipped.
    public static let minimumJSONContract: UInt32 = 1

    private enum CodingKeys: String, CodingKey {
        case abi
        case applicationApi = "application_api"
        case jsonContract = "json_contract"
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
        case activeSubscriptions = "active_subscriptions"
        case capabilities
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        abi = try container.decode(String.self, forKey: .abi)
        applicationApi = try container.decode(String.self, forKey: .applicationApi)
        // P1-7: an older library without the field decodes as 0; the
        // runtime-creation check then refuses it with a clear error
        // instead of a generic decode failure.
        jsonContract = try container.decodeIfPresent(UInt32.self, forKey: .jsonContract) ?? 0
        crateVersion = try container.decode(String.self, forKey: .crateVersion)
        platform = try container.decode(String.self, forKey: .platform)
        arch = try container.decode(String.self, forKey: .arch)
        adbPath = try container.decode(String.self, forKey: .adbPath)
        adbAvailable = try container.decode(Bool.self, forKey: .adbAvailable)
        adbVersion = try container.decodeIfPresent(String.self, forKey: .adbVersion)
        stateDir = try container.decodeIfPresent(String.self, forKey: .stateDir)
        wireLogEnabled = try container.decode(Bool.self, forKey: .wireLogEnabled)
        activeSessions = try container.decode(UInt64.self, forKey: .activeSessions)
        activeTransfers = try container.decode(Int64.self, forKey: .activeTransfers)
        activeSubscriptions = try container.decodeIfPresent(Int.self, forKey: .activeSubscriptions) ?? 0
        capabilities = try container.decode([String].self, forKey: .capabilities)
    }
}
