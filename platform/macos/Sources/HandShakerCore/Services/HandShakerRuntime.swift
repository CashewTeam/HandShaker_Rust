import Foundation
import HandShakerFFI

// MARK: - Shared helpers (Services layer)
//
// The Native layer already provides withHsRequest / hsCall / hsCallVoid /
// hsCallRaw. The Services layer adds two small helpers used by every
// service extension below: a throwing sorted-key JSON encoder for request
// bodies, and a throwing withCString wrapper for FFI arguments that are C
// strings rather than JSON buffers (sync profile ids, media merge kinds).

/// Encode a request body as sorted-key JSON (same deterministic style as
/// `RuntimeConfig.jsonBody()`). Throws `HandShakerError.internal` instead
/// of leaking `EncodingError`.
enum ServicesJSON {
    static func encode<T: Encodable>(_ value: T) throws -> String {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        do {
            let data = try encoder.encode(value)
            guard let json = String(data: data, encoding: .utf8) else {
                throw HandShakerError.internal("request body is not valid UTF-8")
            }
            return json
        } catch let error as HandShakerError {
            throw error
        } catch {
            throw HandShakerError.internal("request encoding failed: \(error)")
        }
    }
}

/// Run `body` with the UTF-8 bytes of `string` as a pointer+length pair
/// that is valid for the duration of `body`. This is the sanctioned way to
/// pass C-string arguments (e.g. `hs_sync_status` profile ids) to the FFI;
/// JSON request bodies should use `withHsRequestThrowing` instead.
func withHsString<T>(_ string: String, _ body: (UnsafePointer<UInt8>?, Int) throws -> T) throws -> T {
    try string.withCString { cString in
        let bytes = UnsafeRawPointer(cString).assumingMemoryBound(to: UInt8.self)
        return try body(bytes, string.utf8.count)
    }
}

/// Throwing variant of the Native `withHsRequest` helper: its body is
/// non-throwing, but the Services layer needs `try hsCall` *inside* the
/// pointer scope, so this mirror is used for every JSON request body.
func withHsRequestThrowing<T>(
    _ json: String,
    _ body: (UnsafePointer<UInt8>?, Int) throws -> T
) throws -> T {
    try json.withCString { cString in
        let bytes = UnsafeRawPointer(cString).assumingMemoryBound(to: UInt8.self)
        return try body(bytes, json.utf8.count)
    }
}

/// Shared shutdown signal passed to event-polling tasks. `shutdown()`
/// sets it *before* calling `hs_runtime_shutdown`, so by the time the
/// subscription reports the `{"closed":true}` sentinel the flag is already
/// set — the event stream can finish instead of polling forever.
final class ShutdownFlag: @unchecked Sendable {
    private let lock = NSLock()
    private var value = false

    var isSet: Bool { lock.withLock { value } }

    func set() {
        lock.withLock { value = true }
    }
}

// MARK: - Request DTOs (FFI JSON contracts)

/// Connection request. `hs_connect`'s request JSON is the bare
/// `DeviceDescriptor` (handshaker_ffi.h), so this wrapper encodes/decodes
/// transparently as its `device` payload — the Swift API stays explicit
/// while the wire contract is unchanged.
public struct ConnectRequest: Codable, Sendable, Equatable {
    /// A descriptor as returned by `listDevices()` (or hand-built with a
    /// matching transport target).
    public let device: DeviceDescriptor

    public init(device: DeviceDescriptor) {
        self.device = device
    }

    public init(from decoder: Decoder) throws {
        device = try DeviceDescriptor(from: decoder)
    }

    public func encode(to encoder: Encoder) throws {
        try device.encode(to: encoder)
    }
}

/// Result of `hs_connect`: `{"session_id": N}`.
private struct SessionIDResult: Decodable {
    let sessionID: UInt64

    private enum CodingKeys: String, CodingKey {
        case sessionID = "session_id"
    }
}

// MARK: - HandShakerRuntime

