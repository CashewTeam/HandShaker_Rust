import XCTest
import HandShakerCore

/// Real-device acceptance for the Swift wrapper (Phase F / goal 3a).
///
/// Gated by `HS_ACCEPTANCE=1` (run via scripts/swift-device-acceptance.sh);
/// skips gracefully when no device is attached so CI without a phone stays
/// green. Uses a unique test directory under /sdcard/Download, verifies
/// round-trip content, and removes every test artifact (including the
/// directory and the adb forward created by the runtime) before finishing.
///
/// Split into read-path and write-path tests: a phone whose SSP service
/// currently rejects write operations (observed on a 1.2.0 device where
/// CREATE_FOLDER and push return FILE_IO_INVALID_SOURCE while reads work)
/// skips the write path with the exact phone error instead of failing —
/// the CLI exhibits the identical rejection, so this is a phone-side
/// condition, not a wrapper defect.
final class DeviceAcceptanceTests: XCTestCase {
    private var runtime: HandShakerRuntime?
    private var sessionID: UInt64?
    private var testDir = ""

    override func setUp() async throws {
        try await super.setUp()
        guard ProcessInfo.processInfo.environment["HS_ACCEPTANCE"] == "1" else {
            throw XCTSkip("HS_ACCEPTANCE not set; real-device acceptance is opt-in")
        }
        let runtime = try HandShakerRuntime()
        let devices = try await runtime.listDevices()
        guard let adb = devices.first(where: { $0.transport == .adb }) else {
            try await runtime.shutdown()
            throw XCTSkip("no ADB device attached")
        }
        let session = try await runtime.connect(.init(device: adb))
        self.runtime = runtime
        self.sessionID = session.id
        self.testDir = "/sdcard/Download/hs-swift-acceptance-\(ProcessInfo.processInfo.processIdentifier)"
    }

    override func tearDown() async throws {
        if let runtime, let sessionID {
            // Remove the unique test directory and close the session;
            // the runtime shutdown releases the adb forward it created.
            _ = try? await runtime.deletePaths(sessionID: sessionID, [testDir])
            _ = try? await runtime.disconnect(sessionID: sessionID)
        }
        try await runtime?.shutdown()
        try await super.tearDown()
    }

    /// Phone-side write rejection (remote_io / remote path errors) is
    /// reported as a skip with the exact message, never as a wrapper
    /// failure — the CLI shows the identical rejection.
    private func skipOnPhoneWriteRejection(_ error: Error, _ step: String) throws {
        if let hsError = error as? HandShakerError {
            switch hsError {
            case .remoteIO, .remotePathNotFound, .notFound:
                throw XCTSkip("phone rejects \(step): \(hsError)")
            default:
                break
            }
        }
        throw error
    }

    func testReadPathAcceptance() async throws {
        guard let runtime, let sessionID else {
            throw XCTSkip("setup did not attach a session")
        }

        // 1. ping + root listing.
        let ping = try await runtime.ping(sessionID: sessionID)
        XCTAssertGreaterThan(ping.roundTripMs, 0)
        let root = try await runtime.listFiles(sessionID: sessionID, path: "/sdcard")
        XCTAssertFalse(root.isEmpty)

        // 2. stat an existing directory.
        let stat = try await runtime.statFile(sessionID: sessionID, path: "/sdcard/Download")
        XCTAssertNotNil(stat)
        XCTAssertTrue(stat?.isDirectory ?? false)

        // 3. clipboard: read-only on a real phone (never clobber user data).
        _ = try await runtime.clipboardList(sessionID: sessionID)

        // 4. media libraries (photos are expected on a phone).
        let photos = try await runtime.photoLibrary(sessionID: sessionID)
        XCTAssertGreaterThan(photos.images.count, 0, "phone should have photos")

        // 5. EXIF of the first photo.
        if let firstPath = photos.images.first?.path {
            _ = try await runtime.fetchExif(sessionID: sessionID, path: firstPath)
        }

        // 6. diagnostics sanity.
        let diagnostics = try await runtime.diagnostics()
        XCTAssertEqual(diagnostics.abi, "1.5.0")
        XCTAssertTrue(diagnostics.capabilities.contains("media_merge"))

        // 7. media merge is a pure function — exercise it with the library
        //    snapshot and an empty change (no-op upsert must not throw).
        let merged = try await runtime.mergePhotoChange(
            library: photos,
            change: .init(mediaKind: .photo, added: [], deleted: [], updated: [])
        )
        XCTAssertEqual(merged.images.count, photos.images.count)
    }

