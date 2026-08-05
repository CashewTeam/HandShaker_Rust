import XCTest
@testable import HandShakerCore

/// Services-layer smoke tests against the real native library. No device
/// is connected anywhere in these tests (CI has none): they exercise
/// runtime creation, device listing, diagnostics, the event stream and one
/// session-scoped error path. All calls are actor-isolated, so they are
/// `async` on the test side.
final class ServiceTests: XCTestCase {
    func testRuntimeCreateAndListDevices() async throws {
        let runtime = try HandShakerRuntime()
        // hs_list_devices must not throw; device count depends on the host
        // (a phone may or may not be attached), so only decode correctness
        // is asserted here.
        let devices = try await runtime.listDevices()
        for device in devices {
            XCTAssertFalse(device.id.isEmpty)
            XCTAssertNotNil(device.transport)
        }
        try await runtime.shutdown()
    }

    func testDiscoverDevicesReturnsWarningsNotErrors() async throws {
        let runtime = try HandShakerRuntime()
        // Discovery never fails the call: per-channel failures are warnings.
        let result = try await runtime.discoverDevices()
        // Devices may or may not be present; the contract is that the call
        // succeeds and reports warnings for failed channels.
        XCTAssertNotNil(result.devices)
        XCTAssertNotNil(result.warnings)
        try await runtime.shutdown()
    }

    func testDiagnosticsReportsABI() async throws {
        let runtime = try HandShakerRuntime()
        let diagnostics = try await runtime.diagnostics()
        XCTAssertEqual(diagnostics.abi, "1.5.0")
        XCTAssertTrue(
            diagnostics.capabilities.contains("files"),
            "expected the files capability, got \(diagnostics.capabilities)"
        )
        XCTAssertTrue(
            diagnostics.capabilities.contains("media_merge"),
            "expected the media_merge capability (ABI 1.5), got \(diagnostics.capabilities)"
        )
        try await runtime.shutdown()
    }

    func testMediaPageRequestJSONUsesNumericValuesAndOmitsNil() throws {
        XCTAssertEqual(
            try ServicesJSON.encode(MediaPageRequest(limit: 25, cursor: 7)),
            #"{"cursor":7,"limit":25}"#
        )
        XCTAssertEqual(
            try ServicesJSON.encode(MediaPageRequest(limit: nil, cursor: nil)),
            "{}"
        )
    }

    func testEmptyThumbnailStreamFinishesWithoutSessionCall() async throws {
        let runtime = try HandShakerRuntime()
        let stream = await runtime.thumbnailStream(sessionID: 999)
        var yielded = 0
        for try await _ in stream {
            yielded += 1
        }
        XCTAssertEqual(yielded, 0)
        try await runtime.shutdown()
    }

    func testThumbnailStreamRejectsInvalidBatchConfiguration() async throws {
        let runtime = try HandShakerRuntime()
        let stream = await runtime.thumbnailStream(sessionID: 999, batchSize: 0)
        do {
            for try await _ in stream {}
            XCTFail("expected invalid thumbnail stream configuration")
        } catch let error as HandShakerError {
            guard case .invalidArgument = error else {
                return XCTFail("expected .invalidArgument, got \(error)")
            }
        }
        try await runtime.shutdown()
    }

    func testEventStreamCancelStopsPoller() async throws {
        let runtime = try HandShakerRuntime()
        let stream = try await runtime.eventStream()
        let consumer = Task { () -> Int in
            var received = 0
            do {
                for try await _ in stream {
                    received += 1
                }
            } catch {
                return -1
            }
            return received
        }
        // Let the poller start and block on hs_subscription_next.
        try await Task.sleep(for: .milliseconds(200))
        consumer.cancel()
        // The stream must terminate promptly after cancellation (the
        // poller notices the cancellation, finishes, and destroys the
        // subscription handle — nothing leaks).
        let received = await consumer.value
        XCTAssertEqual(received, 0, "no events expected; nothing connects")
        try await runtime.shutdown()
    }

    func testEventStreamFinishesAfterShutdown() async throws {
        let runtime = try HandShakerRuntime()
        let stream = try await runtime.eventStream()
        let consumer = Task { () -> Bool in
            do {
                for try await _ in stream {
                    // Drain until the poller finishes the stream after
                    // runtime shutdown.
                }
                return true // finished normally (closed after shutdown)
            } catch {
                return false
            }
        }
        try await Task.sleep(for: .milliseconds(200))
        try await runtime.shutdown()
        // The poller sees the closed sentinel and finishes the stream.
        let finishedNormally = await consumer.value
        XCTAssertTrue(finishedNormally, "event stream must finish after runtime shutdown")
    }

    func testStatFileUnknownSessionThrowsSessionNotFound() async throws {
        let runtime = try HandShakerRuntime()
        do {
            _ = try await runtime.statFile(sessionID: 999, path: "/")
            XCTFail("expected HandShakerError.sessionNotFound for an unknown session")
        } catch let error as HandShakerError {
            guard case .sessionNotFound = error else {
                return XCTFail("expected .sessionNotFound, got \(error)")
            }
        } catch {
            XCTFail("expected HandShakerError, got \(error)")
        }
        try await runtime.shutdown()
    }

    func testTransferUnknownIDThrowsTransferNotFound() async throws {
        let runtime = try HandShakerRuntime()
        do {
            _ = try await runtime.transfer(999)
            XCTFail("expected HandShakerError.transferNotFound for an unknown transfer")
        } catch let error as HandShakerError {
            guard case .transferNotFound = error else {
                return XCTFail("expected .transferNotFound, got \(error)")
            }
        } catch {
            XCTFail("expected HandShakerError, got \(error)")
        }
        try await runtime.shutdown()
    }

    func testShutdownIsIdempotent() async throws {
        let runtime = try HandShakerRuntime()
        try await runtime.shutdown()
        try await runtime.shutdown() // must not crash or throw
        // A second shutdown after the first is a no-op by contract.
    }
}
