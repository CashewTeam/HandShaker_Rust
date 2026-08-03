import Foundation
import HandShakerFFI

// MARK: - Buffer helpers
//
// Ownership rule (handshaker_ffi.h): Rust allocates, Rust frees. Every
// helper below copies the bytes into Swift memory and then releases the
// Rust buffer with hs_byte_buffer_free, so callers never touch the raw
// pointer after the helper returns. An empty buffer ({NULL, 0, 0}) is safe
// to free.

/// Copy a Rust-allocated buffer into a Swift `String`, then free it.
public func hsString(_ buffer: HsByteBuffer) -> String {
    defer { hs_byte_buffer_free(buffer) }
    guard let ptr = buffer.ptr, buffer.len > 0 else { return "" }
    return String(decoding: UnsafeBufferPointer(start: ptr, count: buffer.len), as: UTF8.self)
}

/// Copy a Rust-allocated buffer into Swift `Data`, then free it.
public func hsData(_ buffer: HsByteBuffer) -> Data {
    defer { hs_byte_buffer_free(buffer) }
    guard let ptr = buffer.ptr, buffer.len > 0 else { return Data() }
    return Data(bytes: ptr, count: buffer.len)
}

// MARK: - Request helpers

/// Run `body` with the UTF-8 bytes of `json` as a pointer+length pair that
/// is valid for the duration of `body` (the pointer lives only inside the
/// `withCString` scope). This is the only sanctioned way to pass request
/// JSON to the FFI.
///
///     let result: HsCallResult = withHsRequest(json) { ptr, len in
///         hs_list_files(runtime, sessionID, ptr, len)
///     }
@discardableResult
public func withHsRequest<T>(_ json: String, _ body: (UnsafePointer<UInt8>?, Int) -> T) -> T {
    json.withCString { cString in
        let bytes = UnsafeRawPointer(cString).assumingMemoryBound(to: UInt8.self)
        return body(bytes, json.utf8.count)
    }
}

// MARK: - Unified call wrappers
//
// Every FFI call that returns HsCallResult must go through one of these
// wrappers. They own buffer lifetime: the unconsumed side of the result is
// always freed, the consumed side is freed by the hsString/hsData helpers.

/// Run one FFI call, decode the success JSON into `T`, or map the native
/// error into a thrown `HandShakerError`.
public func hsCall<T: Decodable & Sendable>(`as` type: T.Type, _ body: () -> HsCallResult) throws -> T {
    let result = body()
    if result.status == 0 {
        // error side is empty on success; freeing an empty buffer is safe.
        hs_byte_buffer_free(result.error)
        let json = hsString(result.value)
        guard !json.isEmpty else {
            throw HandShakerError.decodeError("empty success payload")
        }
        do {
            return try JSONDecoder().decode(T.self, from: Data(json.utf8))
        } catch {
            throw HandShakerError.decodeError("cannot decode \(T.self): \(error)")
        }
    } else {
        // value side is empty on failure.
        hs_byte_buffer_free(result.value)
        throw HandShakerError.fromNative(decodeNativeError(result.error))
    }
}

/// Run one FFI call whose success value carries no meaningful payload
/// (e.g. `{"created":true}`), throwing on failure.
public func hsCallVoid(_ body: () -> HsCallResult) throws {
    let result = body()
    if result.status == 0 {
        hs_byte_buffer_free(result.value)
        hs_byte_buffer_free(result.error)
        return
    }
    hs_byte_buffer_free(result.value)
    throw HandShakerError.fromNative(decodeNativeError(result.error))
}

/// Run one FFI call and return the raw success JSON as `Data` without
/// decoding (e.g. thumbnail cache paths, diagnostics or unknown payloads).
public func hsCallRaw(_ body: () -> HsCallResult) throws -> Data {
    let result = body()
    if result.status == 0 {
        hs_byte_buffer_free(result.error)
        return hsData(result.value)
    }
    hs_byte_buffer_free(result.value)
    throw HandShakerError.fromNative(decodeNativeError(result.error))
}

/// Parse the `PublicError` JSON out of a failed call's error buffer.
/// Consumes (and frees) the buffer.
func decodeNativeError(_ buffer: HsByteBuffer) -> HandShakerNativeError {
    let json = hsString(buffer)
    guard !json.isEmpty,
          let decoded = try? JSONDecoder().decode(HandShakerNativeError.self, from: Data(json.utf8))
    else {
        // The Rust side always serializes a PublicError, but stay defensive:
        // an unparseable error payload must still surface as an error.
        return HandShakerNativeError(
            code: "internal",
            message: json.isEmpty ? "unknown native error" : "undecodable error payload: \(json)",
            retryable: false,
            operation: nil
        )
    }
    return decoded
}
