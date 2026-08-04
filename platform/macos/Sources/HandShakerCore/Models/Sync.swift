import Foundation

/// One sync profile: what to sync, where, and over which session
/// (sync.rs `SyncProfileDto`; `hs_sync_plan`/`hs_sync_start` request body).
public struct SyncProfile: Codable, Sendable, Equatable {
    /// Stable caller-chosen id; also the sync-jobs registry key.
    public let id: String
    public let sessionID: UInt64
    /// Stable phone identifier keying the ledger file.
    public let deviceUUID: String
    /// Phone-side root folder to sync.
    public let remoteRoot: String
    /// Local destination directory for downloaded files.
    public let localRoot: String
    public let enabled: Bool

    private enum CodingKeys: String, CodingKey {
        case id
        case sessionID = "session_id"
        case deviceUUID = "device_uuid"
        case remoteRoot = "remote_root"
        case localRoot = "local_root"
        case enabled
    }
}

/// Preview of one sync run (sync.rs `SyncPlanDto`).
public struct SyncPlan: Codable, Sendable, Equatable {
    public let profileID: String
    public let downloads: [SyncAction]
    public let metadataUpdates: [SyncAction]
    public let deletions: [SyncAction]
    public let conflicts: [SyncConflict]
    public let totalBytes: UInt64
    /// `false` when local conflicts would be clobbered; such a plan must
    /// not be executed.
    public let executable: Bool

    private enum CodingKeys: String, CodingKey {
        case profileID = "profile_id"
        case downloads
        case metadataUpdates = "metadata_updates"
        case deletions
        case conflicts
        case totalBytes = "total_bytes"
        case executable
    }
}

/// One planned file action (sync.rs `SyncActionDto`).
public struct SyncAction: Codable, Sendable, Equatable {
    public let remotePath: String
    public let localPath: String
    public let size: UInt64

    private enum CodingKeys: String, CodingKey {
        case remotePath = "remote_path"
        case localPath = "local_path"
        case size
    }
}

/// One local conflict: the local file was preserved, not overwritten
/// (sync.rs `SyncConflictDto`).
public struct SyncConflict: Codable, Sendable, Equatable {
    public let remotePath: String
    public let localPath: String
    /// Stable token explaining the conflict ("local_modified").
    public let reason: String

    private enum CodingKeys: String, CodingKey {
        case remotePath = "remote_path"
        case localPath = "local_path"
        case reason
    }
}

/// Live status of a registered sync job (sync.rs `SyncStatusDto`).
public struct SyncStatus: Codable, Sendable, Equatable {
    public let profileID: String
    public let running: Bool
    public let monitoring: Bool
    public let lastRunAtMs: UInt64?
    public let lastError: HandShakerNativeError?
    /// P1-2: a lag/apply failure requires a full sync before watching
    /// again; decodes with a default so older payloads still work.
    public let reconciliationRequired: Bool
    public let lastSequenceGap: UInt64?

    private enum CodingKeys: String, CodingKey {
        case profileID = "profile_id"
        case running
        case monitoring
        case lastRunAtMs = "last_run_at_ms"
        case lastError = "last_error"
        case reconciliationRequired = "reconciliation_required"
        case lastSequenceGap = "last_sequence_gap"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        profileID = try container.decode(String.self, forKey: .profileID)
        running = try container.decode(Bool.self, forKey: .running)
        monitoring = try container.decode(Bool.self, forKey: .monitoring)
        lastRunAtMs = try container.decodeIfPresent(UInt64.self, forKey: .lastRunAtMs)
        lastError = try container.decodeIfPresent(HandShakerNativeError.self, forKey: .lastError)
        // Newer Rust fields with safe defaults for older payloads.
        reconciliationRequired =
            try container.decodeIfPresent(Bool.self, forKey: .reconciliationRequired) ?? false
        lastSequenceGap =
            try container.decodeIfPresent(UInt64.self, forKey: .lastSequenceGap)
    }
}

/// Result of one executed sync run (sync.rs `SyncRunResultDto`; also the
/// `SyncWatchApplied` event payload, inlined next to the event `kind`).
public struct SyncRunResult: Codable, Sendable, Equatable {
    public let downloaded: [String]
    public let deleted: [String]
    public let failures: [String]
    public let conflicts: [String]
}

/// Ledger summary for the `sync status` command (sync.rs
/// `SyncLedgerStatusDto`; round-2 P0-1 added the scope roots).
public struct SyncLedgerStatus: Codable, Sendable, Equatable {
    public let deviceUUID: String
    /// Normalized remote root of the ledger scope (absent in legacy JSON).
    public let remoteRoot: String?
    /// Normalized local root of the ledger scope (absent in legacy JSON).
    public let localRoot: String?
    public let files: UInt64
    public let bytes: UInt64

    private enum CodingKeys: String, CodingKey {
        case deviceUUID = "device_uuid"
        case remoteRoot = "remote_root"
        case localRoot = "local_root"
        case files
        case bytes
    }
}
