import Foundation

/// Backend event kinds (event.rs `BackendEvent`). Rust serializes with
/// `#[serde(tag = "kind", rename_all = "snake_case")]`: every variant's
/// fields are *inlined* next to the `kind` tag (verified by
/// `backend_event_change_payloads_serialize_with_stable_kinds` in
/// tests.rs), e.g.:
///
///     {"kind":"transfer_updated","id":7,"session_id":1,...}
///     {"kind":"warning","code":"...","message":"...","detail":null,...}
///
/// Unknown `kind` tokens decode safely as `.unknown(String)` instead of
/// failing, so a newer Rust library cannot break the Swift client.
public enum BackendEvent: Codable, Sendable, Equatable {
    case runtimeStarted
    case runtimeStopping
    /// Newtype payload inlined: the DeviceDescriptor fields sit next to
    /// "kind":"device_added".
    case deviceAdded(DeviceDescriptor)
    case deviceUpdated(sessionID: UInt64, device: DeviceDescriptor)
    case deviceRemoved(deviceID: DeviceID)
    /// Newtype payload inlined: SessionSnapshot fields.
    case sessionStateChanged(SessionSnapshot)
    /// Newtype payload inlined: TransferSnapshot fields.
    case transferUpdated(TransferSnapshot)
    case connectionLost(sessionID: UInt64)
    case clipboardChanged(sessionID: UInt64, entries: [ClipboardEntry])
    case mediaChanged(sessionID: UInt64, change: MediaChange)
    case remoteFileChanged(sessionID: UInt64, change: RemoteFileChange)
    /// Newtype payload inlined: SyncRunResult fields.
    case syncWatchApplied(SyncRunResult)
    /// Newtype payload inlined: PublicError fields.
    case warning(HandShakerNativeError)
    /// Forward compatibility: raw `kind` token of an unknown event.
    case unknown(String)

    private enum CodingKeys: String, CodingKey {
        case kind
        case sessionID = "session_id"
        case deviceID = "device_id"
        case entries
        case change
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let kind = try container.decode(String.self, forKey: .kind)
        switch kind {
        case "runtime_started":
            self = .runtimeStarted
        case "runtime_stopping":
            self = .runtimeStopping
        case "device_added":
            self = .deviceAdded(try DeviceDescriptor(from: decoder))
        case "device_updated":
            let sessionID = try container.decode(UInt64.self, forKey: .sessionID)
            self = .deviceUpdated(sessionID: sessionID, device: try DeviceDescriptor(from: decoder))
        case "device_removed":
            self = .deviceRemoved(deviceID: try container.decode(DeviceID.self, forKey: .deviceID))
        case "session_state_changed":
            self = .sessionStateChanged(try SessionSnapshot(from: decoder))
        case "transfer_updated":
            self = .transferUpdated(try TransferSnapshot(from: decoder))
        case "connection_lost":
            self = .connectionLost(sessionID: try container.decode(UInt64.self, forKey: .sessionID))
        case "clipboard_changed":
            self = .clipboardChanged(
                sessionID: try container.decode(UInt64.self, forKey: .sessionID),
                entries: try container.decode([ClipboardEntry].self, forKey: .entries)
            )
        case "media_changed":
            self = .mediaChanged(
                sessionID: try container.decode(UInt64.self, forKey: .sessionID),
                change: try container.decode(MediaChange.self, forKey: .change)
            )
        case "remote_file_changed":
            self = .remoteFileChanged(
                sessionID: try container.decode(UInt64.self, forKey: .sessionID),
                change: try container.decode(RemoteFileChange.self, forKey: .change)
            )
        case "sync_watch_applied":
            self = .syncWatchApplied(try SyncRunResult(from: decoder))
        case "warning":
            self = .warning(try HandShakerNativeError(from: decoder))
        default:
            self = .unknown(kind)
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .runtimeStarted:
            try container.encode("runtime_started", forKey: .kind)
        case .runtimeStopping:
            try container.encode("runtime_stopping", forKey: .kind)
        case .deviceAdded(let device):
            try container.encode("device_added", forKey: .kind)
            try device.encode(to: encoder)
        case .deviceUpdated(let sessionID, let device):
            try container.encode("device_updated", forKey: .kind)
            try container.encode(sessionID, forKey: .sessionID)
            try device.encode(to: encoder)
        case .deviceRemoved(let deviceID):
            try container.encode("device_removed", forKey: .kind)
            try container.encode(deviceID, forKey: .deviceID)
        case .sessionStateChanged(let snapshot):
            try container.encode("session_state_changed", forKey: .kind)
            try snapshot.encode(to: encoder)
        case .transferUpdated(let snapshot):
            try container.encode("transfer_updated", forKey: .kind)
            try snapshot.encode(to: encoder)
        case .connectionLost(let sessionID):
            try container.encode("connection_lost", forKey: .kind)
            try container.encode(sessionID, forKey: .sessionID)
        case .clipboardChanged(let sessionID, let entries):
            try container.encode("clipboard_changed", forKey: .kind)
            try container.encode(sessionID, forKey: .sessionID)
            try container.encode(entries, forKey: .entries)
        case .mediaChanged(let sessionID, let change):
            try container.encode("media_changed", forKey: .kind)
            try container.encode(sessionID, forKey: .sessionID)
            try container.encode(change, forKey: .change)
        case .remoteFileChanged(let sessionID, let change):
            try container.encode("remote_file_changed", forKey: .kind)
            try container.encode(sessionID, forKey: .sessionID)
            try container.encode(change, forKey: .change)
        case .syncWatchApplied(let result):
            try container.encode("sync_watch_applied", forKey: .kind)
            try result.encode(to: encoder)
        case .warning(let error):
            try container.encode("warning", forKey: .kind)
            try error.encode(to: encoder)
        case .unknown(let kind):
            try container.encode(kind, forKey: .kind)
        }
    }
}

