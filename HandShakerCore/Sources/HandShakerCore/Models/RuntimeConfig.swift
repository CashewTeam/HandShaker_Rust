import Foundation

/// Runtime creation configuration — mirrors `FfiRuntimeConfig`
/// (`crates/handshaker-ffi/src/lib.rs`, `hs_runtime_create` contract).
/// All fields are optional; Rust defaults match `RuntimeConfig::default()`:
/// adb "adb", timeout 30s, heartbeat 10s, event capacity 1024, transfer
/// history capacity 64.
public struct RuntimeConfig: Codable, Sendable, Equatable {
    /// Path to the adb binary (default "adb").
    public var adbPathUTF8: String?
    /// Default per-call timeout in ms (default 30000).
    public var defaultTimeoutMs: UInt64?
    /// Heartbeat interval in ms (default 10000).
    public var heartbeatIntervalMs: UInt64?
    /// State directory (thumbnails cache, trust records, sync ledger...).
    public var stateDirUTF8: String?
    /// Optional wire-log path.
    public var wireLogUTF8: String?
    /// Event broadcast capacity (default 1024).
    public var eventCapacity: UInt32?
    /// Bounded finished-transfer history (default 64).
    public var transferHistoryCapacity: Int?
    /// TTL for finished transfers in ms; nil keeps them until eviction.
    public var transferHistoryTTLMs: UInt64?

    public init(
        adbPathUTF8: String? = nil,
        defaultTimeoutMs: UInt64? = nil,
        heartbeatIntervalMs: UInt64? = nil,
        stateDirUTF8: String? = nil,
        wireLogUTF8: String? = nil,
        eventCapacity: UInt32? = nil,
        transferHistoryCapacity: Int? = nil,
        transferHistoryTTLMs: UInt64? = nil
    ) {
        self.adbPathUTF8 = adbPathUTF8
        self.defaultTimeoutMs = defaultTimeoutMs
        self.heartbeatIntervalMs = heartbeatIntervalMs
        self.stateDirUTF8 = stateDirUTF8
        self.wireLogUTF8 = wireLogUTF8
        self.eventCapacity = eventCapacity
        self.transferHistoryCapacity = transferHistoryCapacity
        self.transferHistoryTTLMs = transferHistoryTTLMs
    }

    /// Rust-side defaults (`RuntimeConfig::default()` in dto.rs).
    public static let defaults = RuntimeConfig(
        adbPathUTF8: "adb",
        defaultTimeoutMs: 30_000,
        heartbeatIntervalMs: 10_000,
        eventCapacity: 1024,
        transferHistoryCapacity: 64
    )

    /// Serialize to the `hs_runtime_create` request JSON. Nil fields are
    /// omitted so Rust applies its defaults.
    public func jsonBody() -> String {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        // Explicit CodingKeys keep the FFI field names (with _utf8/_ms
        // suffixes) independent of any decoder strategy.
        guard let data = try? encoder.encode(self), let json = String(data: data, encoding: .utf8) else {
            return "{}"
        }
        return json
    }

    private enum CodingKeys: String, CodingKey {
        case adbPathUTF8 = "adb_path_utf8"
        case defaultTimeoutMs = "default_timeout_ms"
        case heartbeatIntervalMs = "heartbeat_interval_ms"
        case stateDirUTF8 = "state_dir_utf8"
        case wireLogUTF8 = "wire_log_utf8"
        case eventCapacity = "event_capacity"
        case transferHistoryCapacity = "transfer_history_capacity"
        case transferHistoryTTLMs = "transfer_history_ttl_ms"
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encodeIfPresent(adbPathUTF8, forKey: .adbPathUTF8)
        try container.encodeIfPresent(defaultTimeoutMs, forKey: .defaultTimeoutMs)
        try container.encodeIfPresent(heartbeatIntervalMs, forKey: .heartbeatIntervalMs)
        try container.encodeIfPresent(stateDirUTF8, forKey: .stateDirUTF8)
        try container.encodeIfPresent(wireLogUTF8, forKey: .wireLogUTF8)
        try container.encodeIfPresent(eventCapacity, forKey: .eventCapacity)
        try container.encodeIfPresent(transferHistoryCapacity, forKey: .transferHistoryCapacity)
        try container.encodeIfPresent(transferHistoryTTLMs, forKey: .transferHistoryTTLMs)
    }
}
