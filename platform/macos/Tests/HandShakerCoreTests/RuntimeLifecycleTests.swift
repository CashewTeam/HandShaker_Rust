import XCTest
@testable import HandShakerCore

/// RuntimeHandle / SubscriptionHandle lifecycle against the real native
/// library. No device is connected anywhere in these tests (CI has none):
/// they only exercise create → destroy and NULL/destroyed-handle safety.
final class RuntimeLifecycleTests: XCTestCase {
    func testCreateAndDestroy() throws {
        let runtime = try RuntimeHandle(configJson: "{}")
        runtime.destroy()
        runtime.destroy() // idempotent, must not crash
    }

    func testCreateWithExplicitConfig() throws {
        let runtime = try RuntimeHandle(
            configJson: #"{"adb_path_utf8":"adb","event_capacity":64,"state_dir_utf8":"/tmp/hs-swift-test"}"#
        )
        runtime.destroy()
    }

    func testInvalidConfigJSONThrows() {
        XCTAssertThrowsError(try RuntimeHandle(configJson: "{not json")) { error in
            XCTAssertEqual(error as? HandShakerError, .invalidArgument("invalid config JSON"))
        }
    }

    func testWithRuntimeRunsBody() throws {
        let runtime = try RuntimeHandle(configJson: "{}")
        defer { runtime.destroy() }
        let seen = try runtime.withRuntime { _ in 42 }
        XCTAssertEqual(seen, 42)
    }

    func testDestroyedHandleRejectsCalls() throws {
        let runtime = try RuntimeHandle(configJson: "{}")
        runtime.destroy()
        // withRuntime on a destroyed handle must fail fast (never touch a
        // freed pointer).
        XCTAssertThrowsError(try runtime.withRuntime { _ in () }) { error in
            guard case .runtimeClosed = error as? HandShakerError else {
                return XCTFail("expected .runtimeClosed, got \(error)")
            }
        }
    }

    func testDoubleDestroyIsSafe() throws {
        let runtime = try RuntimeHandle(configJson: "{}")
        runtime.destroy()
        runtime.destroy()
        // Deinit also runs destroy(); all paths must be idempotent.
    }

    func testSubscriptionLifecycle() throws {
        let runtime = try RuntimeHandle(configJson: "{}")
        defer { runtime.destroy() }
        let subscription = try runtime.subscribe()
        // No events are produced after subscribe (nothing connects), so
        // next() must time out — as .timeout, not .closed (P1-4).
        let poll = try subscription.next(timeoutMs: 200)
        guard case .timeout = poll else {
            return XCTFail("expected .timeout, got \(poll)")
        }
        subscription.destroy()
        subscription.destroy() // idempotent
    }

    func testDestroyedSubscriptionRejectsNext() throws {
        let runtime = try RuntimeHandle(configJson: "{}")
        defer { runtime.destroy() }
        let subscription = try runtime.subscribe()
        subscription.destroy()
        XCTAssertThrowsError(try subscription.next(timeoutMs: 10)) { error in
            guard case .runtimeClosed = error as? HandShakerError else {
                return XCTFail("expected .runtimeClosed, got \(error)")
            }
        }
    }

    func testRuntimeConfigJSONBodyOmitsNil() {
        let empty = RuntimeConfig()
        XCTAssertEqual(empty.jsonBody(), "{}")
        let configured = RuntimeConfig(adbPathUTF8: "adb", eventCapacity: 64)
        let json = configured.jsonBody()
        XCTAssertTrue(json.contains("\"adb_path_utf8\":\"adb\""), json)
        XCTAssertTrue(json.contains("\"event_capacity\":64"), json)
        XCTAssertFalse(json.contains("state_dir_utf8"), json)
        // The body must be valid JSON and decode back to the same values.
        let data = Data(json.utf8)
        let decoded = try? JSONDecoder().decode(RuntimeConfig.self, from: data)
        XCTAssertEqual(decoded?.adbPathUTF8, "adb")
        XCTAssertEqual(decoded?.eventCapacity, 64)
        XCTAssertNil(decoded?.stateDirUTF8)
    }

