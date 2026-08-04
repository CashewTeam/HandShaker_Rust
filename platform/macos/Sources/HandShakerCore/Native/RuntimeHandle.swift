import Foundation
import HandShakerFFI

/// Owns an `HsRuntime` handle (the Rust runtime: tokio executor +
/// application runtime).
///
/// Threading contract (P1-6): ordinary FFI calls may run **concurrently**
/// (the Rust side serializes internally via its own Tokio executor); a
/// short critical section hands out the raw pointer and bumps an
/// in-flight counter, and `destroy()` marks the handle closing (new calls
/// fail fast with `runtimeClosed`), waits for in-flight calls to drain,
/// then frees the handle. `hs_runtime_destroy` therefore never runs
/// concurrently with a call — the lease replaces the old global NSLock
/// that serialized every network call and blocked one 30s request behind
/// another.
///
/// Contract: never call `destroy()` (or deinit) from *inside* a
/// `withRuntime` body — that would deadlock on the drain wait.
public final class RuntimeHandle: @unchecked Sendable {
    private let condition = NSCondition()
    private var rawPtr: OpaquePointer?
    private var inFlight = 0
    private var destroyed = false

    /// Create a runtime from an `FfiRuntimeConfig` JSON body
    /// (e.g. `{}` or `{"adb_path_utf8":"adb","event_capacity":1024}`; all
    /// fields optional, defaults match `RuntimeConfig::default()`).
    public init(configJson: String) throws {
        var out: OpaquePointer? = nil
        let result: HsCallResult = withHsRequest(configJson) { ptr, len in
            hs_runtime_create(ptr, len, &out)
        }
        if result.status == 0 {
            hs_byte_buffer_free(result.value)
            hs_byte_buffer_free(result.error)
            guard let handle = out else {
                throw HandShakerError.internal("hs_runtime_create returned a NULL handle")
            }
            self.rawPtr = handle
        } else {
            hs_byte_buffer_free(result.value)
            throw HandShakerError.fromNative(decodeNativeError(result.error))
        }
    }

    /// The raw handle pointer, valid only inside `withRuntime`.
    /// Callers (Services layer) must use `withRuntime` and never capture
    /// the pointer past the closure: destroy() may run at any time.
    /// Concurrent calls are allowed; only the pointer hand-out is
    /// serialized (P1-6).
    @discardableResult
    public func withRuntime<T>(_ body: (OpaquePointer) throws -> T) throws -> T {
        let ptr: OpaquePointer
        condition.lock()
        if destroyed {
            condition.unlock()
            throw HandShakerError.runtimeClosed("runtime handle is destroyed")
        }
        guard let handle = rawPtr else {
            condition.unlock()
            throw HandShakerError.runtimeClosed("runtime handle is destroyed")
        }
        inFlight += 1
        ptr = handle
        condition.unlock()
        defer {
            condition.lock()
            inFlight -= 1
            if inFlight == 0 {
                condition.signal()
            }
            condition.unlock()
        }
        return try body(ptr)
    }

    /// Subscribe to the backend event stream. The returned handle owns an
    /// independent Rust-side subscription and must be destroyed with
    /// `SubscriptionHandle.destroy()` (or released via deinit).
    public func subscribe() throws -> SubscriptionHandle {
        try withRuntime { runtime in
            var out: OpaquePointer? = nil
            let result: HsCallResult = hs_subscribe_events(runtime, &out)
            if result.status == 0 {
                hs_byte_buffer_free(result.value)
                hs_byte_buffer_free(result.error)
                guard let handle = out else {
                    throw HandShakerError.internal("hs_subscribe_events returned a NULL handle")
                }
                return SubscriptionHandle(ptr: handle)
            }
            hs_byte_buffer_free(result.value)
            throw HandShakerError.fromNative(decodeNativeError(result.error))
        }
    }

    /// Shut the runtime down and release the handle. Idempotent; safe to
    /// call from any thread and safe on an already-destroyed handle.
    /// Blocks until in-flight calls drain and the Rust side finishes its
    /// shutdown (P1-6: destroy must never race a call).
    public func destroy() {
        condition.lock()
        guard !destroyed else {
            condition.unlock()
            return
        }
        destroyed = true
        while inFlight > 0 {
            condition.wait()
        }
        let ptr = rawPtr
        rawPtr = nil
        condition.unlock()
        if let ptr = ptr {
            hs_runtime_destroy(ptr)
        }
    }

    deinit {
        destroy()
    }
}

/// Owns an `HsSubscription` handle (queue-pull event subscription).
/// `next(timeoutMs:)` blocks up to `timeoutMs` on the Rust side; call it
/// from a background thread, never from the main thread.
public final class SubscriptionHandle: @unchecked Sendable {
    private let lock = NSLock()
    private var rawPtr: OpaquePointer?
    private var destroyed = false

    /// Created by `RuntimeHandle.subscribe()`.
    init(ptr: OpaquePointer) {
        self.rawPtr = ptr
    }

    /// One poll result (P1-4): `timeout` and `closed` are distinct — the
    /// event stream must finish on `.closed` regardless of any local
    /// shutdown flag (the Rust side may close the hub for other reasons,
    /// or the runtime may be destroyed by another owner).
    public enum SubscriptionPoll: Sendable {
        case event(Data)
        case timeout
        case closed
    }

    /// Wait up to `timeoutMs` for the next event.
    ///
    /// - Returns: `.event(raw JSON bytes)` for a real event, `.timeout`
    ///   for a plain poll timeout (`{"timeout":true}`), `.closed` when
    ///   the runtime/hub shut down (`{"closed":true}`).
    /// - Throws: `HandShakerError` on native failures (e.g. a lagged
    ///   subscriber) or when the handle is destroyed.
    public func next(timeoutMs: UInt32) throws -> SubscriptionPoll {
        lock.lock()
        defer { lock.unlock() }
        guard let ptr = rawPtr else {
            throw HandShakerError.runtimeClosed("subscription handle is destroyed")
        }
        let result: HsCallResult = hs_subscription_next(ptr, timeoutMs)
        if result.status != 0 {
            hs_byte_buffer_free(result.value)
            throw HandShakerError.fromNative(decodeNativeError(result.error))
        }
        let json = hsString(result.value)
        hs_byte_buffer_free(result.error)
        guard !json.isEmpty else { return .timeout }
        // Sentinel payloads mean "no event this round".
        if let object = try? JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any] {
            if object["closed"] as? Bool == true {
                return .closed
            }
            if object["timeout"] as? Bool == true {
                return .timeout
            }
        }
        return .event(Data(json.utf8))
    }

    /// Release the subscription handle. Idempotent; NULL-safe on the Rust
    /// side. Must not run concurrently with `next` — the lock guarantees it.
    public func destroy() {
        lock.lock()
        defer { lock.unlock() }
        guard !destroyed else { return }
        destroyed = true
        guard let ptr = rawPtr else { return }
        rawPtr = nil
        hs_subscription_destroy(ptr)
    }

    deinit {
        destroy()
    }
}
