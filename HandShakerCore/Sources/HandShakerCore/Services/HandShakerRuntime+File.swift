import Foundation
import HandShakerFFI

// MARK: - Request DTOs (FFI JSON contracts, handshaker_ffi.h)

/// `hs_list_files` request: `{"path":...,"depth":N}` (depth optional,
/// default 1).
private struct ListFilesRequest: Encodable {
    let path: String
    let depth: UInt32
}

/// `hs_stat_file` request: `{"path":...}` (path optional, default ".").
private struct StatFileRequest: Encodable {
    let path: String
}

/// `hs_stat_file` result: `{"file": FileEntryDto|null}`.
private struct FileStatResult: Decodable {
    let file: FileEntry?
}

/// `hs_count_files` request: `{"path":...,"depth":N,"exclusions":[...]}`
/// (all optional; depth default 1).
private struct CountFilesRequest: Encodable {
    let path: String
    let depth: UInt32
    let exclusions: [String]
}

/// `hs_create_directory` request: `{"path":...}`.
private struct CreateDirectoryRequest: Encodable {
    let path: String
}

/// `hs_move_path` request: `{"source":...,"target":...}`.
private struct MovePathRequest: Encodable {
    let source: String
    let target: String
}

/// `hs_delete_paths` request: `{"paths":[...],"trash":bool,"sync":bool}`
/// (trash/sync optional, default false).
private struct DeletePathsRequest: Encodable {
    let paths: [String]
    let trash: Bool
    let sync: Bool
}

/// `hs_update_file_info` request: `{"files":[...],"is_sync":bool}` (both
/// optional; session id always comes from the call argument).
private struct UpdateFileInfoRequest: Encodable {
    let files: [UpdateFileInfoItem]
    let isSync: Bool

    private enum CodingKeys: String, CodingKey {
        case files
        case isSync = "is_sync"
    }
}

// MARK: - File service

extension HandShakerRuntime {
    // MARK: Files

    /// List one directory level (`hs_list_files`).
    ///
    /// - Parameters:
    ///   - sessionID: open session id.
    ///   - path: absolute remote path (relative paths resolve against the
    ///     device root).
    ///   - depth: recursion depth (0 = one level).
    public func listFiles(sessionID: UInt64, path: String, depth: UInt32 = 1) throws -> [FileEntry] {
        let body = try ServicesJSON.encode(ListFilesRequest(path: path, depth: depth))
        return try handle.withRuntime { runtime in
            try withHsRequestThrowing(body) { ptr, len in
                try hsCall(as: [FileEntry].self) {
                    hs_list_files(runtime, sessionID, ptr, len)
                }
            }
        }
    }

    /// Stat one remote path (`hs_stat_file`).
    ///
    /// - Returns: the entry, or `nil` when the path does not exist
    ///   (`{"file":null}`).
    public func statFile(sessionID: UInt64, path: String) throws -> FileEntry? {
        let body = try ServicesJSON.encode(StatFileRequest(path: path))
        let result = try handle.withRuntime { runtime in
            try withHsRequestThrowing(body) { ptr, len in
                try hsCall(as: FileStatResult.self) {
                    hs_stat_file(runtime, sessionID, ptr, len)
                }
            }
        }
        return result.file
    }

    /// Count files under a remote directory (`hs_count_files`, result
    /// `{"count":N}`). `exclusions` are the protocol exclusion patterns
    /// (`SspGetFileCountRequest.exclusion_pattern`).
    public func countFiles(
        sessionID: UInt64,
        path: String,
        depth: UInt32 = 1,
        exclusions: [String] = []
    ) throws -> UInt64 {
        let body = try ServicesJSON.encode(
            CountFilesRequest(path: path, depth: depth, exclusions: exclusions)
        )
        let result = try handle.withRuntime { runtime in
            try withHsRequestThrowing(body) { ptr, len in
                try hsCall(as: CountResult.self) {
                    hs_count_files(runtime, sessionID, ptr, len)
                }
            }
        }
        return result.count
    }

    /// Create one remote directory (`hs_create_directory`, result
    /// `{"created":true}`).
    public func createDirectory(sessionID: UInt64, path: String) throws {
        let body = try ServicesJSON.encode(CreateDirectoryRequest(path: path))
        try handle.withRuntime { runtime in
            try withHsRequestThrowing(body) { ptr, len in
                try hsCallVoid { hs_create_directory(runtime, sessionID, ptr, len) }
            }
        }
    }

    /// Move/rename a remote path (`hs_move_path`, result `{"moved":true}`).
    public func movePath(sessionID: UInt64, source: String, target: String) throws {
        let body = try ServicesJSON.encode(MovePathRequest(source: source, target: target))
        try handle.withRuntime { runtime in
            try withHsRequestThrowing(body) { ptr, len in
                try hsCallVoid { hs_move_path(runtime, sessionID, ptr, len) }
            }
        }
    }

    /// Delete remote paths (`hs_delete_paths`).
    ///
    /// - Parameters:
    ///   - paths: absolute remote paths to delete.
    ///   - trash: move to trash when the phone supports it.
    ///   - sync: mark the request as part of synchronization.
    /// - Returns: the entries the phone confirmed deleted
    ///   (`DeleteResultDto`).
    public func deletePaths(
        sessionID: UInt64,
        _ paths: [String],
        trash: Bool = false,
        sync: Bool = false
    ) throws -> DeleteResult {
        let body = try ServicesJSON.encode(
            DeletePathsRequest(paths: paths, trash: trash, sync: sync)
        )
        return try handle.withRuntime { runtime in
            try withHsRequestThrowing(body) { ptr, len in
                try hsCall(as: DeleteResult.self) {
                    hs_delete_paths(runtime, sessionID, ptr, len)
                }
            }
        }
    }

    /// Write file metadata back into the phone media store
    /// (`hs_update_file_info`, result `{"updated":true}`). The phone writes
    /// the reported fields back; `isSync` feeds the change into its sync
    /// manager.
    public func updateFileInfo(
        sessionID: UInt64,
        _ items: [UpdateFileInfoItem],
        isSync: Bool = false
    ) throws {
        let body = try ServicesJSON.encode(
            UpdateFileInfoRequest(files: items, isSync: isSync)
        )
        try handle.withRuntime { runtime in
            try withHsRequestThrowing(body) { ptr, len in
                try hsCallVoid { hs_update_file_info(runtime, sessionID, ptr, len) }
            }
        }
    }
}
