import Foundation

/// One locally persisted WiFi trust record (trust.rs `TrustRecordDto`).
public struct TrustRecord: Codable, Sendable, Equatable {
    /// Stable device id ("phone:<uuid>"), matching the reconciled identity
    /// used by connected sessions.
    public let deviceID: DeviceID
    public let deviceName: String?
    /// Last successful trust, Unix milliseconds.
    public let updatedAtMs: UInt64

    private enum CodingKeys: String, CodingKey {
        case deviceID = "device_id"
        case deviceName = "device_name"
        case updatedAtMs = "updated_at_ms"
    }
}