/// The Swift entry point for the HandShaker backend: owns one `HsRuntime`
/// (the Rust runtime: tokio executor + application runtime) and exposes
/// every service as an actor-isolated method.
///
/// Threading model (P1-6):
///  - Service methods are `async throws`; the FFI call itself runs on a
///    dedicated concurrent dispatch queue (`callNative`), never on the
///    actor executor or the caller's cooperative pool. The Rust side
///    serializes internally, and `RuntimeHandle.withRuntime` hands out a
///    short-lived lease — ordinary calls run concurrently, only destroy
///    waits for in-flight calls to drain.
///  - Every FFI call goes through `hsCall`/`hsCallVoid`/`hsCallRaw` (via
///    `RuntimeHandle.withRuntime`), so failures surface as typed
///    `HandShakerError` values and buffers are always freed.
///  - `eventStream()` runs the queue-pull polling on a background `Task`,
///    so it never blocks the actor.
///
/// Lifecycle: `init` verifies the ABI (≥ 1.5.0) and creates the runtime;
/// `shutdown()` is idempotent; the native handle is destroyed when the
/// actor is deallocated.
public actor HandShakerRuntime {
    /// Native runtime handle; all FFI calls go through it.
    let handle: RuntimeHandle
    /// Set by `shutdown()` before the FFI shutdown, so event-polling tasks
    /// can finish on the `closed` sentinel.
    let shutdownFlag = ShutdownFlag()

    /// Dedicated concurrent queue for native calls (P1-6): the FFI uses
    /// Tokio `block_on` internally, so a call blocks its own thread — it
    /// must never block the actor executor or a caller's cooperative
    /// thread pool. Long-running operations (transfers, syncs) are started
    /// here and their completion is observed via ids + events, not by
    /// blocking the queue.
    private static let nativeQueue = DispatchQueue(
        label: "handshaker.native",
        qos: .userInitiated,
        attributes: .concurrent
    )

    /// Run `body` (the actual FFI call) on the native queue and await its
    /// result. `body` must be self-contained: it captures the request
    /// body and session ids, not actor state.
    ///
    /// Cancellation note: the FFI call runs synchronously on the queue
    /// (Tokio `block_on`), so cancelling the awaiting task does NOT abort
    /// it — the call still completes on the queue and the result is
    /// discarded. Long-running work must be started as a transfer/sync id
    /// and observed via events, exactly as the service contract requires.
    func callNative<T: Sendable>(
        _ body: @escaping @Sendable () throws -> T
    ) async throws -> T {
        try await withCheckedThrowingContinuation { continuation in
            Self.nativeQueue.async {
                continuation.resume(with: Result { try body() })
            }
        }
    }

    /// Create the runtime: verifies the loaded ABI (≥ 1.5.0) via
    /// `checkABI()`, then calls `hs_runtime_create` with the config JSON.
    ///
    /// - Parameter config: creation configuration; defaults match
    ///   `RuntimeConfig.defaults` (Rust-side `RuntimeConfig::default()`).
    public init(config: RuntimeConfig = .defaults) throws {
        try checkABI()
        self.handle = try RuntimeHandle(configJson: config.jsonBody())
    }

    deinit {
        handle.destroy()
    }

    // MARK: - Lifecycle

    /// Shut the runtime down (`hs_runtime_shutdown`). Idempotent: a second
    /// call (or a call after the handle was destroyed) is a no-op. Sets the
    /// shutdown flag first so active event streams finish with `closed`.
    public func shutdown() async {
        shutdownFlag.set()
        _ = try? await callNative {
            try self.handle.withRuntime { runtime in
                let result: HsCallResult = hs_runtime_shutdown(runtime)
                hs_byte_buffer_free(result.value)
                hs_byte_buffer_free(result.error)
            }
        }
    }

    // MARK: - Devices

    /// List currently visible devices across the enabled transports
    /// (`hs_list_devices`, request `{}` → all transports enabled).
    /// Per-channel failures are surfaced as warnings only by
    /// `discoverDevices()`; an empty list is the no-device result.
    public func listDevices() async throws -> [DeviceDescriptor] {
        try await callNative {
            try self.handle.withRuntime { runtime in
                try withHsRequestThrowing("{}") { ptr, len in
                    try hsCall(as: [DeviceDescriptor].self) {
                        hs_list_devices(runtime, ptr, len)
                    }
                }
            }
        }
    }

    /// Multi-transport discovery sweep (`hs_discover_devices`, no request
    /// body). Per-channel failures are reported as warnings instead of an
    /// empty-array lie.
    public func discoverDevices() async throws -> DeviceDiscoveryResult {
        try await callNative {
            try self.handle.withRuntime { runtime in
                try hsCall(as: DeviceDiscoveryResult.self) {
                    hs_discover_devices(runtime)
                }
            }
        }
    }

    // MARK: - Sessions

    /// Open a session for the device described by `request`
    /// (`hs_connect` → `{"session_id":N}`, then `hs_get_session` for the
    /// reconciled `SessionSnapshot`). Throws the native connect error
    /// (e.g. `.trustRequired`, `.connectFailed`) on failure.
    public func connect(_ request: ConnectRequest) async throws -> SessionSnapshot {
        let body = try ServicesJSON.encode(request)
        let sessionID = try await callNative {
            try self.handle.withRuntime { runtime in
                try withHsRequestThrowing(body) { ptr, len in
                    try hsCall(as: SessionIDResult.self) {
                        hs_connect(runtime, ptr, len)
                    }
                }
            }
        }
        return try await session(sessionID: sessionID.sessionID)
    }

    /// Close the session (`hs_disconnect`, result `{"disconnected":true}`).
    public func disconnect(sessionID: UInt64) async throws {
        try await callNative {
            try self.handle.withRuntime { runtime in
                try hsCallVoid { hs_disconnect(runtime, sessionID) }
            }
        }
    }

    /// Current snapshot of one open session (`hs_get_session`).
    public func session(sessionID: UInt64) async throws -> SessionSnapshot {
        try await callNative {
            try self.handle.withRuntime { runtime in
                try hsCall(as: SessionSnapshot.self) {
                    hs_get_session(runtime, sessionID)
                }
            }
        }
    }

    /// Round-trip latency of a device ping (`hs_ping`, result
    /// `{"round_trip_ms":N}`).
    public func ping(sessionID: UInt64) async throws -> PingResult {
        try await callNative {
            try self.handle.withRuntime { runtime in
                try hsCall(as: PingResult.self) {
                    hs_ping(runtime, sessionID)
                }
            }
        }
    }

    // MARK: - Diagnostics

    /// Runtime diagnostics (`hs_runtime_diagnostics`): ABI/API versions,
    /// adb probe (never fails the call), state dir, active sessions and
    /// transfers, and the feature capability list.
    public func diagnostics() async throws -> RuntimeDiagnostics {
        try await callNative {
            try self.handle.withRuntime { runtime in
                try hsCall(as: RuntimeDiagnostics.self) {
                    hs_runtime_diagnostics(runtime)
                }
            }
        }
    }
}
