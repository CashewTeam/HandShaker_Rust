import Foundation
import HandShakerFFI

// MARK: - Request DTOs (FFI JSON contracts, handshaker_ffi.h)

/// `hs_trust_remove` request: `{"device_id":"phone:xxx"}`.
private struct TrustRemoveRequest: Encodable {
    let deviceID: DeviceID

    private enum CodingKeys: String, CodingKey {
        case deviceID = "device_id"
    }
}

/// `hs_trust_reset` request:
/// `{"endpoint":"192.168.1.5:5555","expected_device_id":"phone:xxx"}`.
private struct TrustResetRequest: Encodable {
    let endpoint: String
    let expectedDeviceID: DeviceID

    private enum CodingKeys: String, CodingKey {
        case endpoint
        case expectedDeviceID = "expected_device_id"
    }
}

// MARK: - Trust service

extension HandShakerRuntime {
    // MARK: Trust (no session)

    /// List locally persisted WiFi trust records (`hs_trust_list`, no
    /// request body).
    public func trustList() throws -> [TrustRecord] {
        try handle.withRuntime { runtime in
            try hsCall(as: [TrustRecord].self) {
                hs_trust_list(runtime)
            }
        }
    }

    /// Remove one trust record (`hs_trust_remove`, result `{"removed":true}`).
    public func trustRemove(deviceID: DeviceID) throws {
        let body = try ServicesJSON.encode(TrustRemoveRequest(deviceID: deviceID))
        try handle.withRuntime { runtime in
            try withHsRequestThrowing(body) { ptr, len in
                try hsCallVoid { hs_trust_remove(runtime, ptr, len) }
            }
        }
    }

    /// Reset a trust record — e.g. after a phone reinstall the old record
    /// must be dropped before reconnecting (`hs_trust_reset`, result
    /// `{"reset":true}`).
    ///
    /// - Parameters:
    ///   - endpoint: transport endpoint of the record, e.g. "192.168.1.5:5555".
    ///   - expectedDeviceID: the stable id the record must match.
    public func trustReset(endpoint: String, expectedDeviceID: DeviceID) throws {
        let body = try ServicesJSON.encode(
            TrustResetRequest(endpoint: endpoint, expectedDeviceID: expectedDeviceID)
        )
        try handle.withRuntime { runtime in
            try withHsRequestThrowing(body) { ptr, len in
                try hsCallVoid { hs_trust_reset(runtime, ptr, len) }
            }
        }
    }
}
