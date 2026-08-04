import Foundation
import HandShakerFFI

// MARK: - Transfer DTOs (FFI JSON contracts, handshaker_ffi.h)

/// A transfer id, as returned by `hs_transfer_start_*`
/// (`{"transfer_id": N}`).
public struct TransferID: Codable, Sendable, Equatable, Hashable {
    /// The raw numeric transfer id.
    public let value: UInt64

    public init(value: UInt64) {
        self.value = value
    }

    private enum CodingKeys: String, CodingKey {
        case value = "transfer_id"
    }
}

/// `hs_transfer_start_download` / `hs_transfer_start_upload` request:
/// `{"remote_path":...,"local_path":...,"overwrite":bool}` (overwrite
/// optional, default false).
private struct TransferStartRequest: Encodable {
    let remotePath: String
    let localPath: String
    let overwrite: Bool

    private enum CodingKeys: String, CodingKey {
        case remotePath = "remote_path"
        case localPath = "local_path"
        case overwrite
    }
}

/// `hs_transfer_start_batch_download` / `hs_transfer_start_batch_upload`
/// request: `{"files":[{"source","target"}],"trees":[...],"overwrite":bool}`
/// (all optional; empty arrays are equivalent to omitted).
private struct BatchTransferStartRequest: Encodable {
    let files: [BatchTransferItem]
    let trees: [TreeTransfer]
    let overwrite: Bool
}

// MARK: - Transfer service

extension HandShakerRuntime {
    // MARK: Transfers

    /// Start a single-file download (`hs_transfer_start_download`).
    ///
    /// - Parameters:
    ///   - sessionID: open session id.
    ///   - remotePath: remote source path.
    ///   - localPath: local destination path.
    ///   - overwrite: replace an existing local file (default false).
    /// - Returns: the new transfer id; poll progress with `transfer(_:)` or
    ///   observe `transfer_updated` events.
    public func startDownload(
        sessionID: UInt64,
        remotePath: String,
        localPath: String,
        overwrite: Bool = false
    ) async throws -> TransferID {
        let body = try ServicesJSON.encode(
            TransferStartRequest(remotePath: remotePath, localPath: localPath, overwrite: overwrite)
        )
        return try await callNative {
            try self.handle.withRuntime { runtime in
                try withHsRequestThrowing(body) { ptr, len in
                    try hsCall(as: TransferID.self) {
                        hs_transfer_start_download(runtime, sessionID, ptr, len)
                    }
                }
            }
        }
    }

    /// Start a single-file upload (`hs_transfer_start_upload`).
    ///
    /// - Parameters:
    ///   - sessionID: open session id.
    ///   - remotePath: remote destination path.
    ///   - localPath: local source path.
    ///   - overwrite: replace an existing remote file (default false).
    /// - Returns: the new transfer id.
    public func startUpload(
        sessionID: UInt64,
        remotePath: String,
        localPath: String,
        overwrite: Bool = false
    ) async throws -> TransferID {
        let body = try ServicesJSON.encode(
            TransferStartRequest(remotePath: remotePath, localPath: localPath, overwrite: overwrite)
        )
        return try await callNative {
            try self.handle.withRuntime { runtime in
                try withHsRequestThrowing(body) { ptr, len in
                    try hsCall(as: TransferID.self) {
                        hs_transfer_start_upload(runtime, sessionID, ptr, len)
                    }
                }
            }
        }
    }

    /// Start a batch download (`hs_transfer_start_batch_download`):
    /// individual `files` plus directory `trees` to mirror.
    ///
    /// - Parameters:
    ///   - sessionID: open session id.
    ///   - files: individual source/target pairs.
    ///   - trees: directory trees to mirror.
    ///   - overwrite: replace existing local files (default false).
    /// - Returns: the new batch transfer id; `TransferSnapshot` carries
    ///   `itemCount`/`completedItems`/`failedItems`/`batchResult`.
    public func startBatchDownload(
        sessionID: UInt64,
        files: [BatchTransferItem] = [],
        trees: [TreeTransfer] = [],
        overwrite: Bool = false
    ) async throws -> TransferID {
        let body = try ServicesJSON.encode(
            BatchTransferStartRequest(files: files, trees: trees, overwrite: overwrite)
        )
        return try await callNative {
            try self.handle.withRuntime { runtime in
                try withHsRequestThrowing(body) { ptr, len in
                    try hsCall(as: TransferID.self) {
                        hs_transfer_start_batch_download(runtime, sessionID, ptr, len)
                    }
                }
            }
        }
    }

    /// Start a batch upload (`hs_transfer_start_batch_upload`):
    /// individual `files` plus directory `trees` to mirror.
    ///
    /// - Parameters:
    ///   - sessionID: open session id.
    ///   - files: individual source/target pairs.
    ///   - trees: directory trees to mirror.
    ///   - overwrite: replace existing remote files (default false).
    /// - Returns: the new batch transfer id.
    public func startBatchUpload(
        sessionID: UInt64,
        files: [BatchTransferItem] = [],
        trees: [TreeTransfer] = [],
        overwrite: Bool = false
    ) async throws -> TransferID {
        let body = try ServicesJSON.encode(
            BatchTransferStartRequest(files: files, trees: trees, overwrite: overwrite)
        )
        return try await callNative {
            try self.handle.withRuntime { runtime in
                try withHsRequestThrowing(body) { ptr, len in
                    try hsCall(as: TransferID.self) {
                        hs_transfer_start_batch_upload(runtime, sessionID, ptr, len)
                    }
                }
            }
        }
    }

    /// Cancel a transfer (`hs_transfer_cancel`, result `{"cancelled":true}`).
    /// No-op for transfers already in a terminal state.
    public func cancelTransfer(_ id: UInt64) async throws {
        try await callNative {
            try self.handle.withRuntime { runtime in
                try hsCallVoid { hs_transfer_cancel(runtime, id) }
            }
        }
    }

    /// Snapshot of one transfer (`hs_transfer_get`). Throws
    /// `.transferNotFound` for unknown ids.
    public func transfer(_ id: UInt64) async throws -> TransferSnapshot {
        try await callNative {
            try self.handle.withRuntime { runtime in
                try hsCall(as: TransferSnapshot.self) {
                    hs_transfer_get(runtime, id)
                }
            }
        }
    }

    /// Snapshots of all transfers, including the bounded finished history
    /// (`hs_transfer_list`).
    public func transfers() async throws -> [TransferSnapshot] {
        try await callNative {
            try self.handle.withRuntime { runtime in
                try hsCall(as: [TransferSnapshot].self) {
                    hs_transfer_list(runtime)
                }
            }
        }
    }
}
