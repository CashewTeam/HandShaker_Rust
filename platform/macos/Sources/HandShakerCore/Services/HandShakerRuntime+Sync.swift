import Foundation
import HandShakerFFI

// MARK: - Response DTO (FFI JSON contract, handshaker_ffi.h)

/// `hs_sync_start` result: `{"profile_id":"<id>"}`.
private struct ProfileIDResult: Decodable {
    let profileID: String

    private enum CodingKeys: String, CodingKey {
        case profileID = "profile_id"
    }
}

// MARK: - Photo sync service

extension HandShakerRuntime {
    // MARK: Photo sync

    /// Preview one sync run without executing it (`hs_sync_plan`).
    /// The session id always comes from the call argument; `profile`'s
    /// `id`/`remote_root`/`enabled` may be defaulted by the Rust side.
    public func syncPlan(sessionID: UInt64, profile: SyncProfile) async throws -> SyncPlan {
        let body = try ServicesJSON.encode(profile)
        return try await callNative {
            try self.handle.withRuntime { runtime in
                try withHsRequestThrowing(body) { ptr, len in
                    try hsCall(as: SyncPlan.self) {
                        hs_sync_plan(runtime, sessionID, ptr, len)
                    }
                }
            }
        }
    }

    /// Launch a sync run in the background (`hs_sync_start`).
    ///
    /// - Returns: the profile id (the `id` chosen in `profile`, or the
    ///   defaulted `device_uuid`), used for `syncStatus`/`syncStop`/watch.
    public func syncStart(sessionID: UInt64, profile: SyncProfile) async throws -> String {
        let body = try ServicesJSON.encode(profile)
        let result = try await callNative {
            try self.handle.withRuntime { runtime in
                try withHsRequestThrowing(body) { ptr, len in
                    try hsCall(as: ProfileIDResult.self) {
                        hs_sync_start(runtime, sessionID, ptr, len)
                    }
                }
            }
        }
        return result.profileID
    }

    /// Live status of a registered sync job (`hs_sync_status`; profile id
    /// is a C-string argument). Throws `.notFound` for an unknown id.
    public func syncStatus(profileID: String) async throws -> SyncStatus {
        try await callNative {
            try self.handle.withRuntime { runtime in
                try withHsString(profileID) { ptr, len in
                    try hsCall(as: SyncStatus.self) {
                        hs_sync_status(runtime, ptr, len)
                    }
                }
            }
        }
    }

    /// Stop a running sync job (`hs_sync_stop`, result `{"stopped":true}`).
    /// Throws `.notFound` for an unknown profile id.
    public func syncStop(profileID: String) async throws {
        try await callNative {
            try self.handle.withRuntime { runtime in
                try withHsString(profileID) { ptr, len in
                    try hsCallVoid { hs_sync_stop(runtime, ptr, len) }
                }
            }
        }
    }

    /// Start watch mode for a finished sync run (`hs_sync_start_watch`,
    /// result `{"started":true}`): debounced phone changes are applied and
    /// delivered as `SyncWatchApplied` events. Requires the phone to be in
    /// the SYNCING state (poll `syncStatus` until `running == false`).
    public func syncStartWatch(profileID: String) async throws {
        try await callNative {
            try self.handle.withRuntime { runtime in
                try withHsString(profileID) { ptr, len in
                    try hsCallVoid { hs_sync_start_watch(runtime, ptr, len) }
                }
            }
        }
    }

    /// Stop watch mode (`hs_sync_stop_watch`, result `{"stopped":true}`).
    public func syncStopWatch(profileID: String) async throws {
        try await callNative {
            try self.handle.withRuntime { runtime in
                try withHsString(profileID) { ptr, len in
                    try hsCallVoid { hs_sync_stop_watch(runtime, ptr, len) }
                }
            }
        }
    }
}
