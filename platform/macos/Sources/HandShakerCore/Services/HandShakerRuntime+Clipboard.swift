import Foundation
import HandShakerFFI

// MARK: - Request DTOs (FFI JSON contracts, handshaker_ffi.h)

/// `hs_clipboard_set` request: `{"text":"..."}`.
private struct ClipboardSetRequest: Encodable {
    let text: String
}

/// `hs_clipboard_delete` request: `{"timestamp_ms":N}` (i64 in Rust).
private struct ClipboardDeleteRequest: Encodable {
    let timestampMs: Int64

    private enum CodingKeys: String, CodingKey {
        case timestampMs = "timestamp_ms"
    }
}

// MARK: - Clipboard service

extension HandShakerRuntime {
    // MARK: Clipboard

    /// List the phone's clipboard history (`hs_clipboard_list`, no request
    /// body; result is a JSON array of `ClipboardEntry`).
    public func clipboardList(sessionID: UInt64) async throws -> [ClipboardEntry] {
        try await callNative {
            try self.handle.withRuntime { runtime in
                try hsCall(as: [ClipboardEntry].self) {
                    hs_clipboard_list(runtime, sessionID)
                }
            }
        }
    }

    /// Push text onto the phone's clipboard (`hs_clipboard_set`, result
    /// `{"set":true}`).
    public func clipboardSet(sessionID: UInt64, text: String) async throws {
        let body = try ServicesJSON.encode(ClipboardSetRequest(text: text))
        try await callNative {
            try self.handle.withRuntime { runtime in
                try withHsRequestThrowing(body) { ptr, len in
                    try hsCallVoid { hs_clipboard_set(runtime, sessionID, ptr, len) }
                }
            }
        }
    }

    /// Delete one clipboard entry by its timestamp (`hs_clipboard_delete`,
    /// result `{"deleted":true}`).
    public func clipboardDelete(sessionID: UInt64, timestampMs: Int64) async throws {
        let body = try ServicesJSON.encode(ClipboardDeleteRequest(timestampMs: timestampMs))
        try await callNative {
            try self.handle.withRuntime { runtime in
                try withHsRequestThrowing(body) { ptr, len in
                    try hsCallVoid { hs_clipboard_delete(runtime, sessionID, ptr, len) }
                }
            }
        }
    }

    /// Clear the phone's clipboard history (`hs_clipboard_clear`, result
    /// `{"cleared":true}`).
    public func clipboardClear(sessionID: UInt64) async throws {
        try await callNative {
            try self.handle.withRuntime { runtime in
                try hsCallVoid { hs_clipboard_clear(runtime, sessionID) }
            }
        }
    }
}
