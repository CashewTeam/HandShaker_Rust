import Foundation
import HandShakerFFI

// MARK: - Request DTO (FFI JSON contract, handshaker_ffi.h)

/// `hs_monitor_folder` request: `{"path":"/sdcard/DCIM","enabled":true}`
/// (enabled optional, default true). Directory-change events arrive as
/// `RemoteFileChanged` on the event stream.
private struct MonitorFolderRequest: Encodable {
    let path: String
    let enabled: Bool
}

// MARK: - Directory monitor service

extension HandShakerRuntime {
    // MARK: Directory monitor

    /// Register (or unregister) a directory monitor on the phone
    /// (`hs_monitor_folder`, result `{"registered":true}`). While enabled,
    /// changes under `path` are delivered as `remote_file_changed` events
    /// on the `eventStream()`.
    ///
    /// - Parameters:
    ///   - sessionID: open session id.
    ///   - path: absolute remote directory to watch.
    ///   - enabled: `true` registers, `false` unregisters (default true).
    public func monitorFolder(sessionID: UInt64, path: String, enabled: Bool = true) async throws {
        let body = try ServicesJSON.encode(MonitorFolderRequest(path: path, enabled: enabled))
        try await callNative {
            try self.handle.withRuntime { runtime in
                try withHsRequestThrowing(body) { ptr, len in
                    try hsCallVoid { hs_monitor_folder(runtime, sessionID, ptr, len) }
                }
            }
        }
    }
}
