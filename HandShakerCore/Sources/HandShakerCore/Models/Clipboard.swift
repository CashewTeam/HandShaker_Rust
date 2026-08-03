import Foundation

/// One clipboard entry (dto.rs `ClipboardEntryDto`; `timestamp_ms` is an
/// i64 in Rust — Swift `Int64`).
public struct ClipboardEntry: Codable, Sendable, Equatable {
    public let text: String
    public let timestampMs: Int64

    private enum CodingKeys: String, CodingKey {
        case text
        case timestampMs = "timestamp_ms"
    }
}
