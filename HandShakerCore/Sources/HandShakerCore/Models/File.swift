import Foundation

/// One directory entry (dto.rs `FileEntryDto`).
public struct FileEntry: Codable, Sendable, Equatable {
    public let path: String
    public let size: UInt64
    public let createdAtMs: UInt64?
    public let modifiedAtMs: UInt64?
    public let isDirectory: Bool
    public let checksum: String?
    public let isTrash: Bool?
    public let mediaID: UInt64?

    private enum CodingKeys: String, CodingKey {
        case path
        case size
        case createdAtMs = "created_at_ms"
        case modifiedAtMs = "modified_at_ms"
        case isDirectory = "is_directory"
        case checksum
        case isTrash = "is_trash"
        case mediaID = "media_id"
    }
}

/// Result of a delete request (dto.rs `DeleteResultDto`): the entries the
/// phone confirmed deleted.
public struct DeleteResult: Codable, Sendable, Equatable {
    public let deleted: [FileEntry]
}

/// Result of `hs_count_files` (header: `{"count": N}`).
public struct CountResult: Codable, Sendable, Equatable {
    public let count: UInt64
}

/// One file whose metadata should be written back to the phone media store
/// (dto.rs `UpdateFileInfoItemDto`; `hs_update_file_info` request items).
public struct UpdateFileInfoItem: Codable, Sendable, Equatable {
    /// Absolute remote path.
    public let path: String
    /// File size in bytes.
    public let size: UInt64
    public let createdAt: UInt64?
    public let modifiedAt: UInt64?
    public let isDirectory: Bool
    public let checksum: String?
    public let isTrash: Bool?
    /// Media-store identifier, when available.
    public let id: UInt64?
    /// Phone-side extension data (JSON), opaque to the host.
    public let extData: String?

    private enum CodingKeys: String, CodingKey {
        case path
        case size
        case createdAt = "created_at"
        case modifiedAt = "modified_at"
        case isDirectory = "is_directory"
        case checksum
        case isTrash = "is_trash"
        case id
        case extData = "ext_data"
    }
}
