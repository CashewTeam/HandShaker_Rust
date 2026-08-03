import XCTest
import HandShakerFFI
@testable import HandShakerCore

/// ABI version checks against the real native library (HandShakerFFI
/// XCFramework; ABI 1.5.0).
final class ABITests: XCTestCase {
    func testCheckABISucceeds() throws {
        // The bundled library is ABI 1.5.0 (major 1, minor 5) — the check
        // must pass.
        try checkABI()
    }

    func testABIVersionNumbers() {
        XCTAssertEqual(hs_abi_version_major(), 1)
        XCTAssertGreaterThanOrEqual(hs_abi_version_minor(), 5)
    }
}
