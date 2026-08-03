import XCTest
@testable import HandShakerCore

/// Native error JSON decoding and HandShakerError mapping
/// (PublicError shape, crates/handshaker-application/src/error.rs).
final class NativeErrorTests: XCTestCase {
    private let decoder = JSONDecoder()

    func testPublicErrorJSONDecodes() throws {
        // Canonical PublicError JSON (result.rs `err` serialization).
        let json = """
        {"code":"session_not_found","message":"session not found","detail":null,\
        "retryable":false,"operation":"ping"}
        """
        let error = try decoder.decode(HandShakerNativeError.self, from: Data(json.utf8))
        XCTAssertEqual(error.code, "session_not_found")
        XCTAssertEqual(error.message, "session not found")
        XCTAssertNil(error.detail)
        XCTAssertFalse(error.retryable)
        XCTAssertEqual(error.operation, "ping")
    }

    func testPublicErrorWithDetailDecodes() throws {
        let json = """
        {"code":"remote_io","message":"write failed","detail":"disk full",\
        "retryable":true,"operation":"download"}
        """
        let error = try decoder.decode(HandShakerNativeError.self, from: Data(json.utf8))
        XCTAssertEqual(error.code, "remote_io")
        XCTAssertEqual(error.detail, "disk full")
        XCTAssertTrue(error.retryable)
    }

    func testFromNativeMapsKnownTokens() {
        // Direct equality checks (cases carry the message).
        XCTAssertEqual(HandShakerError.fromNative(HandShakerNativeError(code: "invalid_argument", message: "m")), .invalidArgument("m"))
        XCTAssertEqual(HandShakerError.fromNative(HandShakerNativeError(code: "session_not_found", message: "m")), .sessionNotFound("m"))
        XCTAssertEqual(HandShakerError.fromNative(HandShakerNativeError(code: "connection_lost", message: "m")), .connectionLost("m"))
        XCTAssertEqual(HandShakerError.fromNative(HandShakerNativeError(code: "transfer_cancelled", message: "m")), .transferCancelled("m"))
        XCTAssertEqual(HandShakerError.fromNative(HandShakerNativeError(code: "trust_rejected", message: "m")), .trustRejected("m"))
        XCTAssertEqual(HandShakerError.fromNative(HandShakerNativeError(code: "remote_io", message: "m")), .remoteIO("m"))
        XCTAssertEqual(HandShakerError.fromNative(HandShakerNativeError(code: "internal", message: "m")), .internal("m"))
    }

    func testFromNativeUnknownTokenFallsBack() {
        let native = HandShakerNativeError(
            code: "brand_new_code_2026",
            message: "from the future",
            detail: "d",
            retryable: true,
            operation: "op"
        )
        XCTAssertEqual(HandShakerError.fromNative(native), .unknown("brand_new_code_2026"))
    }

    func testAllRustTokensMapAwayFromUnknown() {
        // Every token in PublicErrorCode::as_str must map to a typed case,
        // never to .unknown.
        let tokens = [
            "runtime_closed", "invalid_argument", "invalid_state", "not_found",
            "device_not_found", "device_unavailable", "connect_failed",
            "connection_lost", "session_not_found", "session_closed",
            "trust_required", "trust_rejected", "remote_path_not_found",
            "remote_permission_denied", "remote_path_exists", "remote_io",
            "local_path_not_found", "local_permission_denied", "local_path_exists",
            "transfer_not_found", "transfer_cancelled", "remote_cancelled",
            "protocol_error", "decode_error", "adb_unavailable",
            "adb_unauthorized", "adb_offline", "wifi_discovery_failed",
            "usb_unavailable", "media_error", "clipboard_error", "sync_error",
            "internal",
        ]
        for token in tokens {
            let mapped = HandShakerError.fromNative(HandShakerNativeError(code: token, message: "m"))
            guard case .unknown = mapped else { continue }
            XCTFail("token \(token) mapped to .unknown")
        }
    }
}
