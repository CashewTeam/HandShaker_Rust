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
        // next() must time out and return nil.
        let data = try subscription.next(timeoutMs: 200)
        XCTAssertNil(data, "no events expected; nil means timeout/closed")
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
}
