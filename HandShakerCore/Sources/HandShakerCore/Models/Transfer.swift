import Foundation

/// Fixed transfer states, one-way transitions: queued → running →
/// completed|failed|cancelled (transfer.rs `TransferState`, snake_case).
public enum TransferState: String, Codable, Sendable, Equatable {
    case queued
    case running
    case completed
    case failed
    case cancelled
    /// Forward compatibility: unknown state tokens decode safely.
    case unknown

    public init(from decoder: Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = Self(rawValue: raw) ?? .unknown
    }
}

/// Transfer direction for snapshots (transfer.rs `TransferDirectionDto`).
public enum TransferDirection: String, Codable, Sendable, Equatable {
    case download
    case upload
    /// Forward compatibility: unknown direction tokens decode safely.
    case unknown

    public init(from decoder: Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = Self(rawValue: raw) ?? .unknown
    }
}

/// UI-ready transfer snapshot (transfer.rs `TransferSnapshot`).
/// The Phase E fields (`item_count`, `completed_items`, `failed_items`,
/// `current_item`, `batch_result`) carry `#[serde(default)]` in Rust and
/// may be absent from legacy JSON — the custom `init(from:)` defaults them
/// instead of failing.
public struct TransferSnapshot: Codable, Sendable, Equatable {
    public let id: UInt64
    public let sessionID: UInt64
    public let direction: TransferDirection
    public let source: String
    public let destination: String
    public let state: TransferState
    public let transferredBytes: UInt64
    public let totalBytes: UInt64?
    public let startedAtMs: UInt64?
    public let finishedAtMs: UInt64?
    public let error: HandShakerNativeError?
    /// Planned item count (files + trees) of a batch transfer; 0 for
    /// single-file transfers.
    public let itemCount: UInt64
    public let completedItems: UInt64
    public let failedItems: UInt64
    /// Source path of the item currently being processed.
    public let currentItem: String?
    /// Aggregated per-item result, attached before the terminal transition.
    public let batchResult: BatchTransferResult?

    private enum CodingKeys: String, CodingKey {
        case id
        case sessionID = "session_id"
        case direction
        case source
        case destination
        case state
        case transferredBytes = "transferred_bytes"
        case totalBytes = "total_bytes"
        case startedAtMs = "started_at_ms"
        case finishedAtMs = "finished_at_ms"
        case error
        case itemCount = "item_count"
        case completedItems = "completed_items"
        case failedItems = "failed_items"
        case currentItem = "current_item"
        case batchResult = "batch_result"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(UInt64.self, forKey: .id)
        sessionID = try container.decode(UInt64.self, forKey: .sessionID)
        direction = try container.decode(TransferDirection.self, forKey: .direction)
        source = try container.decode(String.self, forKey: .source)
        destination = try container.decode(String.self, forKey: .destination)
        state = try container.decode(TransferState.self, forKey: .state)
        transferredBytes = try container.decode(UInt64.self, forKey: .transferredBytes)
        totalBytes = try container.decodeIfPresent(UInt64.self, forKey: .totalBytes)
        startedAtMs = try container.decodeIfPresent(UInt64.self, forKey: .startedAtMs)
        finishedAtMs = try container.decodeIfPresent(UInt64.self, forKey: .finishedAtMs)
        error = try container.decodeIfPresent(HandShakerNativeError.self, forKey: .error)
        // Rust marks these `#[serde(default)]`: tolerate missing keys.
        itemCount = try container.decodeIfPresent(UInt64.self, forKey: .itemCount) ?? 0
        completedItems = try container.decodeIfPresent(UInt64.self, forKey: .completedItems) ?? 0
        failedItems = try container.decodeIfPresent(UInt64.self, forKey: .failedItems) ?? 0
        currentItem = try container.decodeIfPresent(String.self, forKey: .currentItem)
        batchResult = try container.decodeIfPresent(BatchTransferResult.self, forKey: .batchResult)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(id, forKey: .id)
        try container.encode(sessionID, forKey: .sessionID)
        try container.encode(direction, forKey: .direction)
        try container.encode(source, forKey: .source)
        try container.encode(destination, forKey: .destination)
        try container.encode(state, forKey: .state)
        try container.encode(transferredBytes, forKey: .transferredBytes)
        try container.encodeIfPresent(totalBytes, forKey: .totalBytes)
        try container.encodeIfPresent(startedAtMs, forKey: .startedAtMs)
        try container.encodeIfPresent(finishedAtMs, forKey: .finishedAtMs)
        try container.encodeIfPresent(error, forKey: .error)
        try container.encode(itemCount, forKey: .itemCount)
        try container.encode(completedItems, forKey: .completedItems)
        try container.encode(failedItems, forKey: .failedItems)
        try container.encodeIfPresent(currentItem, forKey: .currentItem)
        try container.encodeIfPresent(batchResult, forKey: .batchResult)
    }
}

/// One source/target pair in a batch transfer (transfer.rs
/// `BatchTransferItemDto`).
public struct BatchTransferItem: Codable, Sendable, Equatable {
    public let source: String
    public let target: String
}

/// One directory tree to mirror (transfer.rs `TreeTransferDto`).
public struct TreeTransfer: Codable, Sendable, Equatable {
    public let source: String
    public let target: String
}

/// One failed item with its error message (transfer.rs
/// `TransferFailureDto`).
public struct TransferFailure: Codable, Sendable, Equatable {
    public let source: String
    public let target: String
    public let message: String
}

/// Aggregated batch result (transfer.rs `BatchTransferResultDto`).
public struct BatchTransferResult: Codable, Sendable, Equatable {
    public let ok: [BatchTransferItem]
    public let failures: [TransferFailure]
}