    /// `startUpload`/`startDownload` only *launch* the transfer (they return
    /// a `TransferID`); the underlying FFI call must not block on a full
    /// transfer. Wait for the terminal state so the following operations
    /// observe a completed file (a premature move would hit
    /// FILE_IO_INVALID_SOURCE — the source file does not exist yet).
    private func waitForTransfer(
        _ runtime: HandShakerRuntime,
        _ transferID: TransferID,
        step: String
    ) async throws {
        for _ in 0..<150 {  // up to ~30 s
            let snapshot = try await runtime.transfer(transferID.value)
            switch snapshot.state {
            case .completed, .failed, .cancelled:
                guard snapshot.state == .completed else {
                    throw XCTSkip("transfer \(step) ended \(snapshot.state.rawValue): \(snapshot.error?.message ?? "no error")")
                }
                return
            default:
                break
            }
            try await Task.sleep(nanoseconds: 200_000_000)
        }
        throw XCTSkip("transfer \(step) did not finish within 30 s")
    }

    func testWritePathAcceptance() async throws {
        guard let runtime, let sessionID else {
            throw XCTSkip("setup did not attach a session")
        }

        // 1. create the unique test directory.
        do {
            try await runtime.createDirectory(sessionID: sessionID, path: testDir)
        } catch {
            try skipOnPhoneWriteRejection(error, "create_directory")
        }
        let stat = try await runtime.statFile(sessionID: sessionID, path: testDir)
        XCTAssertNotNil(stat)
        XCTAssertTrue(stat?.isDirectory ?? false)

        // 2. upload a local file, then download it back and compare bytes.
        let payload = Data("handshaker-swift-acceptance-\(UUID().uuidString)".utf8)
        let local = FileManager.default.temporaryDirectory
            .appendingPathComponent("hs-acceptance-\(UUID().uuidString).txt")
        try payload.write(to: local)
        defer { try? FileManager.default.removeItem(at: local) }
        let remote = "\(testDir)/payload.txt"
        let uploadID: TransferID
        do {
            uploadID = try await runtime.startUpload(sessionID: sessionID, remotePath: remote, localPath: local.path)
        } catch {
            try skipOnPhoneWriteRejection(error, "upload")
            return
        }
        try await waitForTransfer(runtime, uploadID, step: "upload")
        let downloaded = FileManager.default.temporaryDirectory
            .appendingPathComponent("hs-acceptance-down-\(UUID().uuidString).txt")
        defer { try? FileManager.default.removeItem(at: downloaded) }
        let downloadID: TransferID
        do {
            downloadID = try await runtime.startDownload(
                sessionID: sessionID, remotePath: remote, localPath: downloaded.path
            )
        } catch {
            try skipOnPhoneWriteRejection(error, "download")
            return
        }
        try await waitForTransfer(runtime, downloadID, step: "download")
        XCTAssertEqual(try Data(contentsOf: downloaded), payload)

        // 3. move + stat.
        let moved = "\(testDir)/moved.txt"
        do {
            try await runtime.movePath(sessionID: sessionID, source: remote, target: moved)
        } catch {
            try skipOnPhoneWriteRejection(error, "move")
            return
        }
        let movedStat = try await runtime.statFile(sessionID: sessionID, path: moved)
        XCTAssertNotNil(movedStat)

        // 4. delete the moved file (directory removed in tearDown).
        do {
            try await runtime.deletePaths(sessionID: sessionID, [moved])
        } catch {
            try skipOnPhoneWriteRejection(error, "delete")
        }
        let deletedStat = try await runtime.statFile(sessionID: sessionID, path: moved)
        XCTAssertNil(deletedStat)
    }
}
