import Foundation
import HandShakerFFI

/// Verify the loaded native library satisfies the minimum ABI this SDK was
/// built against: major == 1 and minor >= 5 (ABI 1.5.0 adds update file
/// info and the pure media merge; 1.4 added the photo-sync surface).
///
/// Call once at startup, before any runtime is created. Throws
/// `HandShakerError.unsupported` with the actual ABI version when the
/// library is too old (or reports an incompatible major).
public func checkABI() throws {
    let major = hs_abi_version_major()
    let minor = hs_abi_version_minor()
    let patch = hs_abi_version_patch()
    guard major == 1, minor >= 5 else {
        throw HandShakerError.unsupported(
            "incompatible HandShakerFFI ABI \(major).\(minor).\(patch): this SDK requires ABI 1.5.x"
        )
    }
}