    // MARK: - P1-6 lifecycle lease

    func testConcurrentCallsRunAndDestroyDrainsInflight() throws {
        // P1-6: ordinary calls run concurrently (no global serial lock) and
        // destroy() waits for in-flight calls to drain before freeing the
        // handle. Deterministic: all calls enter withRuntime first, then
        // destroy races their in-flight bodies.
        let runtime = try RuntimeHandle(configJson: "{}")
        let count = 4
        let queue = DispatchQueue(label: "hs.concurrent", attributes: .concurrent)
        let entered = DispatchSemaphore(value: 0)
        let allFinished = DispatchSemaphore(value: 0)
        let lock = NSLock()
        var completed = 0
        for _ in 0..<count {
            queue.async {
                _ = try? runtime.withRuntime { _ in
                    entered.signal() // in-flight counter already bumped
                    Thread.sleep(forTimeInterval: 0.1) // simulated call
                    return 0
                }
                lock.lock()
                completed += 1
                lock.unlock()
                if completed == count {
                    allFinished.signal()
                }
            }
        }
        for _ in 0..<count {
            entered.wait()
        }
        // All four calls are inside withRuntime now; destroy must block
        // until they drain.
        runtime.destroy()
        XCTAssertEqual(completed, count, "destroy must wait for in-flight calls")
        XCTAssertThrowsError(try runtime.withRuntime { _ in 0 }) { error in
            guard case .runtimeClosed = error as? HandShakerError else {
                return XCTFail("expected .runtimeClosed after destroy, got \(error)")
            }
        }
    }

    func testConcurrentCallsBothSucceedWithOneLeaseEach() throws {
        // Two overlapping withRuntime calls must both succeed (the lease
        // hands out the same pointer twice — the Rust side serializes
        // internally). The old global lock would also pass this, but it
        // also serialized unrelated network calls; the lease keeps the
        // pointer safe without blocking.
        let runtime = try RuntimeHandle(configJson: "{}")
        defer { runtime.destroy() }
        let queue = DispatchQueue(label: "hs.concurrent.2", attributes: .concurrent)
        let barrier = DispatchSemaphore(value: 0)
        let results = NSLock()
        var observed: [Int] = []
        let bothDone = DispatchSemaphore(value: 0)
        var finished = 0
        for value in [11, 22] {
            queue.async {
                let result = try? runtime.withRuntime { _ -> Int in
                    barrier.wait()
                    return value
                }
                results.lock()
                if let result = result { observed.append(result) }
                results.unlock()
                finished += 1
                if finished == 2 { bothDone.signal() }
            }
        }
        barrier.signal()
        barrier.signal()
        bothDone.wait()
        XCTAssertEqual(observed.sorted(), [11, 22], "both concurrent calls must complete")
    }

    func testConcurrentRealFFICallsSucceed() async throws {
        // Review follow-up: the lease premise is that REAL overlapping FFI
        // calls are safe (the Rust side serializes on its own executor) —
        // not just sleep-imitated bodies. Four concurrent real
        // hs_runtime_diagnostics calls (via the public API) must all
        // succeed; the actor suspends inside callNative, so the FFI calls
        // genuinely overlap on the native queue.
        let runtime = try HandShakerRuntime(config: .defaults)
        defer { handleShutdown(runtime) }
        let count = 4
        var successes = 0
        await withTaskGroup(of: Bool.self) { group in
            for _ in 0..<count {
                group.addTask {
                    (try? await runtime.diagnostics()) != nil
                }
            }
            for await ok in group where ok {
                successes += 1
            }
        }
        XCTAssertEqual(successes, count, "all overlapping real FFI calls must succeed")
    }

    private func handleShutdown(_ runtime: HandShakerRuntime) {
        // Fire-and-forget: the actor is deallocated with the runtime when
        // the test ends; an explicit shutdown is still attempted to keep
        // the native side deterministic.
        Task { await runtime.shutdown() }
    }
}