/// One delivered event with monotonic sequencing (event.rs `EventEnvelope`).
public struct EventEnvelope: Codable, Sendable, Equatable {
    public let sequence: UInt64
    public let timestampMs: UInt64
    public let event: BackendEvent

    private enum CodingKeys: String, CodingKey {
        case sequence
        case timestampMs = "timestamp_ms"
        case event
    }
}

/// Category of a phone-initiated remote file change
/// (dto.rs `RemoteFileChangeKind`, snake_case).
public enum RemoteFileChangeKind: String, Codable, Sendable, Equatable {
    /// A directory monitor event.
    case directoryChanged = "directory_changed"
    /// A synchronization file change.
    case fileChanged = "file_changed"
    /// A one-shot photo synchronization response.
    case photoSyncChanged = "photo_sync_changed"
    /// A synchronization monitor response.
    case syncMonitorChanged = "sync_monitor_changed"
    /// Forward compatibility: unknown kind tokens decode safely.
    case unknown

    public init(from decoder: Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = Self(rawValue: raw) ?? .unknown
    }
}

/// Summarized remote file change (dto.rs `RemoteFileChangeDto`).
/// `files`/`statuses` carry `#[serde(default, skip_serializing_if =
/// "Vec::is_empty")]` in Rust and may be absent from JSON — Swift
/// optionals tolerate that.
public struct RemoteFileChange: Codable, Sendable, Equatable {
    public let changeKind: RemoteFileChangeKind
    public let paths: [String]
    /// Full metadata for each changed path, parallel to `paths` when the
    /// phone supplied it (absent/empty when only paths are known).
    public let files: [FileEntry]?
    /// Per-path `FileChangeStatus` snake_case strings (e.g. "added",
    /// "deleted", "modified"), parallel to `paths`.
    public let statuses: [String]?

    private enum CodingKeys: String, CodingKey {
        case changeKind = "change_kind"
        case paths
        case files
        case statuses
    }
}
