// Minimal Swift smoke test for the handshaker-ffi C ABI.
// Build (from repo root):
//   swiftc -I crates/handshaker-ffi/include scripts/ffi_smoke.swift \
//          -L target/release -lhandshaker_ffi -o /tmp/ffi_smoke_swift
// Run: /tmp/ffi_smoke_swift  (expect: "swift ffi smoke ok"; exit 0)
import Foundation
import HandShakerFFI

/// RAII wrapper: destroys the native runtime on deinit.
final class RuntimeHandle {
    private let ptr: OpaquePointer
    init() throws {
        var out: OpaquePointer?
        let cfg = "{}"
        let result = cfg.withCString { cstr in
            hs_runtime_create(UnsafePointer<UInt8>(OpaquePointer(cstr)), cfg.utf8.count, &out)
        }
        guard result.status == 0, let out else {
            throw NSError(domain: "ffi", code: 1)
        }
        ptr = out
        hs_call_result_free(result)
    }
    func shutdown() throws {
        let result = hs_runtime_shutdown(ptr)
        guard result.status == 0 else { throw NSError(domain: "ffi", code: 2) }
        hs_call_result_free(result)
    }
    deinit {
        hs_runtime_destroy(ptr)
    }
}

/// Decode a HsByteBuffer as a String, then free it (Rust-allocated).
func stringAndFree(_ buffer: HsByteBuffer) -> String? {
    defer { hs_byte_buffer_free(buffer) }
    guard let ptr = buffer.ptr, buffer.len > 0 else { return nil }
    return String(bytes: UnsafeBufferPointer(start: ptr, count: buffer.len), encoding: .utf8)
}

/// JSON Codable shape for DeviceDescriptor (subset used by the smoke test).
struct DeviceDescriptor: Codable {
    let id: String
    let transport: String
    let available: Bool
}

func main() throws {
    guard hs_abi_version_major() == 1 else { throw NSError(domain: "abi", code: 1) }
    guard hs_abi_version_minor() == 4 else { throw NSError(domain: "abi", code: 14) }
    guard hs_abi_version_patch() == 0 else { throw NSError(domain: "abi", code: 15) }

    // RAII runtime; created and destroyed by the wrapper.
    let runtime = try RuntimeHandle()

    // Call on the calling thread (short operation; production would use a
    // background task). No devices in this environment -> empty array.
    let req = "{\"include_adb\":false,\"include_wifi\":false,\"include_usb\":false}"
    let result = req.withCString { cstr in
        hs_list_devices(runtime.ptrForTest, UnsafePointer<UInt8>(OpaquePointer(cstr)), req.utf8.count)
    }
    guard result.status == 0 else { throw NSError(domain: "ffi", code: 3) }
    guard let json = stringAndFree(result.value) else { throw NSError(domain: "ffi", code: 4) }
    let devices = try JSONDecoder().decode([DeviceDescriptor].self, from: Data(json.utf8))
    guard devices.isEmpty else { throw NSError(domain: "ffi", code: 5) }
    // No value buffer to free here (consumed by stringAndFree).

    // ABI 1.3 surface: diagnostics and trust work without a device.
    let dresult = hs_runtime_diagnostics(runtime.ptrForTest)
    guard dresult.status == 0 else { throw NSError(domain: "ffi", code: 30) }
    guard let djson = stringAndFree(dresult.value),
          djson.contains("\"abi\":\"1.4.0\""),
          djson.contains("\"capabilities\"") else { throw NSError(domain: "ffi", code: 31) }
    let trustResult = hs_trust_list(runtime.ptrForTest)
    guard trustResult.status == 0 else { throw NSError(domain: "ffi", code: 32) }
    guard let tjson = stringAndFree(trustResult.value), tjson == "[]" else { throw NSError(domain: "ffi", code: 33) }
    try runtime.shutdown()

    // Transfer surface (ABI 1.1): a missing session yields a stable
    // session_not_found error (no device in this environment), proving the
    // symbols link and the error JSON round-trips.
    let transfer = try RuntimeHandle()
    let start = "{\"remote_path\":\"/a.bin\",\"local_path\":\"/tmp/a.bin\"}"
    let tresult = start.withCString { cstr in
        hs_transfer_start_download(
            transfer.ptrForTest, 999, UnsafePointer<UInt8>(OpaquePointer(cstr)), start.utf8.count)
    }
    guard tresult.status != 0 else { throw NSError(domain: "ffi", code: 6) }
    guard let terror = stringAndFree(tresult.error),
          terror.contains("session_not_found") else { throw NSError(domain: "ffi", code: 7) }
    // transfer list on an empty runtime is a valid empty array.
    let lresult = hs_transfer_list(transfer.ptrForTest)
    guard lresult.status == 0 else { throw NSError(domain: "ffi", code: 8) }
    guard let ljson = stringAndFree(lresult.value), ljson == "[]" else { throw NSError(domain: "ffi", code: 9) }
    try transfer.shutdown()

    // ABI 1.2 surface: hs_create_directory / hs_ping link and report the
    // stable session_not_found error for a missing session.
    let abi12 = try RuntimeHandle()
    let mkdir = "{\"path\":\"/sdcard/new\"}"
    let mresult = mkdir.withCString { cstr in
        hs_create_directory(
            abi12.ptrForTest, 999, UnsafePointer<UInt8>(OpaquePointer(cstr)), mkdir.utf8.count)
    }
    guard mresult.status != 0 else { throw NSError(domain: "ffi", code: 10) }
    guard let merror = stringAndFree(mresult.error),
          merror.contains("session_not_found") else { throw NSError(domain: "ffi", code: 11) }
    let presult = hs_ping(abi12.ptrForTest, 999)
    guard presult.status != 0 else { throw NSError(domain: "ffi", code: 12) }
    guard let perror = stringAndFree(presult.error),
          perror.contains("session_not_found") else { throw NSError(domain: "ffi", code: 13) }
    try abi12.shutdown()

    print("swift ffi smoke ok")
}

extension RuntimeHandle {
    /// Expose the raw handle for direct FFI calls inside this file.
    var ptrForTest: OpaquePointer { ptr }
}

try main()
